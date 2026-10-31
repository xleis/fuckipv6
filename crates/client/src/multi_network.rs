use anyhow::Result;
use std::sync::Arc;
use tokio::sync::mpsc;

use crate::cli::commands::{self, RuntimeOptions};
use crate::config_manager::ConfigManager;
use crate::ipc::{self, MultiNetworkRegistry};
use crate::node::runtime_metadata;
use crate::node::{
    spawn_command_bus_with_runtime, NetworkRuntimeManager, NodeCommandBus, NodeEventBus,
    NodeEventKind, NodeLifecycle, NodeLifecycleSnapshot, NodeNetworkSelection, NodeNetworkSnapshot,
    NodeNetworkStatus, NodePeerSnapshot, NodeRuntimePlan, NodeStateStore, NodeTrafficSnapshot,
};
use crate::symmetric::EmbeddedServiceManager;
use crate::ui::UiModeResolution;

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
    commands: NodeCommandBus,
    services: EmbeddedServiceManager,
    runtimes: NetworkRuntimeManager,
    ui_mode: UiModeResolution,
}

impl MultiNetworkSupervisor {
    pub fn new(options: RuntimeOptions) -> Result<Self> {
        Self::new_with_config_manager(options, ConfigManager::new()?)
    }

    pub(crate) fn new_with_config_manager(
        options: RuntimeOptions,
        configs: ConfigManager,
    ) -> Result<Self> {
        let ui_mode = crate::ui::resolve_legacy_ui_mode(options.headless, options.gui)?;
        let events = NodeEventBus::default();
        let registry = MultiNetworkRegistry::new();
        let identity = runtime_metadata::identity_snapshot(
            &configs.load_node_identity()?,
            configs.node_display_name()?,
        );
        let state = NodeStateStore::with_events_identity_and_ui(
            events.clone(),
            Some(identity),
            Some(ui_mode),
        );
        let services = EmbeddedServiceManager::spawn(8);
        let lifecycle = NodeLifecycle::new();
        let runtimes = NetworkRuntimeManager::spawn(
            configs,
            options.clone(),
            state.clone(),
            events.clone(),
            lifecycle.clone(),
            registry.clone(),
            services.clone(),
        );
        let commands = spawn_command_bus_with_runtime(
            state.clone(),
            registry.clone(),
            events.clone(),
            services.clone(),
            runtimes.clone(),
        );
        Ok(Self {
            registry,
            options,
            lifecycle,
            state,
            events,
            commands,
            services,
            runtimes,
            ui_mode,
        })
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

    /// Shared command entrypoint for CLI, IPC, WebUI and future GPUI adapters.
    pub fn commands(&self) -> NodeCommandBus {
        self.commands.clone()
    }

    /// Embedded signaling/relay services owned by this node runtime.
    pub fn services(&self) -> EmbeddedServiceManager {
        self.services.clone()
    }

    async fn sync_lifecycle(&self, lifecycle: NodeLifecycleSnapshot) {
        self.state.set_lifecycle(lifecycle).await;
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
        let logs = runtime_metadata::recent_log_entries();
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
                snapshot.logs = logs;
            })
            .await;
    }

    /// Start the node control plane and its selected network sessions.
    ///
    /// An empty plan is valid: the node and its UI remain available so a
    /// network can be created or joined from either interface.
    pub async fn run_plan(&self, plan: NodeRuntimePlan) -> Result<()> {
        let configured_networks = plan.len();

        self.state
            .update(|snapshot| {
                snapshot.permissions = runtime_metadata::trusted_control_plane_permissions();
                snapshot.configuration.networks.clear();
            })
            .await;

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

        // All adapters use the same command bus and runtime registry.
        let _ipc_task =
            ipc::spawn_multi_server_with_commands(self.registry.clone(), self.commands.clone());

        let webui_port = self.options.webui_port;
        if webui_port > 0 {
            let app_ctrl = Arc::new(crate::webui::AppController::new());
            let node_http = crate::node::NodeHttpAdapter::new(
                self.commands.clone(),
                self.state.clone(),
                self.events.clone(),
            );
            if let Err(error) =
                crate::webui::start_app_webui_with_node(webui_port, app_ctrl, node_http).await
            {
                tracing::warn!("启动客户端 WebUI 失败: {error:#}");
            }
        }

        println!("══════════════════════════════════════════════════════════════════════════════");
        println!("🚀 正在初始化节点网络管理器 (共 {configured_networks} 个网络)");
        println!("══════════════════════════════════════════════════════════════════════════════");

        let summary = match self.runtimes.start_plan(plan).await {
            Ok(summary) => summary,
            Err(error) => {
                let lifecycle = self.lifecycle.failed().await;
                self.sync_lifecycle(lifecycle).await;
                let _ = self.runtimes.shutdown().await;
                anyhow::bail!("节点网络运行时启动失败: {error}");
            }
        };

        if configured_networks > 0
            && summary.started.is_empty()
            && summary.already_running.is_empty()
        {
            let lifecycle = self.lifecycle.failed().await;
            self.sync_lifecycle(lifecycle).await;
            let _ = self.runtimes.shutdown().await;
            anyhow::bail!(
                "{}",
                summary.message("所有已选择网络均初始化失败，节点运行时未进入在线状态")
            );
        }

        let active_networks = summary.started.len() + summary.already_running.len();
        let failed_networks = summary
            .failed
            .iter()
            .map(|failure| failure.network_name.clone())
            .collect();
        let lifecycle = self
            .lifecycle
            .running(active_networks, failed_networks)
            .await;
        self.sync_lifecycle(lifecycle).await;
        self.refresh_runtime_state().await;

        if !summary.failed.is_empty() {
            tracing::warn!("{}", summary.message("节点网络部分启动失败"));
        }

        println!("══════════════════════════════════════════════════════════════════════════════");
        if active_networks == 0 {
            println!("✨ 节点控制面已上线，当前尚未加入网络");
            println!("  - 可通过 WebUI 创建或加入网络");
            println!("  - 停止节点: 按 Ctrl+C");
        } else {
            println!("✨ 节点控制面与 {active_networks} 个网络已上线运行！");
            println!("  - 查看状态: fuckipv6 status");
            println!("  - 查看节点: fuckipv6 peers");
            println!("  - 停止网络: fuckipv6 down [名称] 或按 Ctrl+C");
        }
        println!("══════════════════════════════════════════════════════════════════════════════");

        if self.ui_mode.effective.is_web_ui() && webui_port > 0 {
            let _ = crate::ui::run_webui_window(webui_port).await;
        }

        // Ctrl+C/SIGTERM and the legacy IPC shutdown command end the node.
        // Stopping all networks through the typed command deliberately leaves
        // the node control plane available for create/join operations.
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
                    break;
                }
                Some(stop_target) = stop_rx.recv() => {
                    match stop_target {
                        Some(name) => {
                            println!("🛑 收到 IPC 指令: 停止网络 [{name}]");
                            self.mark_network_status(Some(&name), NodeNetworkStatus::Stopping)
                                .await;
                            if let Err(error) = self.runtimes.stop(vec![name], false).await {
                                tracing::warn!("停止网络失败: {error}");
                            }
                            self.refresh_runtime_state().await;
                        }
                        None => {
                            println!("🛑 收到 IPC 指令: 停止节点");
                            break;
                        }
                    }
                }
            }
        }

        self.mark_network_status(None, NodeNetworkStatus::Stopping)
            .await;
        let lifecycle = self.lifecycle.begin_stop().await;
        self.sync_lifecycle(lifecycle).await;

        if let Err(error) = self.runtimes.shutdown().await {
            tracing::warn!("关闭网络运行时失败: {error}");
        }
        // `shutdown` closes the shared service actor. Tolerate an actor that
        // has already closed so node shutdown still completes deterministically.
        let _ = self.services.shutdown().await;

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
    let supervisor = MultiNetworkSupervisor::new(options)?;
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
    let supervisor = MultiNetworkSupervisor::new(options)?;
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
    use crate::node::NodeNetworkRole;

    fn temp_config_dir(name: &str) -> std::path::PathBuf {
        let unique = format!("fuckipv6-supervisor-{}-{}", name, std::process::id());
        std::env::temp_dir().join(unique)
    }

    #[tokio::test]
    async fn zero_network_supervisor_publishes_stable_identity() -> Result<()> {
        let dir = temp_config_dir("identity");
        let _ = std::fs::remove_dir_all(&dir);
        let configs = ConfigManager::new_for_dir(dir.clone())?;
        configs.set_node_display_name(Some("edge-gateway"))?;

        let supervisor = MultiNetworkSupervisor::new_with_config_manager(
            RuntimeOptions::default(),
            configs.clone(),
        )?;
        let snapshot = supervisor.state().snapshot().await;
        let identity = snapshot
            .identity
            .clone()
            .expect("zero-network snapshot must include identity");

        assert!(snapshot.networks.is_empty());
        assert_eq!(identity.display_name.as_deref(), Some("edge-gateway"));
        assert!(!identity.node_id.is_empty());
        assert!(!identity.public_key_fingerprint.is_empty());

        let encoded = serde_json::to_string(&snapshot)?;
        assert!(!encoded.contains("private_key"));
        assert!(!encoded.contains("seed"));
        assert!(!encoded.contains(&dir.to_string_lossy().to_string()));

        supervisor.runtimes.shutdown().await?;
        let _ = supervisor.services.shutdown().await;

        let reopened = ConfigManager::new_for_dir(dir.clone())?;
        let restarted =
            MultiNetworkSupervisor::new_with_config_manager(RuntimeOptions::default(), reopened)?;
        let restarted_identity = restarted
            .state()
            .snapshot()
            .await
            .identity
            .expect("restarted node must retain identity");

        assert_eq!(restarted_identity, identity);
        restarted.runtimes.shutdown().await?;
        let _ = restarted.services.shutdown().await;

        let _ = std::fs::remove_dir_all(dir);
        Ok(())
    }

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
            active_endpoint: Some("[2001:db8::2]:9000".parse().expect("valid endpoint")),
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
            dataplane: ipc::DataplaneMetricsSnapshot::default(),
        };

        update_network_from_runtime(&mut existing, &runtime);

        assert_eq!(existing.status, NodeNetworkStatus::Stopping);
        assert_eq!(existing.tun_name, "fk-home-live");
        assert_eq!(existing.assigned_ipv6, Some(runtime.assigned_ip));
        assert_eq!(existing.p2p_listen_addr, Some(runtime.p2p_listen_addr));
        assert_eq!(existing.uptime_secs, 30);
    }
}
