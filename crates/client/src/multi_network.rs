use anyhow::Result;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, watch, RwLock};

use crate::cli::commands::{self, RuntimeOptions};
use crate::config_manager::ConfigManager;
use crate::ipc::{self, MultiNetworkRegistry, NetworkSessionHandle};
use crate::node::{
    NodeEventBus, NodeEventKind, NodeLifecycle, NodeLifecycleSnapshot, NodeNetworkRole,
    NodeNetworkSelection, NodeNetworkSnapshot, NodeNetworkStatus, NodePeerSnapshot, NodeProblem,
    NodeProblemCode, NodeRuntimePlan, NodeStateStore, NodeTrafficSnapshot,
};

/// 根据网络名称与用户配置生成唯一的 TUN 虚拟网卡接口名称
/// Linux IFNAMSIZ 限制为 16 字节（包含空终止符 \0），最大有效长度为 15 字符。
pub fn derive_tun_name(network_name: Option<&str>, configured_name: &str) -> String {
    // 若用户显式配置了非默认的网卡名称，则优先使用用户自定义名称
    if !configured_name.is_empty() && configured_name != "fuckipv6" {
        return sanitize_interface_name(configured_name);
    }

    match network_name {
        Some(name) if !name.is_empty() && name != "default" => {
            let mut sanitized = String::new();
            for c in name.chars() {
                if c.is_ascii_alphanumeric() {
                    sanitized.push(c.to_ascii_lowercase());
                } else if c == '-' || c == '_' || c == '.' {
                    sanitized.push('-');
                }
            }
            let trimmed = sanitized.trim_matches('-');
            if trimmed.is_empty() {
                "fuckipv6".to_string()
            } else {
                // "fk-" 占用 3 字符，最多保留 12 字符，确保总长 <= 15
                let suffix: String = trimmed.chars().take(12).collect();
                format!("fk-{suffix}")
            }
        }
        _ => "fuckipv6".to_string(),
    }
}

/// 净化网卡名称，确保不包含非法字符且长度不超过 15
pub fn sanitize_interface_name(name: &str) -> String {
    let mut sanitized = String::new();
    for c in name.chars() {
        if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
            sanitized.push(c);
        }
    }
    let truncated: String = sanitized.chars().take(15).collect();
    if truncated.is_empty() {
        "fuckipv6".to_string()
    } else {
        truncated
    }
}

/// 多网络并发管理器
pub struct MultiNetworkSupervisor {
    registry: MultiNetworkRegistry,
    options: RuntimeOptions,
    lifecycle: NodeLifecycle,
    state: NodeStateStore,
    events: NodeEventBus,
}

impl MultiNetworkSupervisor {
    pub fn new(options: RuntimeOptions) -> Self {
        let events = NodeEventBus::default();
        Self {
            registry: MultiNetworkRegistry::new(),
            options,
            lifecycle: NodeLifecycle::new(),
            state: NodeStateStore::with_events(events.clone()),
            events,
        }
    }

    pub fn registry(&self) -> &MultiNetworkRegistry {
        &self.registry
    }

    /// Shared node lifecycle consumed by every UI and control surface.
    pub fn lifecycle(&self) -> &NodeLifecycle {
        &self.lifecycle
    }

    /// Canonical state store shared by all node UI adapters.
    pub fn state(&self) -> &NodeStateStore {
        &self.state
    }

    /// Shared event bus for state, network and command lifecycle notifications.
    pub fn event_bus(&self) -> NodeEventBus {
        self.events.clone()
    }

    async fn sync_lifecycle(&self, lifecycle: NodeLifecycleSnapshot) {
        self.state.set_lifecycle(lifecycle).await;
    }

    async fn record_network(&self, network: NodeNetworkSnapshot) {
        let network_name = network.name.clone();
        let status = network.status;
        self.state
            .update(|snapshot| {
                if let Some(existing) = snapshot
                    .networks
                    .iter_mut()
                    .find(|existing| existing.name == network.name)
                {
                    *existing = network;
                } else {
                    snapshot.networks.push(network);
                }
            })
            .await;
        self.events.publish(NodeEventKind::NetworkChanged {
            network_name,
            status,
        });
    }

    async fn mark_network_status(&self, network_name: Option<&str>, status: NodeNetworkStatus) {
        let snapshot = self
            .state
            .update(|snapshot| {
                for network in &mut snapshot.networks {
                    if network_name.is_none_or(|name| network.name == name) {
                        network.status = status;
                    }
                }
            })
            .await;
        for network in snapshot.networks.iter().filter(|network| {
            network_name.is_none_or(|name| network.name == name) && network.status == status
        }) {
            self.events.publish(NodeEventKind::NetworkChanged {
                network_name: network.name.clone(),
                status,
            });
        }
    }

    async fn refresh_runtime_state(&self) {
        let runtime = ipc::multi_status_snapshot(&self.registry).await;
        self.state
            .update(|snapshot| {
                for network in &runtime.networks {
                    if let Some(existing) = snapshot
                        .networks
                        .iter_mut()
                        .find(|existing| existing.name == network.network_name)
                    {
                        update_network_from_runtime(existing, network);
                    }
                }

                // An empty runtime means all peers are gone. Replacing the
                // collection also removes peers from networks stopped earlier.
                snapshot.peers = runtime.peers.iter().map(node_peer_snapshot).collect();
            })
            .await;
    }

    /// Start the node control plane and its selected network sessions.
    ///
    /// An empty plan is valid: the node and its UI remain available so a
    /// network can be created or joined from either interface.
    pub async fn run_plan(&self, plan: NodeRuntimePlan) -> Result<()> {
        let configured_networks = plan.len();
        let configs: Vec<_> = plan
            .into_networks()
            .into_iter()
            .map(|network| {
                (
                    network.name,
                    network.config_path.to_string_lossy().into_owned(),
                    network.config,
                )
            })
            .collect();

        let lifecycle = self
            .lifecycle
            .begin_start(configured_networks, Vec::new())
            .await;
        self.sync_lifecycle(lifecycle).await;
        tracing::info!(
            "正在启动节点运行时，已选择 {} 个网络...",
            configured_networks
        );

        let (stop_tx, mut stop_rx) = mpsc::unbounded_channel::<Option<String>>();
        self.registry.set_stop_sender(stop_tx);

        // 启动全局聚合 IPC 控制接口
        let _ipc_task = ipc::spawn_multi_server(self.registry.clone());

        // 启动 WebUI (如果指定端口且大于0)
        let webui_port = self.options.webui_port;
        let ui_mode = crate::ui::detect_ui_mode(self.options.headless, self.options.gui);
        if webui_port > 0 {
            let app_ctrl = Arc::new(crate::webui::AppController::new());
            if let Err(e) = crate::webui::start_app_webui(webui_port, app_ctrl.clone()).await {
                tracing::warn!("启动客户端 WebUI 失败: {e:#}");
            }
        }

        // 统一优雅停止广播通道
        let (global_shutdown_tx, _) = watch::channel(false);
        let network_shutdown_senders =
            Arc::new(RwLock::new(HashMap::<String, watch::Sender<bool>>::new()));

        let mut handles = Vec::new();
        let mut failed_networks = Vec::new();
        let mut started_networks = 0usize;

        println!("══════════════════════════════════════════════════════════════════════════════");
        println!("🚀 正在初始化节点网络管理器 (共 {configured_networks} 个网络)");
        println!("══════════════════════════════════════════════════════════════════════════════");

        for (net_name, config_path, mut client_config) in configs {
            println!("  正在建立网络连接: {} ...", net_name);

            self.record_network(NodeNetworkSnapshot {
                name: net_name.clone(),
                tun_name: derive_tun_name(Some(&net_name), &client_config.tun.name),
                status: NodeNetworkStatus::Connecting,
                assigned_ipv6: None,
                assigned_ipv4: None,
                p2p_listen_addr: None,
                uptime_secs: 0,
                roles: vec![NodeNetworkRole::Member],
                peer_count: 0,
                traffic: NodeTrafficSnapshot::default(),
            })
            .await;

            // 应用命令行分流模式与出口节点参数覆盖
            if let Some(mode_str) = &self.options.routing_mode {
                match mode_str.to_lowercase().as_str() {
                    "mesh-only" | "meshonly" | "mesh_only" => {
                        client_config.routing.mode = fuckipv6_common::RoutingMode::MeshOnly;
                    }
                    "bypass-mainland" | "bypassmainland" | "bypass_mainland" => {
                        client_config.routing.mode = fuckipv6_common::RoutingMode::BypassMainland;
                    }
                    "global-proxy" | "globalproxy" | "global_proxy" => {
                        client_config.routing.mode = fuckipv6_common::RoutingMode::GlobalProxy;
                    }
                    _ => {}
                }
            }
            if let Some(exit) = &self.options.exit_node {
                client_config.routing.exit_node = Some(exit.clone());
            }

            // 派生独立 TUN 设备名称并准备运行时
            let mut runtime = match commands::prepare_client_runtime_ext(
                &config_path,
                client_config,
                Some(&net_name),
                false, // IPC 由 Supervisor 统一托管
            )
            .await
            {
                Ok(r) => r,
                Err(e) => {
                    tracing::error!("网络 [{}] 初始化失败: {:#}", net_name, e);
                    println!("  ❌ 网络 [{}] 初始化失败: {:#}", net_name, e);
                    self.state
                        .update(|snapshot| {
                            snapshot.last_error = Some(
                                NodeProblem::new(
                                    NodeProblemCode::Unavailable,
                                    format!("网络 [{net_name}] 初始化失败: {e:#}"),
                                )
                                .for_network(net_name.clone())
                                .retryable(true),
                            );
                        })
                        .await;
                    self.mark_network_status(Some(&net_name), NodeNetworkStatus::Failed)
                        .await;
                    failed_networks.push(net_name);
                    continue;
                }
            };

            let tun_device_name = runtime.tun_name().to_string();
            let assigned_ip = runtime.assigned_ip;
            let assigned_ipv4 = runtime.assigned_ips.ipv4;
            let p2p = runtime.p2p.clone();
            let p2p_listen_addr = p2p.local_addr();
            let stats = runtime.stats.clone();

            println!(
                "  ✓ [{}] 成功就绪 | TUN: {} | 虚拟 IPv6: {} | P2P 端口: {}",
                net_name,
                tun_device_name,
                assigned_ip,
                p2p.local_addr().port()
            );

            // 注册到多网络状态管理器
            let session_handle = NetworkSessionHandle {
                network_name: net_name.clone(),
                tun_name: tun_device_name.clone(),
                assigned_ip,
                assigned_ipv4,
                p2p,
                stats,
            };
            self.registry.register(session_handle).await;

            self.record_network(NodeNetworkSnapshot {
                name: net_name.clone(),
                tun_name: tun_device_name.clone(),
                status: NodeNetworkStatus::Online,
                assigned_ipv6: Some(assigned_ip),
                assigned_ipv4,
                p2p_listen_addr: Some(p2p_listen_addr),
                uptime_secs: 0,
                roles: vec![NodeNetworkRole::Member],
                peer_count: 0,
                traffic: NodeTrafficSnapshot::default(),
            })
            .await;

            // 为该网络配置独立的关闭信号监听
            let (net_shutdown_tx, net_shutdown_rx) = watch::channel(false);
            network_shutdown_senders
                .write()
                .await
                .insert(net_name.clone(), net_shutdown_tx);

            let reg = self.registry.clone();
            let name_for_task = net_name.clone();
            let state_for_task = self.state.clone();
            let events_for_task = self.events.clone();

            let handle = tokio::spawn(async move {
                tracing::info!("网络 [{name_for_task}] 正在启动事件循环");
                let mut rx = net_shutdown_rx;
                if let Err(e) = runtime.run_with_shutdown(&mut rx).await {
                    tracing::error!("网络 [{name_for_task}] 事件循环异常退出: {e:#}");
                }
                tracing::info!("网络 [{name_for_task}] 正在清理资源");
                runtime.shutdown().await;
                reg.unregister(&name_for_task).await;
                state_for_task
                    .update(|snapshot| {
                        if let Some(network) = snapshot
                            .networks
                            .iter_mut()
                            .find(|network| network.name == name_for_task)
                        {
                            network.status = NodeNetworkStatus::Offline;
                            network.peer_count = 0;
                        }
                    })
                    .await;
                events_for_task.publish(NodeEventKind::NetworkChanged {
                    network_name: name_for_task.clone(),
                    status: NodeNetworkStatus::Offline,
                });
                tracing::info!("网络 [{name_for_task}] 已安全下线");
            });

            handles.push(handle);
            started_networks += 1;
        }

        if configured_networks > 0 && started_networks == 0 {
            let lifecycle = self.lifecycle.failed().await;
            self.sync_lifecycle(lifecycle).await;
            anyhow::bail!("所有已选择网络均初始化失败，节点运行时未进入在线状态");
        }

        let lifecycle = self
            .lifecycle
            .running(started_networks, failed_networks)
            .await;
        self.sync_lifecycle(lifecycle).await;

        println!("══════════════════════════════════════════════════════════════════════════════");
        if started_networks == 0 {
            println!("✨ 节点控制面已上线，当前尚未加入网络");
            println!("  - 可通过 WebUI 创建或加入网络");
            println!("  - 停止节点: 按 Ctrl+C");
        } else {
            println!("✨ 节点控制面与 {started_networks} 个网络已上线运行！");
            println!("  - 查看状态: fuckipv6 status");
            println!("  - 查看节点: fuckipv6 peers");
            println!("  - 停止网络: fuckipv6 down [名称] 或按 Ctrl+C");
        }
        println!("══════════════════════════════════════════════════════════════════════════════");

        if ui_mode.is_gui() && webui_port > 0 {
            let _ = crate::ui::run_desktop_gui(webui_port).await;
        }

        // 监听外部信号 (Ctrl+C, SIGTERM, 或通过 IPC 下发的 StopNetwork)
        let shutdown_sig = commands::shutdown_signal();
        tokio::pin!(shutdown_sig);
        let mut state_refresh = tokio::time::interval(std::time::Duration::from_secs(1));
        state_refresh.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        state_refresh.tick().await;

        loop {
            tokio::select! {
                _ = state_refresh.tick() => {
                    self.refresh_runtime_state().await;
                }
                _ = &mut shutdown_sig => {
                    println!("\n🛑 收到中断信号，正在优雅关闭节点网络...");
                    self.mark_network_status(None, NodeNetworkStatus::Stopping)
                        .await;
                    let lifecycle = self.lifecycle.begin_stop().await;
                    self.sync_lifecycle(lifecycle).await;
                    let _ = global_shutdown_tx.send(true);
                    let senders = network_shutdown_senders.read().await;
                    for tx in senders.values() {
                        let _ = tx.send(true);
                    }
                    break;
                }
                Some(stop_target) = stop_rx.recv() => {
                    match stop_target {
                        Some(ref name) => {
                            println!("🛑 收到 IPC 指令: 停止网络 [{name}]");
                            self.mark_network_status(Some(name), NodeNetworkStatus::Stopping)
                                .await;
                            let senders = network_shutdown_senders.read().await;
                            if let Some(tx) = senders.get(name) {
                                let _ = tx.send(true);
                            }
                            if self.registry.is_empty().await {
                                println!("所有网络均已停止，主进程退出");
                                let lifecycle = self.lifecycle.begin_stop().await;
                                self.sync_lifecycle(lifecycle).await;
                                break;
                            }
                        }
                        None => {
                            println!("🛑 收到 IPC 指令: 停止所有网络");
                            self.mark_network_status(None, NodeNetworkStatus::Stopping)
                                .await;
                            let lifecycle = self.lifecycle.begin_stop().await;
                            self.sync_lifecycle(lifecycle).await;
                            let _ = global_shutdown_tx.send(true);
                            let senders = network_shutdown_senders.read().await;
                            for tx in senders.values() {
                                let _ = tx.send(true);
                            }
                            break;
                        }
                    }
                }
            }
        }

        // 等待所有网络运行时完成释放和删除 TUN
        for h in handles {
            let _ = h.await;
        }

        let lifecycle = self.lifecycle.stopped().await;
        self.sync_lifecycle(lifecycle).await;
        println!("👋 节点网络运行时已安全退出");
        Ok(())
    }
}

/// 启动指定的多个网络
pub async fn start_multiple_networks(
    network_names: &[String],
    options: RuntimeOptions,
) -> Result<()> {
    let config_manager = ConfigManager::new()?;
    let plan = NodeRuntimePlan::load(
        &config_manager,
        NodeNetworkSelection::Named(network_names.to_vec()),
    )?;
    let supervisor = MultiNetworkSupervisor::new(options);
    supervisor.run_plan(plan).await
}

/// 启动所有已保存的网络
pub async fn start_all_networks(options: RuntimeOptions) -> Result<()> {
    let config_manager = ConfigManager::new()?;
    let plan = NodeRuntimePlan::load(&config_manager, NodeNetworkSelection::All)?;
    if plan.is_empty() {
        anyhow::bail!(
            "当前没有已保存的网络配置。请使用 'fuckipv6 join' 或 'fuckipv6 host' 添加网络。"
        );
    }

    let names: Vec<_> = plan
        .networks()
        .iter()
        .map(|network| network.name.as_str())
        .collect();
    println!("🚀 正在并发启动全部已保存的虚拟网络: {}", names.join(", "));
    let supervisor = MultiNetworkSupervisor::new(options);
    supervisor.run_plan(plan).await
}

/// 停止一个或全部运行中的网络
pub async fn stop_networks(network_names: &[String], all: bool) -> Result<()> {
    if all || network_names.is_empty() {
        println!("正在向后台客户端下发停止全部虚拟网络指令...");
        match ipc::request(ipc::IpcRequest::StopNetwork { name: None }).await {
            Ok(ipc::IpcResponse::Success { message }) => {
                println!("✓ {message}");
            }
            Ok(ipc::IpcResponse::Error { message }) => {
                println!("⚠️  {message}");
            }
            Err(e) => {
                println!("提示: 本地未连接到运行中的 fuckipv6 后台进程 ({e:#})");
            }
            _ => {}
        }
    } else {
        for name in network_names {
            println!("正在向后台客户端下发停止网络 [{name}] 指令...");
            match ipc::request(ipc::IpcRequest::StopNetwork {
                name: Some(name.clone()),
            })
            .await
            {
                Ok(ipc::IpcResponse::Success { message }) => {
                    println!("✓ {message}");
                }
                Ok(ipc::IpcResponse::Error { message }) => {
                    println!("⚠️  {message}");
                }
                Err(e) => {
                    println!("提示: 本地未连接到运行中的 fuckipv6 后台进程 ({e:#})");
                }
                _ => {}
            }
        }
    }

    Ok(())
}

/// Refresh telemetry from a live session without owning lifecycle transitions.
/// Status changes are published by the supervisor/network task so a runtime
/// snapshot racing with shutdown cannot resurrect `Stopping` as `Online`.
fn update_network_from_runtime(
    existing: &mut NodeNetworkSnapshot,
    network: &ipc::NetworkSessionSnapshot,
) {
    existing.tun_name = network.tun_name.clone();
    existing.assigned_ipv6 = Some(network.assigned_ip);
    existing.assigned_ipv4 = network.assigned_ipv4;
    existing.p2p_listen_addr = Some(network.p2p_listen_addr);
    existing.uptime_secs = network.uptime_secs;
    existing.peer_count = network.peers.len();
    existing.traffic = node_traffic_snapshot(&network.global);
}

fn node_traffic_snapshot(global: &ipc::GlobalStatsSnapshot) -> NodeTrafficSnapshot {
    NodeTrafficSnapshot {
        tun_rx_packets: global.tun_rx_packets,
        tun_rx_bytes: global.tun_rx_bytes,
        tun_tx_packets: global.tun_tx_packets,
        tun_tx_bytes: global.tun_tx_bytes,
        p2p_rx_packets: global.p2p_rx_packets,
        p2p_rx_bytes: global.p2p_rx_bytes,
        p2p_tx_packets: global.p2p_tx_packets,
        p2p_tx_bytes: global.p2p_tx_bytes,
        error_count: global.tun_read_errors
            + global.tun_write_errors
            + global.p2p_send_errors
            + global.p2p_recv_errors,
    }
}

fn node_peer_snapshot(peer: &ipc::PeerSnapshot) -> NodePeerSnapshot {
    NodePeerSnapshot {
        peer_id: peer.client_id.clone(),
        network_name: peer
            .network_name
            .clone()
            .unwrap_or_else(|| "default".to_string()),
        virtual_ipv6: peer.virtual_ip,
        virtual_ipv4: peer.virtual_ipv4,
        endpoints: peer.endpoints.clone(),
        online: true,
        p2p_active: peer.p2p_active,
        p2p_healthy: peer.p2p_healthy,
        rtt_us: peer.rtt_us,
        loss_rate: peer.loss_rate,
        traffic: NodeTrafficSnapshot {
            p2p_rx_packets: peer.rx_packets,
            p2p_rx_bytes: peer.rx_bytes,
            p2p_tx_packets: peer.tx_packets,
            p2p_tx_bytes: peer.tx_bytes,
            ..NodeTrafficSnapshot::default()
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_derive_tun_name_custom_override() {
        assert_eq!(derive_tun_name(Some("office"), "myvpn0"), "myvpn0");
        assert_eq!(derive_tun_name(None, "myvpn0"), "myvpn0");
    }

    #[test]
    fn test_derive_tun_name_default_network() {
        assert_eq!(derive_tun_name(Some("default"), "fuckipv6"), "fuckipv6");
        assert_eq!(derive_tun_name(None, "fuckipv6"), "fuckipv6");
        assert_eq!(derive_tun_name(Some(""), "fuckipv6"), "fuckipv6");
    }

    #[test]
    fn test_derive_tun_name_distinct_networks() {
        assert_eq!(derive_tun_name(Some("home"), "fuckipv6"), "fk-home");
        assert_eq!(derive_tun_name(Some("office"), "fuckipv6"), "fk-office");
        assert_eq!(
            derive_tun_name(Some("cloud-vpn"), "fuckipv6"),
            "fk-cloud-vpn"
        );
    }

    #[test]
    fn test_derive_tun_name_truncates_to_ifnamsiz() {
        // IFNAMSIZ is 16, max 15 chars string. "fk-" (3) + 12 chars = 15 chars.
        let name = derive_tun_name(Some("verylongnetworknamethatexceeds15chars"), "fuckipv6");
        assert!(name.len() <= 15);
        assert_eq!(name, "fk-verylongnetw");
    }

    #[test]
    fn test_sanitize_interface_name() {
        assert_eq!(sanitize_interface_name("eth0_test"), "eth0_test");
        assert_eq!(sanitize_interface_name("fk-home.1"), "fk-home1");
        assert_eq!(sanitize_interface_name(""), "fuckipv6");
    }

    fn sample_global_stats() -> ipc::GlobalStatsSnapshot {
        ipc::GlobalStatsSnapshot {
            tun_rx_packets: 1,
            tun_rx_bytes: 2,
            tun_tx_packets: 3,
            tun_tx_bytes: 4,
            p2p_rx_packets: 5,
            p2p_rx_bytes: 6,
            p2p_tx_packets: 7,
            p2p_tx_bytes: 8,
            tun_read_errors: 9,
            tun_write_errors: 10,
            p2p_send_errors: 11,
            p2p_recv_errors: 12,
            signaling_messages_sent: 13,
            signaling_messages_received: 14,
            signaling_reconnects: 15,
        }
    }

    #[test]
    fn node_traffic_snapshot_maps_global_counters_and_aggregate_errors() {
        let traffic = node_traffic_snapshot(&sample_global_stats());

        assert_eq!(traffic.tun_rx_packets, 1);
        assert_eq!(traffic.tun_rx_bytes, 2);
        assert_eq!(traffic.tun_tx_packets, 3);
        assert_eq!(traffic.tun_tx_bytes, 4);
        assert_eq!(traffic.p2p_rx_packets, 5);
        assert_eq!(traffic.p2p_rx_bytes, 6);
        assert_eq!(traffic.p2p_tx_packets, 7);
        assert_eq!(traffic.p2p_tx_bytes, 8);
        assert_eq!(traffic.error_count, 42);
    }

    #[test]
    fn node_peer_snapshot_maps_identity_network_quality_and_traffic() {
        let peer = ipc::PeerSnapshot {
            client_id: "peer-1".into(),
            virtual_ip: "fd00::2".parse().expect("valid IPv6"),
            virtual_ipv4: Some("10.0.0.2".parse().expect("valid IPv4")),
            endpoints: vec!["[2001:db8::2]:9000".parse().expect("valid endpoint")],
            nat_type: "port_restricted".into(),
            p2p_active: true,
            p2p_healthy: true,
            tx_packets: 10,
            tx_bytes: 1000,
            rx_packets: 20,
            rx_bytes: 2000,
            rtt_us: Some(12_000),
            loss_rate: 0.25,
            bandwidth_bps: 1_000_000,
            health_probes_sent: 10,
            health_probes_acked: 9,
            health_window_probes: 10,
            health_window_lost: 1,
            probe_success_rate: 0.9,
            idle_secs: Some(0),
            last_p2p_success_secs: Some(1),
            remote_rtt_us: Some(15_000),
            remote_loss_rate: Some(0.5),
            remote_bandwidth_bps: Some(900_000),
            remote_quality_age_secs: Some(2),
            network_name: Some("home".into()),
        };

        let snapshot = node_peer_snapshot(&peer);

        assert_eq!(snapshot.peer_id, "peer-1");
        assert_eq!(snapshot.network_name, "home");
        assert_eq!(snapshot.virtual_ipv6, peer.virtual_ip);
        assert_eq!(snapshot.virtual_ipv4, peer.virtual_ipv4);
        assert_eq!(snapshot.endpoints, peer.endpoints);
        assert!(snapshot.online);
        assert!(snapshot.p2p_active);
        assert!(snapshot.p2p_healthy);
        assert_eq!(snapshot.rtt_us, Some(12_000));
        assert_eq!(snapshot.loss_rate, 0.25);
        assert_eq!(snapshot.traffic.p2p_rx_packets, 20);
        assert_eq!(snapshot.traffic.p2p_rx_bytes, 2000);
        assert_eq!(snapshot.traffic.p2p_tx_packets, 10);
        assert_eq!(snapshot.traffic.p2p_tx_bytes, 1000);
    }

    #[test]
    fn runtime_refresh_does_not_overwrite_stopping_status() {
        let mut existing = NodeNetworkSnapshot {
            name: "home".into(),
            tun_name: "fk-home".into(),
            status: NodeNetworkStatus::Stopping,
            assigned_ipv6: None,
            assigned_ipv4: None,
            p2p_listen_addr: None,
            uptime_secs: 0,
            roles: vec![NodeNetworkRole::Member],
            peer_count: 0,
            traffic: NodeTrafficSnapshot::default(),
        };
        let runtime = ipc::NetworkSessionSnapshot {
            network_name: "home".into(),
            tun_name: "fk-home-live".into(),
            assigned_ip: "fd00::2".parse().expect("valid IPv6"),
            assigned_ipv4: Some("10.0.0.2".parse().expect("valid IPv4")),
            p2p_listen_addr: "[::]:9000".parse().expect("valid socket"),
            uptime_secs: 30,
            peers: Vec::new(),
            global: sample_global_stats(),
        };

        update_network_from_runtime(&mut existing, &runtime);

        assert_eq!(existing.status, NodeNetworkStatus::Stopping);
        assert_eq!(existing.tun_name, "fk-home-live");
        assert_eq!(existing.assigned_ipv6, Some(runtime.assigned_ip));
        assert_eq!(existing.p2p_listen_addr, Some(runtime.p2p_listen_addr));
        assert_eq!(existing.uptime_secs, 30);
    }
}
