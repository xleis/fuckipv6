use anyhow::Result;
use fuckipv6_common::NetworkTicket;
use fuckipv6_coordinator::signaling::session::{PeerSnapshot, TrafficSnapshot};
use fuckipv6_coordinator::webui::{NetworkConfigDto, UpdateNetworkConfigRequest};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::node::{
    NodeCommand, NodeCommandBus, NodeCommandBusError, NodeCommandOutcome, NodeCommandRequest,
    NodeCreateNetworkCommand, NodeEmbeddedServiceStatus, NodeJoinNetworkCommand, NodeNetworkRole,
    NodeNetworkStatus, NodePeerProbeSnapshot, NodeProbePeerCommand, NodeProblem, NodeProblemCode,
    NodeRoutingMode, NodeSnapshot, NodeStateStore, NodeUpdateRoutingCommand,
};
#[cfg(test)]
use crate::node::{NodeCommandFuture, NodeCommandHandler, NodeCommandResult};
use crate::p2p::connection::PeerProbeOutcome;
use crate::remote_node::RemoteNodeSession;

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

    // === 本机节点运行时状态（来自共享 NodeStateStore） ===
    #[serde(default)]
    pub runtime_available: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_p2p_addr: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_traffic: Option<RuntimeTrafficDto>,
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
struct NodeBackend {
    state: NodeStateStore,
    commands: NodeCommandBus,
    /// Keep the UI-owned node session alive exactly as long as this adapter.
    /// Dropping it only disconnects the adapter; the node process continues.
    _session: Option<Arc<RemoteNodeSession>>,
}

#[cfg(test)]
struct TestNodeHandler {
    state: NodeStateStore,
}

#[cfg(test)]
impl NodeCommandHandler for TestNodeHandler {
    fn handle(&self, request: NodeCommandRequest) -> NodeCommandFuture {
        let state = self.state.clone();
        Box::pin(async move {
            let request_id = request.request_id;
            match request.command {
                NodeCommand::GetSnapshot => {
                    NodeCommandResult::snapshot(request_id, state.snapshot().await)
                }
                NodeCommand::GetUiManifest => {
                    NodeCommandResult::ui_manifest(request_id, crate::node::node_ui_manifest())
                }
                _ => NodeCommandResult::completed(
                    request_id,
                    NodeCommandOutcome::Completed {
                        message: "test node command completed".to_string(),
                    },
                ),
            }
        })
    }
}

/// Typed controller failure that preserves the canonical node problem across
/// the legacy WebUI HTTP boundary.
#[derive(Debug, Clone)]
pub struct AppControllerError {
    problem: NodeProblem,
}

impl AppControllerError {
    pub fn from_problem(problem: NodeProblem) -> Self {
        Self { problem }
    }

    pub fn invalid(message: impl Into<String>) -> Self {
        Self::from_problem(NodeProblem::new(NodeProblemCode::InvalidRequest, message))
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::from_problem(NodeProblem::new(NodeProblemCode::NotFound, message))
    }

    pub fn unsupported(message: impl Into<String>) -> Self {
        Self::from_problem(NodeProblem::new(NodeProblemCode::Unsupported, message))
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::from_problem(NodeProblem::new(NodeProblemCode::Internal, message))
    }

    pub fn problem(&self) -> &NodeProblem {
        &self.problem
    }
}

impl fmt::Display for AppControllerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.problem.message)
    }
}

impl std::error::Error for AppControllerError {}

impl From<NodeCommandBusError> for AppControllerError {
    fn from(error: NodeCommandBusError) -> Self {
        match error {
            NodeCommandBusError::Invalid(problem) => Self::from_problem(problem),
            NodeCommandBusError::Closed => Self::from_problem(
                NodeProblem::new(
                    NodeProblemCode::Unavailable,
                    "节点命令总线已关闭，控制服务暂时不可用",
                )
                .retryable(true),
            ),
            NodeCommandBusError::ResponseDropped(request_id) => Self::from_problem(
                NodeProblem::new(
                    NodeProblemCode::Unavailable,
                    format!("节点命令 {request_id} 的结果通道提前关闭，请重试"),
                )
                .retryable(true),
            ),
        }
    }
}

#[derive(Clone)]
pub struct AppController {
    node: NodeBackend,
    owner_ticket: Arc<RwLock<Option<String>>>,
    recent_logs: Arc<RwLock<VecDeque<String>>>,
    request_id: Arc<AtomicU64>,
}

impl AppController {
    /// Test-only controller backed by a minimal authoritative node command bus.
    /// Production construction always attaches the local node session.
    #[cfg(test)]
    pub fn new() -> Self {
        let events = crate::node::NodeEventBus::new(32);
        let state = NodeStateStore::with_events(events.clone());
        let commands = NodeCommandBus::spawn(
            Arc::new(TestNodeHandler {
                state: state.clone(),
            }),
            events,
            8,
        );
        Self::from_backend(NodeBackend {
            state,
            commands,
            _session: None,
        })
    }

    /// Attach a WebUI controller to the shared node state and command bus while
    /// keeping ownership of the local node session guard.
    pub fn from_node_session(session: RemoteNodeSession) -> Self {
        let backend = NodeBackend {
            state: session.state(),
            commands: session.commands(),
            _session: Some(Arc::new(session)),
        };
        Self::from_backend(backend)
    }

    fn from_backend(node: NodeBackend) -> Self {
        Self {
            node,
            owner_ticket: Arc::new(RwLock::new(None)),
            recent_logs: Arc::new(RwLock::new(VecDeque::with_capacity(200))),
            request_id: Arc::new(AtomicU64::new(1)),
        }
    }

    pub async fn get_state(&self) -> AppUiState {
        let snapshot = self.node.state.snapshot().await;
        let local_ticket = self.owner_ticket.read().await.clone();
        let mut projected = project_node_snapshot(&snapshot, local_ticket);
        let local_logs = self.recent_logs.read().await;
        projected.logs.extend(local_logs.iter().cloned());
        projected
    }

    async fn node_snapshot(&self) -> Result<NodeSnapshot> {
        Ok(self.node.state.snapshot().await)
    }

    async fn execute_node_command(
        &self,
        command: NodeCommand,
    ) -> Result<NodeCommandOutcome, AppControllerError> {
        let request_id = self.request_id.fetch_add(1, Ordering::Relaxed);
        let result = self
            .node
            .commands
            .execute(NodeCommandRequest::new(request_id, command))
            .await?;
        match result.outcome {
            NodeCommandOutcome::Rejected(problem) => Err(AppControllerError::from_problem(problem)),
            outcome => Ok(outcome),
        }
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
        self.connect_node(target, routing_mode, exit_node).await
    }

    async fn connect_node(
        &self,
        target: String,
        routing_mode: Option<String>,
        exit_node: Option<String>,
    ) -> Result<()> {
        let target = target.trim().to_string();
        if target.is_empty() {
            return Err(AppControllerError::invalid("连接目标不能为空").into());
        }

        self.log(format!("发起网络连接 -> 目标: {target}")).await;
        // Joining/starting one network is additive. Never stop unrelated
        // networks merely because the user selected another target.
        let previous_snapshot = self.node_snapshot().await?;

        let network_name = if target.starts_with(NetworkTicket::URI_SCHEME) {
            let ticket = NetworkTicket::parse(&target).map_err(|error| {
                AppControllerError::invalid(format!("解析 fk:// 邀请票据失败: {error:#}"))
            })?;
            self.log(format!(
                "[票据] 成功解析网络邀请票据: 网络={}, 端点={}",
                ticket.network_name, ticket.endpoint
            ))
            .await;
            ticket.network_name
        } else {
            "default".to_string()
        };

        self.execute_node_command(NodeCommand::JoinNetwork(NodeJoinNetworkCommand {
            target,
            network_name: Some(network_name.clone()),
            start: true,
        }))
        .await?;

        let snapshot = self.node_snapshot().await?;
        let is_exit_node = snapshot
            .networks
            .iter()
            .find(|network| network.name == network_name)
            .is_some_and(|network| network.roles.contains(&NodeNetworkRole::ExitNode))
            || snapshot
                .configuration
                .networks
                .iter()
                .find(|network| network.network_name == network_name)
                .is_some_and(|network| network.exit_node_enabled);

        let previous_mode = routing_mode
            .is_none()
            .then(|| config_routing_mode(&previous_snapshot, Some(&network_name)))
            .flatten();
        let mode = routing_mode
            .as_deref()
            .map(node_routing_mode)
            .transpose()?
            .or(previous_mode)
            .unwrap_or(NodeRoutingMode::MeshOnly);
        self.execute_node_command(NodeCommand::UpdateRouting(NodeUpdateRoutingCommand {
            network_name,
            mode,
            exit_node,
            is_exit_node,
        }))
        .await?;

        Ok(())
    }

    /// 断开当前网络连接，并等待运行中的客户端确认已经释放资源。
    pub async fn disconnect(&self) -> Result<()> {
        self.log("正在请求节点停止全部网络；控制面将保持运行...")
            .await;
        self.execute_node_command(NodeCommand::StopNetworks {
            networks: Vec::new(),
            all: true,
        })
        .await?;
        *self.owner_ticket.write().await = None;
        Ok(())
    }

    /// 向运行中的 P2P 引擎发起真实探针。
    pub async fn probe_peer(&self, target: String, timeout_ms: u64) -> Result<PeerProbeOutcome> {
        if target.trim().is_empty() {
            return Err(AppControllerError::invalid("探测目标不能为空").into());
        }
        let snapshot = self.node_snapshot().await?;
        let network_name = primary_network_name(&snapshot)
            .ok_or_else(|| AppControllerError::not_found("当前没有可探测的网络"))?;
        let outcome = self
            .execute_node_command(NodeCommand::ProbePeer(NodeProbePeerCommand {
                network_name,
                target,
                timeout_ms,
            }))
            .await?;
        match outcome {
            NodeCommandOutcome::Probe(NodePeerProbeSnapshot {
                peer_id,
                virtual_ipv6,
                endpoint,
                rtt_us,
                p2p_active,
                timed_out,
            }) => Ok(PeerProbeOutcome {
                client_id: peer_id,
                virtual_ip: virtual_ipv6,
                endpoint,
                rtt_us,
                p2p_active,
                timed_out,
            }),
            other => Err(AppControllerError::internal(format!(
                "节点探针返回了意外结果: {other:?}"
            ))
            .into()),
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
            "[启动] 正在通过节点创建网络: {name} (信令={port}, 中继={relay_port})"
        ))
        .await;
        let outcome = self
            .execute_node_command(NodeCommand::CreateNetwork(NodeCreateNetworkCommand {
                name,
                signaling_port: port,
                relay_port,
                external_host,
                auto_connect,
            }))
            .await?;
        let NodeCommandOutcome::NetworkCreated(created) = outcome else {
            return Err(AppControllerError::internal(format!(
                "创建网络返回了意外结果: {outcome:?}"
            ))
            .into());
        };
        *self.owner_ticket.write().await = Some(created.invitation_ticket.clone());
        self.log("[网络] 对等网络已创建成功；邀请票据仅保留在所有者控制面")
            .await;
        Ok(created.invitation_ticket)
    }

    /// 停止自建网络
    pub async fn stop_host(&self) -> Result<()> {
        let snapshot = self.node_snapshot().await?;
        let hosted: Vec<String> = snapshot
            .embedded_services
            .iter()
            .filter(|service| service.status != NodeEmbeddedServiceStatus::Stopped)
            .map(|service| service.network_name.clone())
            .collect();
        self.log("正在停止当前节点托管的网络服务...").await;
        if !hosted.is_empty() {
            self.execute_node_command(NodeCommand::StopNetworks {
                networks: hosted,
                all: false,
            })
            .await?;
        }
        *self.owner_ticket.write().await = None;
        self.log("节点托管的网络服务已停止").await;
        Ok(())
    }

    /// 协调中枢封禁客户端
    pub async fn ban_client(&self, client_id: String) -> Result<()> {
        Err(AppControllerError::unsupported(format!(
            "当前节点命令协议尚未提供封禁操作，无法封禁客户端 {client_id}"
        ))
        .into())
    }

    /// 协调中枢解封客户端
    pub async fn unban_client(&self, client_id: &str) -> Result<bool> {
        Err(AppControllerError::unsupported(format!(
            "当前节点命令协议尚未提供解封操作，无法解封客户端 {client_id}"
        ))
        .into())
    }

    /// 动态热更新路由分流与出口节点配置，并等待运行时确认。
    pub async fn update_routing(
        &self,
        mode: String,
        exit_node: Option<String>,
        is_exit_node: bool,
    ) -> Result<()> {
        let snapshot = self.node_snapshot().await?;
        let network_name = primary_network_name(&snapshot)
            .ok_or_else(|| AppControllerError::not_found("当前没有可更新路由的网络"))?;
        let mode = node_routing_mode(&mode)?;
        self.execute_node_command(NodeCommand::UpdateRouting(NodeUpdateRoutingCommand {
            network_name,
            mode,
            exit_node,
            is_exit_node,
        }))
        .await?;
        self.log("节点路由配置已热更新").await;
        Ok(())
    }

    /// Compatibility hook for adapters that still poll. The shared node state
    /// subscription is push-based, so there is no second source to refresh.
    pub async fn refresh_peers(&self) {}

    /// 获取全网对等节点快照列表 (兼容原有拓扑与节点管理)
    pub async fn get_peers_snapshots(&self) -> Vec<PeerSnapshot> {
        self.node
            .state
            .snapshot()
            .await
            .peers
            .iter()
            .map(node_peer_to_snapshot)
            .collect()
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

    /// 查询当前专网网段与地址池配置。
    pub async fn get_network_config(&self) -> NetworkConfigDto {
        let snapshot = self.node.state.snapshot().await;
        let mode = primary_network_config(&snapshot)
            .and_then(|config| config.virtual_mode)
            .unwrap_or(fuckipv6_common::NetworkMode::DualStack);
        let ipv4_pool = fuckipv6_common::default_ipv4_pool();
        let ipv6_pool = fuckipv6_common::default_ipv6_pool();
        NetworkConfigDto {
            mode,
            ipv4_prefix: cidr_prefix(&ipv4_pool, 24),
            ipv6_prefix: cidr_prefix(&ipv6_pool, 120),
            ipv4_pool,
            ipv6_pool,
            config_file: None,
        }
    }

    /// 热更新专网网段与地址池配置。
    ///
    /// The canonical command protocol does not expose subnet allocation
    /// mutation yet. Reject instead of mutating a second Coordinator state.
    pub async fn update_network_config(
        &self,
        req: UpdateNetworkConfigRequest,
    ) -> Result<NetworkConfigDto> {
        Err(AppControllerError::unsupported(format!(
            "当前节点命令协议尚未提供地址池热更新，未应用请求: mode={:?}, IPv4={}, IPv6={}",
            req.mode, req.ipv4_pool, req.ipv6_pool
        ))
        .into())
    }

    /// 获取黑名单列表。
    ///
    /// Blacklist mutation/query is not yet part of the canonical node command
    /// protocol; return an empty projection rather than reading a second
    /// Coordinator instance.
    pub async fn get_blacklist(&self) -> Vec<String> {
        Vec::new()
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

fn project_node_snapshot(snapshot: &NodeSnapshot, local_ticket: Option<String>) -> AppUiState {
    let primary = primary_network(snapshot);
    let primary_config = primary_network_config(snapshot);
    let coordinator = active_coordinator_service(snapshot);
    let coordinator_active = coordinator.is_some();
    let network_online = snapshot.networks.iter().any(|network| {
        matches!(
            network.status,
            NodeNetworkStatus::Online | NodeNetworkStatus::Degraded
        )
    });
    let network_connecting = snapshot
        .networks
        .iter()
        .any(|network| network.status == NodeNetworkStatus::Connecting);
    let network_degraded = snapshot
        .networks
        .iter()
        .any(|network| network.status == NodeNetworkStatus::Degraded);
    let status = if network_degraded {
        "degraded"
    } else if network_online {
        "online"
    } else if network_connecting {
        "connecting"
    } else if coordinator_active {
        "hosting"
    } else {
        "offline"
    };

    let peers: Vec<PeerUiInfo> = snapshot
        .peers
        .iter()
        .filter(|peer| peer.online)
        .map(|peer| PeerUiInfo {
            client_id: peer.peer_id.clone(),
            virtual_ip: peer.virtual_ipv6.to_string(),
            virtual_ipv4: peer.virtual_ipv4.map(|ip| ip.to_string()),
            direct: peer.p2p_active,
            latency_ms: peer.rtt_us.map(|rtt| rtt as f64 / 1000.0),
            network_name: Some(peer.network_name.clone()),
            networks: vec![peer.network_name.clone()],
        })
        .collect();

    let mut networks: Vec<NetworkUiDto> = snapshot
        .networks
        .iter()
        .map(|network| NetworkUiDto {
            name: network.name.clone(),
            tun_name: network.tun_name.clone(),
            assigned_ip: network.assigned_ipv6.map(|ip| ip.to_string()),
            assigned_ipv4: network.assigned_ipv4.map(|ip| ip.to_string()),
            peer_count: network.peer_count,
            status: network_status_label(network.status).to_string(),
        })
        .collect();

    if networks.is_empty() {
        if let Some(service) = coordinator {
            let name = service.network_name.clone();
            let tun_name = snapshot
                .configuration
                .networks
                .iter()
                .find(|config| config.network_name == name)
                .map(|config| config.tun_name.clone())
                .unwrap_or_else(|| crate::multi_network::derive_tun_name(Some(&name), "fuckipv6"));
            networks.push(NetworkUiDto {
                name,
                tun_name,
                assigned_ip: None,
                assigned_ipv4: None,
                peer_count: 0,
                status: "协调中枢托管中".to_string(),
            });
        }
    }

    let network_name = primary
        .map(|network| network.name.clone())
        .or_else(|| {
            snapshot
                .configuration
                .networks
                .first()
                .map(|config| config.network_name.clone())
        })
        .or_else(|| coordinator.map(|service| service.network_name.clone()))
        .or_else(|| Some("default".to_string()));
    let coordinator_ticket = coordinator_active.then_some(local_ticket).flatten();

    AppUiState {
        status: status.to_string(),
        virtual_ipv6: primary.and_then(|network| network.assigned_ipv6.map(|ip| ip.to_string())),
        virtual_ipv4: primary.and_then(|network| network.assigned_ipv4.map(|ip| ip.to_string())),
        routing_mode: primary_config
            .map(|config| mode_name(config.routing_mode).to_string())
            .unwrap_or_else(|| "MeshOnly".to_string()),
        exit_node: primary_config.and_then(|config| config.exit_node.clone()),
        is_exit_node: primary
            .is_some_and(|network| network.roles.contains(&NodeNetworkRole::ExitNode))
            || primary_config.is_some_and(|config| config.exit_node_enabled),
        connected_target: primary_config.map(|config| config.coordinator_url.clone()),
        hosting_ticket: coordinator_ticket,
        peers,
        logs: snapshot
            .logs
            .iter()
            .map(|entry| {
                format!(
                    "[{}] {}: {}",
                    node_log_level_label(entry.level),
                    entry.target,
                    entry.message
                )
            })
            .collect(),
        network_name,
        networks,
        coordinator_active,
        coordinator_signaling_addr: coordinator
            .and_then(|service| service.signaling_addr)
            .map(|addr| addr.to_string()),
        coordinator_relay_addr: coordinator
            .and_then(|service| service.relay_addr)
            .map(|addr| addr.to_string()),
        coordinator_online_clients: coordinator.map_or(0, |service| {
            snapshot
                .peers
                .iter()
                .filter(|peer| peer.online && peer.network_name == service.network_name)
                .count()
        }),
        // The node snapshot does not currently expose coordinator-only relay
        // counters or blacklist entries. Keep them unknown/empty instead of
        // manufacturing values from unrelated local traffic counters.
        coordinator_traffic: None,
        coordinator_peers: Vec::new(),
        coordinator_blacklist: Vec::new(),
        runtime_available: !snapshot.networks.is_empty(),
        runtime_p2p_addr: primary
            .and_then(|network| network.p2p_listen_addr)
            .map(|addr| addr.to_string()),
        runtime_traffic: primary.map(|network| RuntimeTrafficDto {
            uptime_secs: network.uptime_secs,
            tun_rx_packets: network.traffic.tun_rx_packets,
            tun_rx_bytes: network.traffic.tun_rx_bytes,
            tun_tx_packets: network.traffic.tun_tx_packets,
            tun_tx_bytes: network.traffic.tun_tx_bytes,
            p2p_rx_packets: network.traffic.p2p_rx_packets,
            p2p_rx_bytes: network.traffic.p2p_rx_bytes,
            p2p_tx_packets: network.traffic.p2p_tx_packets,
            p2p_tx_bytes: network.traffic.p2p_tx_bytes,
            ..RuntimeTrafficDto::default()
        }),
    }
}

fn primary_network(snapshot: &NodeSnapshot) -> Option<&crate::node::NodeNetworkSnapshot> {
    snapshot
        .networks
        .iter()
        .max_by_key(|network| match network.status {
            NodeNetworkStatus::Online => 5,
            NodeNetworkStatus::Degraded => 4,
            NodeNetworkStatus::Connecting => 3,
            NodeNetworkStatus::Stopping => 2,
            NodeNetworkStatus::Failed => 1,
            NodeNetworkStatus::Offline => 0,
        })
}

fn primary_network_config(
    snapshot: &NodeSnapshot,
) -> Option<&crate::node::NodeNetworkConfigSnapshot> {
    let primary_name = primary_network(snapshot).map(|network| network.name.as_str());
    primary_name
        .and_then(|name| {
            snapshot
                .configuration
                .networks
                .iter()
                .find(|config| config.network_name == name)
        })
        .or_else(|| snapshot.configuration.networks.first())
}

fn config_routing_mode(
    snapshot: &NodeSnapshot,
    network_name: Option<&str>,
) -> Option<NodeRoutingMode> {
    network_name
        .and_then(|name| {
            snapshot
                .configuration
                .networks
                .iter()
                .find(|config| config.network_name == name)
        })
        .or_else(|| primary_network_config(snapshot))
        .map(|config| config.routing_mode)
}

fn primary_network_name(snapshot: &NodeSnapshot) -> Option<String> {
    primary_network(snapshot)
        .map(|network| network.name.clone())
        .or_else(|| {
            snapshot
                .configuration
                .networks
                .first()
                .map(|config| config.network_name.clone())
        })
}

fn active_coordinator_service(
    snapshot: &NodeSnapshot,
) -> Option<&crate::node::NodeEmbeddedServiceSnapshot> {
    snapshot
        .embedded_services
        .iter()
        .find(|service| service.status == NodeEmbeddedServiceStatus::Running)
        .or_else(|| {
            snapshot.embedded_services.iter().find(|service| {
                matches!(
                    service.status,
                    NodeEmbeddedServiceStatus::Starting | NodeEmbeddedServiceStatus::Stopping
                )
            })
        })
}

fn node_peer_to_snapshot(peer: &crate::node::NodePeerSnapshot) -> PeerSnapshot {
    PeerSnapshot {
        client_id: peer.peer_id.clone(),
        virtual_ip: peer.virtual_ipv6,
        virtual_ipv4: peer.virtual_ipv4,
        endpoints: peer.endpoints.clone(),
        // NAT type and per-peer idle/connected durations are not present in the
        // canonical NodePeerSnapshot. Mark NAT explicitly unknown and keep the
        // unavailable durations at zero rather than inferring them.
        nat_type: "unknown".to_string(),
        connected_secs: 0,
        idle_secs: 0,
        network_name: Some(peer.network_name.clone()),
        networks: vec![peer.network_name.clone()],
    }
}

fn node_routing_mode(mode: &str) -> Result<NodeRoutingMode, AppControllerError> {
    match mode.to_lowercase().as_str() {
        "meshonly" | "mesh-only" | "mesh_only" => Ok(NodeRoutingMode::MeshOnly),
        "bypassmainland" | "bypass-mainland" | "bypass_mainland" => {
            Ok(NodeRoutingMode::BypassMainland)
        }
        "globalproxy" | "global-proxy" | "global_proxy" => Ok(NodeRoutingMode::GlobalProxy),
        other => Err(AppControllerError::invalid(format!(
            "不支持的路由模式: {other}"
        ))),
    }
}

fn mode_name(mode: NodeRoutingMode) -> &'static str {
    match mode {
        NodeRoutingMode::MeshOnly => "MeshOnly",
        NodeRoutingMode::BypassMainland => "BypassMainland",
        NodeRoutingMode::GlobalProxy => "GlobalProxy",
    }
}

fn network_status_label(status: NodeNetworkStatus) -> &'static str {
    match status {
        NodeNetworkStatus::Offline => "离线",
        NodeNetworkStatus::Connecting => "连接中",
        NodeNetworkStatus::Online => "在线",
        NodeNetworkStatus::Degraded => "运行降级",
        NodeNetworkStatus::Stopping => "停止中",
        NodeNetworkStatus::Failed => "启动失败",
    }
}

fn node_log_level_label(level: crate::node::NodeLogLevel) -> &'static str {
    match level {
        crate::node::NodeLogLevel::Trace => "TRACE",
        crate::node::NodeLogLevel::Debug => "DEBUG",
        crate::node::NodeLogLevel::Info => "INFO",
        crate::node::NodeLogLevel::Warn => "WARN",
        crate::node::NodeLogLevel::Error => "ERROR",
    }
}

fn cidr_prefix(cidr: &str, fallback: u8) -> u8 {
    cidr.rsplit_once('/')
        .and_then(|(_, prefix)| prefix.parse().ok())
        .unwrap_or(fallback)
}

#[cfg(test)]
impl Default for AppController {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::node::{
        NodeEmbeddedServiceSnapshot, NodeNetworkConfigSnapshot, NodeNetworkSnapshot,
        NodePeerSnapshot, NodeTrafficSnapshot,
    };
    use std::net::{Ipv4Addr, Ipv6Addr};

    fn node_network_config(name: &str, mode: NodeRoutingMode) -> NodeNetworkConfigSnapshot {
        NodeNetworkConfigSnapshot {
            network_name: name.to_string(),
            coordinator_url: "ws://coordinator.example:6666".to_string(),
            virtual_mode: Some(fuckipv6_common::NetworkMode::DualStack),
            preferred_ipv4: None,
            preferred_ipv6: None,
            virtual_network: Some("fd00::/64".to_string()),
            tun_name: format!("fuckipv6-{name}"),
            mtu: 1420,
            p2p_listen_port: 40000,
            p2p_connect_timeout_secs: 30,
            p2p_heartbeat_interval_secs: 5,
            routing_mode: mode,
            exit_node: Some("peer-exit".to_string()),
            direct_cidrs: vec!["10.0.0.0/8".to_string()],
            proxy_cidrs: vec!["0.0.0.0/0".to_string()],
            exit_node_enabled: true,
            exit_node_allowed_clients: Vec::new(),
            exit_node_allow_lan_access: false,
            exit_node_region: Some("test".to_string()),
            acl_enabled: false,
            acl_default_action: fuckipv6_common::AclAction::Allow,
            acl_rule_count: 0,
            log_level: "info".to_string(),
            log_file_configured: false,
            identity_configured: true,
        }
    }

    fn node_network(name: &str, status: NodeNetworkStatus) -> NodeNetworkSnapshot {
        NodeNetworkSnapshot {
            name: name.to_string(),
            tun_name: format!("fuckipv6-{name}"),
            status,
            assigned_ipv6: Some(Ipv6Addr::new(0xfd00, 0, 0, 0, 0, 0, 0x10, 1)),
            assigned_ipv4: Some(Ipv4Addr::new(10, 8, 0, 1)),
            p2p_listen_addr: Some("127.0.0.1:40000".parse().unwrap()),
            uptime_secs: 90,
            roles: vec![NodeNetworkRole::Member, NodeNetworkRole::ExitNode],
            peer_count: 2,
            traffic: NodeTrafficSnapshot {
                tun_rx_packets: 20,
                tun_rx_bytes: 2_000,
                tun_tx_packets: 10,
                tun_tx_bytes: 1_000,
                p2p_rx_packets: 8,
                p2p_rx_bytes: 800,
                p2p_tx_packets: 6,
                p2p_tx_bytes: 600,
                error_count: 2,
            },
        }
    }

    fn node_peer(name: &str, peer_id: &str, online: bool) -> NodePeerSnapshot {
        NodePeerSnapshot {
            peer_id: peer_id.to_string(),
            network_name: name.to_string(),
            virtual_ipv6: Ipv6Addr::new(0xfd00, 0, 0, 0, 0, 0, 0x10, 2),
            virtual_ipv4: Some(Ipv4Addr::new(10, 8, 0, 2)),
            endpoints: vec!["127.0.0.1:41000".parse().unwrap()],
            online,
            p2p_active: true,
            p2p_healthy: true,
            rtt_us: Some(23_000),
            loss_rate: 0.0,
            traffic: NodeTrafficSnapshot::default(),
        }
    }

    #[test]
    fn node_snapshot_projection_uses_authoritative_network_and_traffic_state() {
        let mut network = node_network("office", NodeNetworkStatus::Online);
        network.traffic.error_count = 3;
        let snapshot = NodeSnapshot {
            configuration: crate::node::NodeConfigurationSnapshot {
                networks: vec![node_network_config(
                    "office",
                    NodeRoutingMode::BypassMainland,
                )],
            },
            networks: vec![network],
            peers: vec![
                node_peer("office", "peer-alpha", true),
                node_peer("office", "peer-offline", false),
            ],
            logs: vec![crate::node::NodeLogEntry {
                unix_ms: 1,
                level: crate::node::NodeLogLevel::Info,
                target: "node".to_string(),
                message: "ready".to_string(),
            }],
            ..NodeSnapshot::default()
        };

        let state = project_node_snapshot(&snapshot, None);

        assert_eq!(state.status, "online");
        assert_eq!(state.virtual_ipv6.as_deref(), Some("fd00::10:1"));
        assert_eq!(state.virtual_ipv4.as_deref(), Some("10.8.0.1"));
        assert_eq!(state.routing_mode, "BypassMainland");
        assert_eq!(state.exit_node.as_deref(), Some("peer-exit"));
        assert!(state.is_exit_node);
        assert_eq!(state.network_name.as_deref(), Some("office"));
        assert_eq!(state.networks.len(), 1);
        assert_eq!(state.networks[0].status, "在线");
        assert_eq!(state.networks[0].peer_count, 2);
        assert_eq!(state.peers.len(), 1);
        assert_eq!(state.peers[0].client_id, "peer-alpha");
        assert_eq!(state.peers[0].latency_ms, Some(23.0));
        assert_eq!(state.logs, vec!["[INFO] node: ready"]);
        let traffic = state.runtime_traffic.expect("runtime traffic");
        assert_eq!(traffic.uptime_secs, 90);
        assert_eq!(traffic.tun_rx_bytes, 2_000);
        assert_eq!(traffic.p2p_tx_bytes, 600);
    }

    #[test]
    fn node_snapshot_projection_distinguishes_degraded_and_connecting() {
        let degraded = NodeSnapshot {
            networks: vec![node_network("office", NodeNetworkStatus::Degraded)],
            ..NodeSnapshot::default()
        };
        let degraded = project_node_snapshot(&degraded, None);
        assert_eq!(degraded.status, "degraded");
        assert_eq!(degraded.networks[0].status, "运行降级");

        let connecting = NodeSnapshot {
            networks: vec![node_network("office", NodeNetworkStatus::Connecting)],
            ..NodeSnapshot::default()
        };
        assert_eq!(
            project_node_snapshot(&connecting, None).status,
            "connecting"
        );
    }

    #[test]
    fn node_snapshot_projection_maps_coordinator_service_and_scopes_clients() {
        let snapshot = NodeSnapshot {
            configuration: crate::node::NodeConfigurationSnapshot {
                networks: vec![node_network_config("office", NodeRoutingMode::MeshOnly)],
            },
            embedded_services: vec![NodeEmbeddedServiceSnapshot {
                network_name: "office".to_string(),
                status: NodeEmbeddedServiceStatus::Running,
                signaling_addr: Some("127.0.0.1:6666".parse().unwrap()),
                relay_addr: Some("127.0.0.1:6667".parse().unwrap()),
            }],
            peers: vec![
                node_peer("office", "peer-office", true),
                node_peer("lab", "peer-lab", true),
            ],
            ..NodeSnapshot::default()
        };

        let state = project_node_snapshot(&snapshot, Some("fk://invite".to_string()));

        assert_eq!(state.status, "hosting");
        assert!(state.coordinator_active);
        assert_eq!(
            state.coordinator_signaling_addr.as_deref(),
            Some("127.0.0.1:6666")
        );
        assert_eq!(
            state.coordinator_relay_addr.as_deref(),
            Some("127.0.0.1:6667")
        );
        assert_eq!(state.coordinator_online_clients, 1);
        assert_eq!(state.hosting_ticket.as_deref(), Some("fk://invite"));
        assert_eq!(state.network_name.as_deref(), Some("office"));
        assert_eq!(state.networks.len(), 1);
        assert_eq!(state.networks[0].status, "协调中枢托管中");
    }

    #[test]
    fn node_peer_projection_marks_unavailable_nat_metadata_unknown() {
        let projected = node_peer_to_snapshot(&node_peer("office", "peer-alpha", true));
        assert_eq!(projected.nat_type, "unknown");
        assert_eq!(projected.connected_secs, 0);
        assert_eq!(projected.idle_secs, 0);
        assert_eq!(projected.network_name.as_deref(), Some("office"));
    }

    #[test]
    fn routing_mode_falls_back_to_matching_or_current_configuration() {
        let snapshot = NodeSnapshot {
            configuration: crate::node::NodeConfigurationSnapshot {
                networks: vec![
                    node_network_config("office", NodeRoutingMode::GlobalProxy),
                    node_network_config("lab", NodeRoutingMode::MeshOnly),
                ],
            },
            ..NodeSnapshot::default()
        };

        assert_eq!(
            config_routing_mode(&snapshot, Some("lab")),
            Some(NodeRoutingMode::MeshOnly)
        );
        assert_eq!(
            config_routing_mode(&snapshot, Some("new-network")),
            Some(NodeRoutingMode::GlobalProxy)
        );
    }
}
