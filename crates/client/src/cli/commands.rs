use anyhow::{Context, Result};
use bytes::Bytes;
use fuckipv6_common::domain::{
    evaluate_effective_access, select_relay_candidate, AccessContext, AuditEntry, AuditId,
    AuditOutcome, CapabilityKind, ExitSessionAction, InvitationCode, InvitationRedeemRequest,
    MembershipRole, MembershipState, NetworkId, NetworkMemberDirectory, NetworkRole, NodeId,
    PermissionIntent, PermissionRequest, PolicyAction, RelayCandidate, RelayGovernanceAction,
    RelayHealthStatus, RelayLimitPolicy, RelayMemberGrant, RelaySelectionPolicy, ResourceKind,
    ResourceRef, TrafficFailureKind, TrafficProtectionState, TrafficSessionSnapshot,
};
#[cfg(test)]
use fuckipv6_common::is_encrypted_config_path;
use fuckipv6_common::recent_error_log_lines;
use fuckipv6_common::{
    decrypt_config_file, encrypt_config_file, ClientConfig, Identity, NatType, PeerInfo,
    RelayEndpoint, RoutingAction, SignalingMessage, SmartRouteEngine, VirtualIps,
};
use ipnet::{IpNet, Ipv4Net, Ipv6Net};
use std::collections::HashMap;
use std::future::Future;
#[cfg(target_os = "linux")]
use std::net::SocketAddr;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
#[cfg(target_os = "linux")]
use std::path::Path;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::{mpsc, watch, RwLock};
use tokio::task::JoinHandle;
use tokio::time::{sleep, Duration, Instant};

use crate::ipc::{self, IpcRequest, IpcResponse};
use crate::local_ipc::IpcRole;
use crate::node::{
    LocalExitCapabilitySnapshot, LocalExitCapabilityStage, NodeCommand, NodeCommandContext,
    NodeCommandOutcome, NodeCommandRequest, NodeCreateInvitationCommand, NodeNetworkSnapshot,
    NodeNetworkStatus, NodePreviewInvitationCommand, NodeProbePeerCommand, NodeProblem,
    NodeProblemCode, NodeRedeemInvitationCommand, NodeSetLocalExitCapabilityCommand, NodeSnapshot,
    NodeStateStore, RuntimeControlError,
};
use crate::owner_security::audit::OwnerSecurityAudit;
use crate::p2p::buffer_pool::InboundPayload;
use crate::p2p::connection::{InPlaceSendOutcome, InboundPacket, P2pEngine, PeerRouteInfo};
use crate::p2p::punching;
use crate::p2p::quic;
use crate::p2p::relay::QuicRelayClient;
use crate::remote_node::RemoteNodeSession;
#[cfg(target_os = "linux")]
use crate::routing::exit_readiness::encode_exit_readiness_frame;
use crate::routing::exit_readiness::{
    decode_exit_readiness_frame, is_exit_readiness_frame, ExitReadinessFrame,
};
#[cfg(target_os = "linux")]
use crate::routing::linux_exit::LinuxExitForwarder;
#[cfg(target_os = "linux")]
use crate::routing::linux_peer_route::LinuxPeerRouteManager;
#[cfg(target_os = "linux")]
use crate::routing::linux_recovery::{
    probe_exit_readiness_target, LinuxRecoveryAdapter, LinuxRecoveryTun, RecoveryAuditContext,
};
#[cfg(target_os = "linux")]
use crate::routing::linux_recovery_events::{LinuxRecoveryEvent, LinuxRecoveryEventWatcher};
#[cfg(target_os = "linux")]
use crate::routing::linux_route::LinuxRouteManager;
use crate::routing::recovery::RecoveryTrigger;
#[cfg(target_os = "linux")]
use crate::routing::recovery::{
    default_recovery_probes, PathRecoveryState, RecoveryCoordinator, RecoveryExpectations,
    RecoveryProbeTarget, RecoveryReport, RecoveryRequest, RecoveryStatus,
};
#[cfg(target_os = "linux")]
use crate::routing::table::{parse_linux_ip_routes, RouteEntry};
use crate::routing::{
    ExitContributionGate, ExitContributionNetworkContext, ExitNodeManager, ExitSelectionGate,
};
use crate::signaling::client::{SignalingClient, SignalingEvent};
use crate::stats::StatsCollector;
use crate::tun::TunDevice;
#[cfg(target_os = "linux")]
use fuckipv6_common::domain::{TrafficAction, TrafficFailurePolicy, TrafficFailureRouteContext};
use fuckipv6_secure_session::{AEAD_TAG_LEN, DATA_HEADER_LEN};

const TUN_PACKET_BUFFER_SIZE: usize = 65536;
const TUN_BUFFER_ALLOCATION_SIZE: usize = TUN_PACKET_BUFFER_SIZE + DATA_HEADER_LEN + AEAD_TAG_LEN;
const TUN_BUFFER_POOL_SIZE: usize = 64;
const DATA_PLANE_BATCH_SIZE: usize = 32;
const QUIC_RELAY_SEND_TIMEOUT: Duration = Duration::from_secs(2);
const RELAY_CLOSE_TIMEOUT: Duration = Duration::from_secs(1);
const OUTBOUND_CLOSE_TIMEOUT: Duration = Duration::from_secs(2);
const TASK_STOP_TIMEOUT: Duration = Duration::from_secs(2);
const RUNTIME_CLEANUP_TIMEOUT: Duration = Duration::from_secs(12);
const OUTBOUND_QUEUE_CAPACITY: usize = 256;
const RECOVERY_TRIGGER_QUEUE_CAPACITY: usize = 8;
const EXIT_CONTRIBUTION_MEMBER_REFRESH_INTERVAL: Duration = Duration::from_secs(30);
const EXIT_CONTRIBUTION_DIRECTORY_QUEUE_CAPACITY: usize = 4;
const EXIT_AVAILABILITY_REPORT_INTERVAL: Duration = Duration::from_secs(30);
const RELAY_MEMBER_GRANT_TTL: Duration = Duration::from_secs(15 * 60);
const RELAY_MEMBER_GRANT_RENEW_AHEAD: Duration = Duration::from_secs(5 * 60);
const RELAY_AUTHORIZATION_UPDATE_TIMEOUT: Duration = Duration::from_secs(5);
const RELAY_CONNECT_ATTEMPT_TIMEOUT: Duration = Duration::from_secs(3);
const RELAY_SELECTION_MAX_ATTEMPTS: u32 = 3;
const RELAY_SELECTION_HEALTH_MAX_AGE_MS: u64 = 30_000;
const RELAY_SELECTION_MAX_CONSECUTIVE_FAILURES: u32 = 3;
const RELAY_SELECTION_SWITCH_COOLDOWN_MS: u64 = 5_000;
const EXIT_CONNECTION_WATCHDOG_INTERVAL: Duration = Duration::from_millis(250);
#[cfg(target_os = "linux")]
const EXIT_READINESS_TIMEOUT: Duration = Duration::from_secs(10);
#[cfg(target_os = "linux")]
const LOCAL_EXIT_DRAIN_TIMEOUT: Duration = Duration::from_secs(5);
#[cfg(target_os = "linux")]
const LOCAL_EXIT_DRAIN_RETRY_TIMEOUT: Duration = Duration::from_secs(1);

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
    /// Resolved once by the foreground entrypoint and carried across the
    /// detached control/node boundary. `None` means no UI owns this node.
    pub ui_mode: Option<crate::ui::UiModeResolution>,
}

/// Owns every handle required to stop and recreate one TUN data path.
///
/// `TunDevice` clones share one Linux link lifetime, so replacing only the
/// runtime-held handle leaves the reader task pinning the old link. Keeping the
/// device, reader task and buffer-pool channels together makes quiesce-before-
/// delete an invariant instead of a shutdown-only convention.
pub(crate) struct RuntimeTunIo {
    tun: Option<TunDevice>,
    packet_rx: mpsc::UnboundedReceiver<Vec<TunPacket>>,
    buffer_tx: mpsc::UnboundedSender<TunPacket>,
    reader_task: Option<JoinHandle<()>>,
    descriptor: TunIoDescriptor,
    stats: StatsCollector,
}

#[derive(Clone)]
struct TunIoDescriptor {
    name: String,
    assigned_ips: VirtualIps,
    mtu: u16,
}

impl RuntimeTunIo {
    fn new(
        config: &ClientConfig,
        assigned_ips: &VirtualIps,
        tun: TunDevice,
        stats: &StatsCollector,
    ) -> Self {
        let descriptor = TunIoDescriptor {
            name: config.tun.name.clone(),
            assigned_ips: assigned_ips.clone(),
            mtu: config.tun.mtu,
        };
        let (packet_rx, buffer_tx, reader_task) = spawn_tun_reader_pool(&tun, stats);
        Self {
            tun: Some(tun),
            packet_rx,
            buffer_tx,
            reader_task: Some(reader_task),
            descriptor,
            stats: stats.clone(),
        }
    }

    fn name(&self) -> &str {
        &self.descriptor.name
    }

    fn packet_rx(&mut self) -> &mut mpsc::UnboundedReceiver<Vec<TunPacket>> {
        &mut self.packet_rx
    }

    fn buffer_tx(&self) -> mpsc::UnboundedSender<TunPacket> {
        self.buffer_tx.clone()
    }

    fn tun_mut(&mut self) -> Option<&mut TunDevice> {
        self.tun.as_mut()
    }

    async fn stop_reader(&mut self) -> Result<()> {
        let Some(task) = self.reader_task.take() else {
            return Ok(());
        };
        task.abort();
        match task.await {
            Ok(()) => Ok(()),
            Err(error) if error.is_cancelled() => Ok(()),
            Err(error) => Err(anyhow::anyhow!("TUN reader task failed: {error}")),
        }
    }

    async fn stop_reader_within(&mut self, deadline: Instant) -> CleanupStatus {
        stop_runtime_task(&mut self.reader_task, deadline).await
    }

    fn abort_reader(&self) {
        abort_task(&self.reader_task);
    }

    /// Recreate the TUN link after the reader has fully released its clone.
    /// The old handle is dropped even when deletion reports a missing link, but
    /// creation still must succeed before the runtime reports recovery.
    async fn recreate(&mut self, stats: &StatsCollector) -> Result<()> {
        self.stop_reader().await?;
        if let Some(mut old_tun) = self.tun.take() {
            if let Err(error) = old_tun.delete().await {
                tracing::warn!("恢复时删除旧 TUN 未完成，将尝试重建同名设备: {error:#}");
            }
        }

        let tun = TunDevice::create_dual_stack(
            &self.descriptor.name,
            &self.descriptor.assigned_ips,
            24,
            64,
            self.descriptor.mtu,
        )
        .await
        .context("恢复时重建 TUN 设备失败")?;
        let (packet_rx, buffer_tx, reader_task) = spawn_tun_reader_pool(&tun, stats);
        self.tun = Some(tun);
        self.packet_rx = packet_rx;
        self.buffer_tx = buffer_tx;
        self.reader_task = Some(reader_task);
        Ok(())
    }

    async fn delete(&mut self) -> Result<()> {
        self.stop_reader().await?;
        if let Some(mut tun) = self.tun.take() {
            tun.delete().await?;
        }
        Ok(())
    }
}

#[cfg(target_os = "linux")]
impl LinuxRecoveryTun for RuntimeTunIo {
    fn name(&self) -> &str {
        RuntimeTunIo::name(self)
    }

    fn recreate(
        &mut self,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<
                    Output = Result<(), crate::routing::recovery::RecoveryOperationError>,
                > + Send
                + '_,
        >,
    > {
        Box::pin(async move {
            RuntimeTunIo::recreate(self, &self.stats.clone())
                .await
                .map_err(|_| {
                    crate::routing::recovery::RecoveryOperationError::new(
                        "recovery.tun_recreate_failed",
                    )
                })
        })
    }
}

/// Legacy config-file launch is intentionally no longer a separate runtime
/// entrypoint. A node must be started once, and all clients attach through the
/// authenticated local IPC boundary.
const CONFIG_START_REPLACEMENT: &str =
    "`start --config` 已停用：请使用 `join <票据/地址>` 或 `up [网络]` 启动网络；节点及其数据面会由后台控制进程统一管理";

pub async fn start(_config_path: &str) -> Result<()> {
    anyhow::bail!(CONFIG_START_REPLACEMENT)
}

pub async fn start_with_options(_config_path: &str, _options: RuntimeOptions) -> Result<()> {
    anyhow::bail!(CONFIG_START_REPLACEMENT)
}

pub async fn start_with_config(_config_path: &str, _config: ClientConfig) -> Result<()> {
    anyhow::bail!(CONFIG_START_REPLACEMENT)
}

#[cfg(test)]
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
    _config_path: &str,
    _config: ClientConfig,
    _options: RuntimeOptions,
) -> Result<()> {
    anyhow::bail!(CONFIG_START_REPLACEMENT)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CleanupStatus {
    Removed,
    NotOwned,
    Deferred(String),
    Failed(String),
    Unknown(String),
}

impl CleanupStatus {
    pub(crate) fn is_clean(&self) -> bool {
        matches!(self, Self::Removed | Self::NotOwned)
    }

    fn issue(&self) -> Option<String> {
        match self {
            Self::Removed | Self::NotOwned => None,
            Self::Deferred(message) => Some(format!("延迟完成: {message}")),
            Self::Failed(message) => Some(format!("失败: {message}")),
            Self::Unknown(message) => Some(format!("状态未知: {message}")),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CleanupStep {
    pub resource: &'static str,
    pub status: CleanupStatus,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct RuntimeCleanupOutcome {
    pub steps: Vec<CleanupStep>,
}

impl RuntimeCleanupOutcome {
    pub(crate) fn record(&mut self, resource: &'static str, status: CleanupStatus) {
        self.steps.push(CleanupStep { resource, status });
    }

    pub(crate) fn is_clean(&self) -> bool {
        !self.steps.is_empty() && self.steps.iter().all(|step| step.status.is_clean())
    }

    pub(crate) fn issues(&self) -> Vec<String> {
        if self.steps.is_empty() {
            return vec!["运行时未返回清理结果".into()];
        }
        self.steps
            .iter()
            .filter_map(|step| {
                step.status
                    .issue()
                    .map(|issue| format!("{} {issue}", step.resource))
            })
            .collect()
    }

    pub(crate) fn summary(&self) -> String {
        let issues = self.issues();
        if issues.is_empty() {
            "运行资源已清理".into()
        } else {
            issues.join("; ")
        }
    }
}

#[derive(Clone)]
pub(crate) struct RuntimeRecoveryTriggerSender {
    sender: mpsc::Sender<RecoveryTrigger>,
}

impl RuntimeRecoveryTriggerSender {
    /// Enqueue a recovery request without blocking the caller.
    ///
    /// Recovery requests are coalescible operational hints, so a bounded
    /// channel is used to keep a failed subsystem from turning into unbounded
    /// memory growth. The caller remains responsible for the actual failure
    /// handling when this returns false.
    fn try_trigger(&self, trigger: RecoveryTrigger) -> bool {
        self.sender.try_send(trigger).is_ok()
    }
}

pub(crate) struct RuntimeRecoveryEvents {
    #[cfg(target_os = "linux")]
    watcher: Option<LinuxRecoveryEventWatcher>,
    trigger_rx: mpsc::Receiver<RecoveryTrigger>,
}

impl RuntimeRecoveryEvents {
    fn spawn() -> (Self, RuntimeRecoveryTriggerSender) {
        let (trigger_tx, trigger_rx) = mpsc::channel(RECOVERY_TRIGGER_QUEUE_CAPACITY);
        (
            Self {
                #[cfg(target_os = "linux")]
                watcher: Some(LinuxRecoveryEventWatcher::spawn()),
                trigger_rx,
            },
            RuntimeRecoveryTriggerSender { sender: trigger_tx },
        )
    }

    #[cfg(test)]
    fn without_os_watcher_for_test() -> (Self, RuntimeRecoveryTriggerSender) {
        let (trigger_tx, trigger_rx) = mpsc::channel(RECOVERY_TRIGGER_QUEUE_CAPACITY);
        (
            Self {
                #[cfg(target_os = "linux")]
                watcher: None,
                trigger_rx,
            },
            RuntimeRecoveryTriggerSender { sender: trigger_tx },
        )
    }

    fn shutdown(&self) {
        #[cfg(target_os = "linux")]
        if let Some(watcher) = self.watcher.as_ref() {
            watcher.shutdown();
        }
    }

    async fn next_trigger(&mut self) -> Option<RecoveryTrigger> {
        #[cfg(target_os = "linux")]
        loop {
            let Some(watcher) = self.watcher.as_mut() else {
                return self.trigger_rx.recv().await;
            };
            tokio::select! {
                event = watcher.recv() => match event {
                    Some(LinuxRecoveryEvent::Trigger(trigger)) => return Some(trigger),
                    Some(LinuxRecoveryEvent::SourceAvailable(source)) => tracing::info!(
                        source = ?source,
                        diagnostic_id = "recovery.linux.event_source_available",
                        "Linux 恢复事件源已连接"
                    ),
                    Some(LinuxRecoveryEvent::SourceUnavailable(source)) => tracing::warn!(
                        source = ?source,
                        diagnostic_id = "recovery.linux.event_source_unavailable",
                        "Linux 恢复事件源不可用，将自动重试"
                    ),
                    None => self.watcher = None,
                },
                trigger = self.trigger_rx.recv() => return trigger,
            }
        }

        #[cfg(not(target_os = "linux"))]
        {
            self.trigger_rx.recv().await
        }
    }
}

pub(crate) struct ClientRuntime {
    pub(crate) signaling: Arc<SignalingClient>,
    pub(crate) p2p: P2pEngine,
    pub(crate) identity: Identity,
    pub(crate) network_id: Option<NetworkId>,
    pub(crate) outbound: OutboundDispatcher,
    relay_selection: Arc<RwLock<RelaySelectionTracker>>,
    #[cfg(target_os = "linux")]
    pub(crate) route_manager: LinuxRouteManager,
    #[cfg(target_os = "linux")]
    pub(crate) exit_forwarder: Option<LinuxExitForwarder>,
    #[cfg(target_os = "linux")]
    peer_route_manager: LinuxPeerRouteManager,
    #[cfg(target_os = "linux")]
    pub(crate) exit_drain_deadline: Option<Instant>,
    pub(crate) local_exit_runtime_problem: Option<NodeProblem>,
    pub(crate) recovery_events: RuntimeRecoveryEvents,
    recovery_trigger_tx: RuntimeRecoveryTriggerSender,
    #[cfg(target_os = "linux")]
    exit_recovery_path: PathRecoveryState,
    #[cfg(target_os = "linux")]
    relay_recovery_path: PathRecoveryState,
    #[cfg(target_os = "linux")]
    exit_connection_watchdog: Option<ExitConnectionWatchdog>,
    #[cfg(target_os = "linux")]
    pending_exit_readiness: Option<PendingExitReadiness>,
    pub(crate) tun_io: RuntimeTunIo,
    pub(crate) assigned_ip: Ipv6Addr,
    pub(crate) assigned_ips: VirtualIps,
    pub(crate) stats: StatsCollector,
    pub(crate) acl: fuckipv6_common::AclEngine,
    pub(crate) route_engine: SmartRouteEngine,
    pub(crate) exit_node_manager: ExitNodeManager,
    pub(crate) exit_contribution_gate: ExitContributionGate,
    exit_selection_gate: ExitSelectionGate,
    exit_selection_blocked: bool,
    pub(crate) configured_exit_node: Option<String>,
    pub(crate) heartbeat_interval: tokio::time::Interval,
    pub(crate) config_path: String,
    pub(crate) config: ClientConfig,
    pub(crate) local_exit_enabled: bool,
    pub(crate) control_rx: mpsc::UnboundedReceiver<ipc::RuntimeCommand>,
    node_state: Option<NodeStateStore>,
    node_state_updates: Option<watch::Receiver<NodeSnapshot>>,
    traffic_session: TrafficSessionSnapshot,
    local_exit_snapshot: Option<LocalExitCapabilitySnapshot>,
    local_exit_availability_report_interval: tokio::time::Interval,
    exit_contribution_member_refresh_task: Option<JoinHandle<()>>,
    exit_contribution_directory_updates: Option<mpsc::Receiver<ExitContributionDirectoryUpdate>>,
    pub(crate) ipc_task: Option<JoinHandle<()>>,
    pub(crate) config_reload_task: Option<JoinHandle<()>>,
    pub(crate) relay_reconnect_task: Option<JoinHandle<()>>,
    pub(crate) relay_grant_renewal_task: Option<JoinHandle<()>>,
}

#[cfg(target_os = "linux")]
#[derive(Debug, Clone, PartialEq, Eq)]
struct ExitConnectionWatchdog {
    target: NodeId,
    deadline_unix_ms: u64,
    validation_trigger: RecoveryTrigger,
    validation_queued: bool,
}

#[cfg(target_os = "linux")]
#[derive(Debug, Clone)]
struct PendingExitReadiness {
    target: NodeId,
    nonce: [u8; 16],
    targets: Vec<RecoveryProbeTarget>,
    deadline: Instant,
}

#[cfg(target_os = "linux")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExitConnectionWatchdogAction {
    Wait,
    Validate(RecoveryTrigger),
    TimedOut,
}

#[cfg(target_os = "linux")]
impl ExitConnectionWatchdog {
    fn new(
        target: NodeId,
        now_unix_ms: u64,
        connect_timeout_secs: u64,
        validation_trigger: RecoveryTrigger,
    ) -> Self {
        debug_assert!(matches!(
            validation_trigger,
            RecoveryTrigger::Manual | RecoveryTrigger::ExitMigration
        ));
        Self {
            target,
            deadline_unix_ms: now_unix_ms
                .saturating_add(connect_timeout_secs.saturating_mul(1_000)),
            validation_trigger,
            validation_queued: false,
        }
    }

    fn observe(
        &mut self,
        now_unix_ms: u64,
        session_established: bool,
    ) -> ExitConnectionWatchdogAction {
        if self.validation_queued {
            return ExitConnectionWatchdogAction::Wait;
        }
        if now_unix_ms >= self.deadline_unix_ms {
            self.validation_queued = true;
            return ExitConnectionWatchdogAction::TimedOut;
        }
        if session_established {
            self.validation_queued = true;
            return ExitConnectionWatchdogAction::Validate(self.validation_trigger);
        }
        ExitConnectionWatchdogAction::Wait
    }
}

impl ClientRuntime {
    pub(crate) fn tun_name(&self) -> &str {
        self.tun_io.name()
    }

    pub(crate) async fn run_with_shutdown(
        &mut self,
        shutdown_rx: &mut tokio::sync::watch::Receiver<bool>,
    ) -> Result<Option<DisconnectReply>> {
        run_client_event_loop_with_shutdown(self, shutdown_rx).await
    }

    pub(crate) async fn shutdown(&mut self) -> RuntimeCleanupOutcome {
        shutdown_runtime(self).await
    }

    /// Bind Exit contribution enforcement to the same authoritative state that
    /// every UI consumes. Membership terms and the owner policy are copied only
    /// after validating the exact network/member identity; rosters are refreshed
    /// independently so a stale one fails closed without blocking packet flow.
    pub(crate) async fn attach_node_state(&mut self, state: NodeStateStore) {
        let snapshot = state.snapshot().await;
        self.node_state = Some(state.clone());
        self.local_exit_snapshot = Some(snapshot.local_exit.clone());
        self.apply_exit_contribution_snapshot(&snapshot).await;
        self.publish_traffic_session().await;

        let Some(network_id) = self.network_id.clone() else {
            self.node_state_updates = Some(state.subscribe());
            return;
        };
        self.report_local_exit_availability("initial_registration")
            .await;
        match (
            state.member_directory(&network_id).await,
            state.member_directory_received_at(&network_id).await,
        ) {
            (Some(directory), Some(received_at_unix_ms)) => {
                let selection_directory = directory.clone();
                if let Err(denial) = self
                    .exit_contribution_gate
                    .replace_member_directory(directory, received_at_unix_ms)
                {
                    tracing::warn!(
                        reason_code = denial.reason_code(),
                        "初始成员目录不能用于 Exit 贡献授权，已失败关闭"
                    );
                    self.exit_contribution_gate.clear_member_directory();
                }
                self.replace_exit_selection_directory(selection_directory, received_at_unix_ms)
                    .await;
            }
            _ => {
                self.exit_contribution_gate.clear_member_directory();
                self.handle_exit_selection_directory_unavailable(
                    "exit.selection.directory_unavailable",
                )
                .await;
            }
        }

        let (directory_tx, directory_rx) =
            mpsc::channel(EXIT_CONTRIBUTION_DIRECTORY_QUEUE_CAPACITY);
        self.exit_contribution_directory_updates = Some(directory_rx);
        self.exit_contribution_member_refresh_task =
            Some(tokio::spawn(run_exit_contribution_member_refresh(
                self.signaling.clone(),
                state.clone(),
                network_id,
                directory_tx,
            )));
        self.node_state_updates = Some(state.subscribe());
    }

    async fn report_local_exit_availability(&self, trigger: &'static str) {
        let Some(network_id) = self.network_id.clone() else {
            return;
        };
        let Some(snapshot) = self.local_exit_snapshot.as_ref() else {
            return;
        };
        let (available, reason_code) = local_exit_availability_report(snapshot);
        match self
            .signaling
            .send(SignalingMessage::ExitAvailabilityReport {
                network_id,
                available,
                reason_code: reason_code.map(str::to_owned),
            }) {
            Ok(()) => self.stats.record_signaling_sent().await,
            Err(error) => tracing::warn!(
                trigger,
                error = %error,
                "发送本机 Exit 可用性上报失败"
            ),
        }
    }

    async fn publish_traffic_session(&self) {
        let Some(state) = self.node_state.as_ref() else {
            return;
        };
        state
            .set_traffic_session(self.traffic_session.clone())
            .await;
    }

    fn block_exit_session(
        &mut self,
        failure: TrafficFailureKind,
        now_unix_ms: u64,
        reason_code: &'static str,
    ) {
        self.exit_selection_blocked = true;
        #[cfg(target_os = "linux")]
        {
            self.exit_connection_watchdog = None;
            self.pending_exit_readiness = None;
        }
        self.exit_selection_gate.set_current_exit(None);
        self.exit_selection_gate.discard_pending_exit();
        if let Err(error) =
            self.traffic_session
                .enter_fail_closed(Some(failure), now_unix_ms.max(1), reason_code)
        {
            tracing::error!(
                error = %error,
                reason_code,
                "无法写入失败关闭的流量会话状态"
            );
        }
    }

    async fn enter_fail_closed(
        &mut self,
        failure: TrafficFailureKind,
        now_unix_ms: u64,
        reason_code: &'static str,
    ) {
        self.block_exit_session(failure, now_unix_ms, reason_code);
        self.restore_host_network(reason_code).await;
        self.publish_traffic_session().await;
    }

    async fn restore_host_network(&mut self, reason_code: &'static str) {
        #[cfg(target_os = "linux")]
        if let Err(error) = self.route_manager.deactivate_global_proxy().await {
            tracing::error!(
                reason_code,
                error = %error,
                "失败关闭状态已写入，但恢复宿主网络失败；兜底路由与 DNS 所有权已保留以便重试"
            );
        }
        #[cfg(not(target_os = "linux"))]
        let _ = reason_code;
    }

    async fn clear_exit_selection(&mut self, reason_code: &'static str) {
        self.exit_selection_gate.clear_member_directory();
        // Snapshot refreshes can repeatedly observe the same missing context.
        // Publishing an identical fail-closed transition would wake the state
        // subscriber, which observes the new revision and repeats this path.
        if traffic_session_already_failed_closed(&self.traffic_session, reason_code) {
            // A prior route/DNS restoration may have failed transiently. Keep
            // this idempotent path as a cleanup retry even when the traffic
            // state itself is already fail-closed.
            self.restore_host_network(reason_code).await;
            return;
        }
        self.enter_fail_closed(
            TrafficFailureKind::NetworkSelectionUnavailable,
            unix_time_ms(),
            reason_code,
        )
        .await;
    }

    /// A missing Exit directory is not by itself a traffic failure. Mesh-only
    /// networks never consult Exit authority, and proxy networks without an
    /// established/pending Exit can remain idle until packet admission asks for
    /// an authorized target (which then fails closed if evidence is missing).
    /// Once an Exit session exists, losing its authoritative directory must
    /// still terminate the session rather than continuing on stale permission.
    async fn handle_exit_selection_directory_unavailable(&mut self, reason_code: &'static str) {
        let had_exit_session = self.exit_selection_blocked
            || self.exit_selection_gate.current_exit().is_some()
            || self.exit_selection_gate.pending_exit().is_some()
            || matches!(
                self.traffic_session.state,
                TrafficProtectionState::Connecting
                    | TrafficProtectionState::Migrating
                    | TrafficProtectionState::Protected
                    | TrafficProtectionState::ExplicitDirectFallback
                    | TrafficProtectionState::FailClosed
            );
        self.exit_selection_gate.clear_member_directory();
        if exit_selection_directory_loss_requires_fail_closed(
            self.route_engine.routing_mode(),
            had_exit_session,
        ) && !traffic_session_already_failed_closed(&self.traffic_session, reason_code)
        {
            self.enter_fail_closed(
                TrafficFailureKind::NetworkSelectionUnavailable,
                unix_time_ms(),
                reason_code,
            )
            .await;
        }
    }

    fn intended_exit(&self) -> Option<NodeId> {
        self.exit_selection_gate
            .pending_exit()
            .or_else(|| self.exit_selection_gate.current_exit())
            .cloned()
    }

    fn arm_exit_connection_watchdog(
        &mut self,
        target: NodeId,
        now_unix_ms: u64,
        validation_trigger: RecoveryTrigger,
    ) {
        #[cfg(target_os = "linux")]
        {
            if self
                .exit_connection_watchdog
                .as_ref()
                .is_some_and(|watchdog| watchdog.target == target)
            {
                return;
            }
            self.exit_connection_watchdog = Some(ExitConnectionWatchdog::new(
                target,
                now_unix_ms,
                self.config.p2p.connect_timeout,
                validation_trigger,
            ));
            self.exit_recovery_path = PathRecoveryState::Disconnected;
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = target;
            if !self.recovery_trigger_tx.try_trigger(validation_trigger) {
                tracing::warn!(
                    diagnostic_id = "recovery.trigger_queue_unavailable",
                    "Exit 验证无法排队，数据平面已失败关闭"
                );
                self.block_exit_session(
                    TrafficFailureKind::ConnectionTimeout,
                    now_unix_ms,
                    "traffic.session.validation_schedule_failed",
                );
            }
        }
    }

    async fn begin_exit_migration(
        &mut self,
        to: NodeId,
        now_unix_ms: u64,
        reason_code: &'static str,
    ) {
        #[cfg(target_os = "linux")]
        {
            self.pending_exit_readiness = None;
        }
        if let Some(pending) = self.exit_selection_gate.pending_exit() {
            if pending != &to {
                self.enter_fail_closed(
                    TrafficFailureKind::ExitUnavailable,
                    now_unix_ms,
                    "exit.selection.migration_target_conflict",
                )
                .await;
                return;
            }
            self.arm_exit_connection_watchdog(
                pending.clone(),
                now_unix_ms,
                RecoveryTrigger::ExitMigration,
            );
            self.exit_selection_blocked = true;
            if self.traffic_session.state != TrafficProtectionState::Migrating {
                if let Err(error) =
                    self.traffic_session
                        .begin_recovery(true, now_unix_ms.max(1), reason_code)
                {
                    tracing::error!(error = %error, "无法写入 Exit 迁移状态");
                    self.enter_fail_closed(
                        TrafficFailureKind::ExitUnavailable,
                        now_unix_ms,
                        "exit.selection.migration_state_failed",
                    )
                    .await;
                    return;
                }
            }
            self.publish_traffic_session().await;
            return;
        }

        if !self.exit_selection_gate.begin_pending_exit(to.clone()) {
            self.enter_fail_closed(
                TrafficFailureKind::ExitUnavailable,
                now_unix_ms,
                "exit.selection.migration_target_unavailable",
            )
            .await;
            return;
        }
        self.exit_selection_blocked = true;
        if let Err(error) =
            self.traffic_session
                .begin_recovery(true, now_unix_ms.max(1), reason_code)
        {
            tracing::error!(error = %error, "无法写入 Exit 迁移状态");
            self.enter_fail_closed(
                TrafficFailureKind::ExitUnavailable,
                now_unix_ms,
                "exit.selection.migration_state_failed",
            )
            .await;
            return;
        }
        self.arm_exit_connection_watchdog(to, now_unix_ms, RecoveryTrigger::ExitMigration);
        self.publish_traffic_session().await;
    }

    async fn begin_exit_recovery(&mut self, trigger: RecoveryTrigger, now_unix_ms: u64) {
        #[cfg(target_os = "linux")]
        {
            self.pending_exit_readiness = None;
        }
        self.exit_selection_blocked = true;
        let migrating = self.exit_selection_gate.current_exit().is_some()
            && self.exit_selection_gate.pending_exit().is_some();
        if let Err(error) = self.traffic_session.begin_recovery(
            migrating,
            now_unix_ms.max(1),
            recovery_started_reason_code(trigger),
        ) {
            tracing::error!(error = %error, "无法写入恢复中的流量会话状态");
            self.enter_fail_closed(
                trigger
                    .failure_kind()
                    .unwrap_or(TrafficFailureKind::ConnectionTimeout),
                now_unix_ms,
                "recovery.state_transition_failed",
            )
            .await;
            return;
        }
        self.publish_traffic_session().await;
    }

    async fn apply_exit_contribution_snapshot(&mut self, snapshot: &NodeSnapshot) {
        self.exit_contribution_gate
            .replace_policy(snapshot.access_control.local_policy.clone());

        let Some(network_id) = self.network_id.clone() else {
            self.exit_contribution_gate.clear_network_context();
            self.exit_contribution_gate.clear_member_directory();
            self.clear_exit_selection("exit.selection.network_context_missing")
                .await;
            return;
        };
        let local_node_id = NodeId::from_public_key(&self.identity.public_key());
        if local_membership_relationship_is_revoked(snapshot, &network_id, &local_node_id) {
            self.exit_contribution_gate.clear_network_context();
            self.exit_contribution_gate.clear_member_directory();
            let already_revoked = self.traffic_session.state == TrafficProtectionState::FailClosed
                && self.traffic_session.reason_code.as_deref()
                    == Some("exit.selection.authorization_revoked");
            if !already_revoked {
                tracing::warn!(
                    network_id = %network_id,
                    "成员关系已明确撤权，终止本机 Exit 授权"
                );
                self.clear_exit_selection("exit.selection.authorization_revoked")
                    .await;
            }
            return;
        }

        match exit_contribution_context_from_snapshot(snapshot, &network_id, &local_node_id) {
            Some(context) => {
                let restored = !self.exit_contribution_gate.has_network_context();
                self.exit_contribution_gate.replace_network_context(context);
                if restored {
                    tracing::debug!(
                        network_id = %network_id,
                        "本机 Exit 贡献授权上下文已就绪或恢复"
                    );
                }
            }
            None => {
                if self.exit_contribution_gate.has_network_context() {
                    tracing::warn!(
                        network_id = %network_id,
                        "本机 Exit 贡献授权证据暂时不可用，已清空贡献上下文"
                    );
                } else {
                    tracing::debug!(
                        network_id = %network_id,
                        "本机 Exit 贡献授权上下文尚未就绪"
                    );
                }
                self.exit_contribution_gate.clear_network_context();
            }
        }
    }

    async fn replace_exit_selection_directory(
        &mut self,
        directory: NetworkMemberDirectory,
        received_at_unix_ms: u64,
    ) {
        match self
            .exit_selection_gate
            .replace_member_directory(directory, received_at_unix_ms)
        {
            Ok(update) => {
                if update == crate::routing::ExitSelectionDirectoryUpdate::Applied {
                    self.exit_selection_blocked = false;
                    self.reconcile_exit_selection(unix_time_ms()).await;
                }
            }
            Err(denial) => {
                tracing::warn!(
                    reason_code = denial.reason_code(),
                    "成员目录不能用于管理员 Exit 选择授权，已清理 Exit 选择会话"
                );
                self.handle_exit_selection_directory_unavailable(
                    "exit.selection.directory_rejected",
                )
                .await;
            }
        }
    }

    async fn reconcile_exit_selection(&mut self, now_unix_ms: u64) {
        let Some(current_exit) = self.exit_selection_gate.current_exit().cloned() else {
            return;
        };
        match self
            .exit_selection_gate
            .plan_current_session_transition(now_unix_ms)
        {
            Ok(transition) => match transition.action {
                ExitSessionAction::Keep => {}
                ExitSessionAction::Migrate { to } => {
                    tracing::warn!(
                        from_exit = %current_exit,
                        to_exit = %to,
                        policy_revision = transition.policy_revision,
                        reason = ?transition.reason,
                        "管理员 Exit 策略已变更，先挂起迁移并关闭数据平面"
                    );
                    self.begin_exit_migration(to, now_unix_ms, "exit.selection.migration_pending")
                        .await;
                }
                ExitSessionAction::Terminate | ExitSessionAction::FailClosed => {
                    tracing::warn!(
                        from_exit = %current_exit,
                        policy_revision = transition.policy_revision,
                        reason = ?transition.reason,
                        "管理员 Exit 策略已撤权，终止当前 Exit 会话"
                    );
                    self.enter_fail_closed(
                        TrafficFailureKind::NetworkSelectionUnavailable,
                        now_unix_ms,
                        "exit.selection.authorization_revoked",
                    )
                    .await;
                }
            },
            Err(denial) => {
                tracing::warn!(
                    from_exit = %current_exit,
                    reason_code = denial.reason_code(),
                    "Exit 选择授权不可用，当前 Exit 会话失败关闭"
                );
                self.enter_fail_closed(
                    TrafficFailureKind::NetworkSelectionUnavailable,
                    now_unix_ms,
                    "exit.selection.authorization_unavailable",
                )
                .await;
            }
        }
    }

    async fn requested_exit_node_id(&self) -> Option<NodeId> {
        let target = self.configured_exit_node.as_deref()?.trim();
        if target.is_empty() {
            return None;
        }
        let virtual_ip = self.p2p.resolve_exit_peer(Some(target)).await?;
        let peer = self.p2p.peer_by_ip(IpAddr::V6(virtual_ip)).await?;
        Some(NodeId::from_public_key(&peer.public_key))
    }

    /// Return the one administrator-authorized Exit for this packet. Packet
    /// admission is allowed only after recovery has produced complete evidence
    /// for the current Exit. A pending migration never promotes its target here.
    async fn exit_for_packet(&mut self, now_unix_ms: u64) -> Option<NodeId> {
        if self.route_engine.routing_mode() == fuckipv6_common::RoutingMode::MeshOnly {
            return None;
        }
        if self.exit_selection_blocked || self.exit_selection_gate.pending_exit().is_some() {
            return None;
        }
        if let Some(current_exit) = self.exit_selection_gate.current_exit().cloned() {
            match self
                .exit_selection_gate
                .plan_current_session_transition(now_unix_ms)
            {
                Ok(transition) => match transition.action {
                    ExitSessionAction::Keep => {
                        return exit_packet_admission_allowed(
                            self.exit_selection_blocked,
                            self.exit_selection_gate.pending_exit().is_some(),
                            self.traffic_session.state,
                        )
                        .then_some(current_exit);
                    }
                    ExitSessionAction::Migrate { to } => {
                        tracing::warn!(
                            from_exit = %current_exit,
                            to_exit = %to,
                            reason = ?transition.reason,
                            "Exit 会话迁移已挂起，验证完成前不放行数据包"
                        );
                        self.begin_exit_migration(
                            to,
                            now_unix_ms,
                            "exit.selection.migration_pending",
                        )
                        .await;
                        return None;
                    }
                    ExitSessionAction::Terminate | ExitSessionAction::FailClosed => {
                        tracing::warn!(
                            from_exit = %current_exit,
                            reason = ?transition.reason,
                            "Exit 会话已按管理员策略关闭"
                        );
                        self.enter_fail_closed(
                            TrafficFailureKind::NetworkSelectionUnavailable,
                            now_unix_ms,
                            "exit.selection.authorization_revoked",
                        )
                        .await;
                        return None;
                    }
                },
                Err(denial) => {
                    tracing::warn!(
                        from_exit = %current_exit,
                        reason_code = denial.reason_code(),
                        "Exit 选择授权陈旧或无效，当前 Exit 会话失败关闭"
                    );
                    self.enter_fail_closed(
                        TrafficFailureKind::NetworkSelectionUnavailable,
                        now_unix_ms,
                        "exit.selection.authorization_unavailable",
                    )
                    .await;
                    return None;
                }
            }
        }

        let requested = self.requested_exit_node_id().await;
        match self
            .exit_selection_gate
            .select_for_session(now_unix_ms, requested.as_ref())
        {
            Ok(selected) => {
                tracing::debug!(
                    exit = %selected,
                    requested = ?requested,
                    "管理员 Exit 策略为新会话选定出口，等待恢复验证"
                );
                if !self
                    .exit_selection_gate
                    .begin_pending_exit(selected.clone())
                {
                    self.enter_fail_closed(
                        TrafficFailureKind::ExitUnavailable,
                        now_unix_ms,
                        "exit.selection.pending_target_conflict",
                    )
                    .await;
                    return None;
                }
                self.exit_selection_blocked = true;
                if let Err(error) = self
                    .traffic_session
                    .set_connecting(now_unix_ms.max(1), "traffic.session.exit_selected")
                {
                    tracing::error!(error = %error, "无法写入 Exit 连接状态");
                    self.enter_fail_closed(
                        TrafficFailureKind::ConnectionTimeout,
                        now_unix_ms,
                        "traffic.session.connecting_state_failed",
                    )
                    .await;
                    return None;
                } else {
                    self.arm_exit_connection_watchdog(
                        selected,
                        now_unix_ms,
                        RecoveryTrigger::Manual,
                    );
                }
                self.publish_traffic_session().await;
                None
            }
            Err(denial) => {
                tracing::warn!(
                    reason_code = denial.reason_code(),
                    requested = ?requested,
                    "没有可证明授权的新 Exit 会话，数据平面失败关闭"
                );
                self.enter_fail_closed(
                    TrafficFailureKind::NetworkSelectionUnavailable,
                    now_unix_ms,
                    "exit.selection.no_authorized_exit",
                )
                .await;
                None
            }
        }
    }
}

/// Packet admission is deliberately narrower than "a target was selected": an
/// active recovery or migration must close the data plane even while the
/// previous Exit identity is still retained for diagnosis.
fn exit_packet_admission_allowed(
    blocked: bool,
    migration_pending: bool,
    state: TrafficProtectionState,
) -> bool {
    !blocked && !migration_pending && state.may_carry_traffic()
}

impl Drop for ClientRuntime {
    fn drop(&mut self) {
        self.signaling.shutdown();
        abort_task(&self.ipc_task);
        abort_task(&self.config_reload_task);
        self.tun_io.abort_reader();
        abort_task(&self.relay_reconnect_task);
        abort_task(&self.relay_grant_renewal_task);
        abort_task(&self.exit_contribution_member_refresh_task);
        self.recovery_events.shutdown();
    }
}

#[derive(Debug)]
struct ExitContributionDirectoryUpdate {
    directory: Option<NetworkMemberDirectory>,
    received_at_unix_ms: Option<u64>,
}

async fn run_exit_contribution_member_refresh(
    signaling: Arc<SignalingClient>,
    state: NodeStateStore,
    network_id: NetworkId,
    updates: mpsc::Sender<ExitContributionDirectoryUpdate>,
) {
    loop {
        let update = match signaling.list_network_members(network_id.clone()).await {
            Ok(directory) => {
                state.set_member_directory(directory.clone()).await;
                ExitContributionDirectoryUpdate {
                    directory: Some(directory),
                    received_at_unix_ms: state.member_directory_received_at(&network_id).await,
                }
            }
            Err(error) => {
                state.clear_member_directory(&network_id).await;
                tracing::warn!(
                    network_id = %network_id,
                    error = %error,
                    "刷新成员目录失败，Exit 贡献授权已失败关闭"
                );
                ExitContributionDirectoryUpdate {
                    directory: None,
                    received_at_unix_ms: None,
                }
            }
        };
        if updates.send(update).await.is_err() {
            return;
        }
        sleep(EXIT_CONTRIBUTION_MEMBER_REFRESH_INTERVAL).await;
    }
}

fn exit_contribution_context_from_snapshot(
    snapshot: &NodeSnapshot,
    network_id: &NetworkId,
    local_node_id: &NodeId,
) -> Option<ExitContributionNetworkContext> {
    let membership = snapshot.memberships.iter().find(|membership| {
        membership.network_id == network_id.as_str() && membership.node_id == local_node_id.as_str()
    })?;
    if !membership.state.can_access_network_resources() {
        return None;
    }
    let terms = membership.current_terms.as_ref()?;
    let terms_hash = terms.content_hash().ok()?;
    Some(ExitContributionNetworkContext {
        mode: terms.exit_contribution_mode,
        policy_revision: terms.network_policy_revision,
        terms_hash,
        policy_confirmation_required: membership.policy_confirmation_required,
    })
}

/// Startup and shutdown traverse non-access membership states (`accepted`,
/// `key_ready`, `connecting`). Missing evidence and those transient states are
/// not authorization revocations. Only an explicit withdrawal removes the
/// local authority.
fn local_membership_relationship_is_revoked(
    snapshot: &NodeSnapshot,
    network_id: &NetworkId,
    local_node_id: &NodeId,
) -> bool {
    let membership = snapshot.memberships.iter().find(|membership| {
        membership.network_id == network_id.as_str() && membership.node_id == local_node_id.as_str()
    });
    membership.is_some_and(|membership| {
        matches!(
            membership.state,
            MembershipState::Leaving
                | MembershipState::Left
                | MembershipState::Rejected
                | MembershipState::Suspended
                | MembershipState::Cancelled
        )
    })
}

fn exit_selection_directory_loss_requires_fail_closed(
    mode: fuckipv6_common::RoutingMode,
    had_exit_session: bool,
) -> bool {
    mode != fuckipv6_common::RoutingMode::MeshOnly && had_exit_session
}

fn traffic_session_already_failed_closed(
    session: &TrafficSessionSnapshot,
    reason_code: &str,
) -> bool {
    session.state == TrafficProtectionState::FailClosed
        && session.reason_code.as_deref() == Some(reason_code)
}

pub(crate) struct RuntimeObservability {
    pub(crate) state: Option<NodeStateStore>,
    pub(crate) failover_audit: Option<Arc<OwnerSecurityAudit>>,
}

pub(crate) async fn prepare_client_runtime_ext(
    config_path: &str,
    mut config: ClientConfig,
    network_id: Option<NetworkId>,
    network_name: Option<&str>,
    local_exit_enabled: bool,
    enable_ipc: bool,
    observability: RuntimeObservability,
) -> Result<ClientRuntime> {
    let RuntimeObservability {
        state: node_state,
        failover_audit,
    } = observability;
    config.tun.name = crate::multi_network::derive_tun_name(network_name, &config.tun.name);

    #[cfg(target_os = "linux")]
    crate::routing::doh_dns::DohDnsGuard::restore_stale_host_configuration()
        .context("恢复上次异常退出遗留的 DNS 配置失败")?;

    let identity = load_identity(&config)?;
    let exit_contribution_gate = ExitContributionGate::new(
        NodeId::from_public_key(&identity.public_key()),
        network_id.clone(),
    );
    let exit_selection_gate = ExitSelectionGate::new(
        NodeId::from_public_key(&identity.public_key()),
        network_id.clone(),
    );
    let relay_identity = identity.clone();

    let (p2p, nat_report) = create_p2p_engine(&config).await?;
    p2p.set_identity(identity.clone()).await;
    let signaling = Arc::new(connect_signaling(&config, identity, &p2p, &nat_report).await?);
    p2p.set_control_signaling(signaling.clone()).await;
    let assigned_ips = wait_for_assigned_ips(&signaling).await?;
    let assigned_ip = assigned_ips.primary_ipv6();
    configure_local_ips(&p2p, &assigned_ips).await;
    let relay_selection = Arc::new(RwLock::new(RelaySelectionTracker::default()));
    let initial_node_snapshot = match node_state.as_ref() {
        Some(state) => Some(state.snapshot().await),
        None => None,
    };
    let relay = connect_quic_relay(
        signaling.as_ref(),
        &relay_identity,
        assigned_ip,
        network_id.as_ref(),
        initial_node_snapshot.as_ref(),
        relay_selection.as_ref(),
    )
    .await;
    let tun = create_tun_device(&config, &assigned_ips).await?;
    #[cfg(target_os = "linux")]
    let mut tun = tun;
    let acl = fuckipv6_common::AclEngine::new(&config.acl);

    let mut route_engine = SmartRouteEngine::new(config.routing.mode);
    route_engine.set_virtual_networks(
        parse_optional_ipv6_cidr(
            config.network.virtual_network.as_deref(),
            "network.virtual_network",
        )?,
        Some(parse_default_virtual_ipv4_network()?),
    );
    for cidr in parse_cidr_list(&config.routing.direct_cidrs, "routing.direct_cidrs")? {
        route_engine.add_direct_cidr(cidr);
    }
    for cidr in parse_cidr_list(&config.routing.proxy_cidrs, "routing.proxy_cidrs")? {
        route_engine.add_proxy_cidr(cidr);
    }
    let effective_exit_enabled = local_exit_effective(local_exit_enabled, config.exit_node.enabled);
    let mut effective_exit_config = config.exit_node.clone();
    effective_exit_config.enabled = effective_exit_enabled;
    let exit_node_manager = ExitNodeManager::new(effective_exit_config);
    let configured_exit_node = config.routing.exit_node.clone();
    #[cfg(target_os = "linux")]
    let (route_manager, exit_forwarder, peer_route_manager) = {
        let mut virtual_cidrs = Vec::new();
        if let Some(network) = parse_optional_ipv6_cidr(
            config.network.virtual_network.as_deref(),
            "network.virtual_network",
        )? {
            virtual_cidrs.push(IpNet::V6(network));
        }
        virtual_cidrs.push(IpNet::V4(parse_default_virtual_ipv4_network()?));

        let direct_cidrs = parse_cidr_list(&config.routing.direct_cidrs, "routing.direct_cidrs")?;
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
                    relay.client.close().await;
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
                let _ = manager.rollback().await;
                signaling.shutdown();
                if let Some(relay) = relay.as_ref() {
                    relay.client.close().await;
                }
                let _ = tun.delete().await;
                return Err(error).context("安装节点物理网络旁路失败");
            }
        }
        let exit_forwarder = if effective_exit_enabled {
            LinuxExitForwarder::install(&config.exit_node, tun.name()).await
        } else {
            LinuxExitForwarder::purge_stale(tun.name())
                .await
                .map(|()| None)
        };
        let mut exit_forwarder = match exit_forwarder {
            Ok(forwarder) => forwarder,
            Err(error) => {
                let _ = manager.rollback().await;
                signaling.shutdown();
                if let Some(relay) = relay.as_ref() {
                    relay.client.close().await;
                }
                let _ = tun.delete().await;
                return Err(error).context("准备 Exit Node 内核转发失败");
            }
        };

        let mut peer_route_manager = LinuxPeerRouteManager::new(tun.name());
        for peer in signaling.known_peers().await {
            if let Err(error) = peer_route_manager
                .sync_peer(&peer.client_id, &peer.virtual_ips)
                .await
            {
                let _ = peer_route_manager
                    .rollback_within(Duration::from_secs(2))
                    .await;
                if let Some(forwarder) = exit_forwarder.as_mut() {
                    let _ = forwarder.rollback_within(Duration::from_secs(2)).await;
                }
                let _ = manager.rollback().await;
                signaling.shutdown();
                if let Some(relay) = relay.as_ref() {
                    relay.client.close().await;
                }
                let _ = tun.delete().await;
                return Err(error).with_context(|| {
                    format!("安装节点 {} 的虚拟地址精确定向路由失败", peer.client_id)
                });
            }
        }
        (manager, exit_forwarder, peer_route_manager)
    };

    // 创建性能统计收集器
    let stats = StatsCollector::new();
    let (recovery_events, recovery_trigger_tx) = RuntimeRecoveryEvents::spawn();
    let relay_state = match failover_audit {
        Some(audit) => {
            let actor = NodeId::from_public_key(&relay_identity.public_key());
            let sink = spawn_relay_failover_audit_worker(
                audit,
                node_state.clone(),
                actor,
                network_id.clone(),
            );
            RelayState::with_recovery_and_failover_audit(recovery_trigger_tx.clone(), sink)
        }
        None => RelayState::with_recovery_trigger_sender(recovery_trigger_tx.clone()),
    };
    let outbound = OutboundDispatcher::new(
        Arc::new(P2pOutboundTransport::new(
            p2p.clone(),
            signaling.clone(),
            stats.clone(),
            relay_state.clone(),
            relay_selection.clone(),
        )),
        relay_state,
    );
    if let Some(relay) = relay {
        if let Err(relay) = outbound
            .install_relay(relay.client, relay.relay_id, relay.provider_node_id)
            .await
        {
            relay.close().await;
        }
    }
    let (control_tx, control_rx) = mpsc::unbounded_channel();

    tracing::info!("性能监控已启用");

    let tasks = spawn_runtime_tasks_ext(
        config_path,
        &config,
        &p2p,
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

    let tun_io = RuntimeTunIo::new(&config, &assigned_ips, tun, &stats);
    #[cfg(target_os = "linux")]
    let relay_recovery_path = if outbound.relay_handle().await.is_some() {
        PathRecoveryState::Connected
    } else {
        PathRecoveryState::Disconnected
    };
    Ok(ClientRuntime {
        signaling,
        p2p,
        identity: relay_identity,
        network_id,
        outbound,
        relay_selection,
        #[cfg(target_os = "linux")]
        route_manager,
        #[cfg(target_os = "linux")]
        exit_forwarder,
        #[cfg(target_os = "linux")]
        peer_route_manager,
        #[cfg(target_os = "linux")]
        exit_drain_deadline: None,
        local_exit_runtime_problem: None,
        recovery_events,
        recovery_trigger_tx,
        #[cfg(target_os = "linux")]
        exit_recovery_path: PathRecoveryState::Unknown,
        #[cfg(target_os = "linux")]
        relay_recovery_path,
        #[cfg(target_os = "linux")]
        exit_connection_watchdog: None,
        #[cfg(target_os = "linux")]
        pending_exit_readiness: None,
        tun_io,
        assigned_ip,
        assigned_ips,
        stats,
        acl,
        route_engine,
        exit_node_manager,
        exit_contribution_gate,
        exit_selection_gate,
        exit_selection_blocked: false,
        configured_exit_node,
        heartbeat_interval,
        config_path: config_path.to_string(),
        config,
        local_exit_enabled,
        control_rx,
        node_state,
        node_state_updates: None,
        traffic_session: TrafficSessionSnapshot::default(),
        local_exit_snapshot: None,
        local_exit_availability_report_interval: {
            let mut interval = tokio::time::interval(EXIT_AVAILABILITY_REPORT_INTERVAL);
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            interval
        },
        exit_contribution_member_refresh_task: None,
        exit_contribution_directory_updates: None,
        ipc_task: Some(tasks.ipc_task),
        config_reload_task: Some(tasks.config_reload_task),
        relay_reconnect_task: None,
        relay_grant_renewal_task: None,
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

struct RuntimeTasks {
    ipc_task: JoinHandle<()>,
    config_reload_task: JoinHandle<()>,
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
    RuntimeTasks {
        ipc_task,
        config_reload_task,
    }
}

#[allow(dead_code)]
fn spawn_runtime_tasks(
    config_path: &str,
    config: &ClientConfig,
    p2p: &P2pEngine,
    stats: &StatsCollector,
    assigned_ip: Ipv6Addr,
) -> RuntimeTasks {
    spawn_runtime_tasks_ext(config_path, config, p2p, stats, assigned_ip, true, None)
}

#[derive(Debug, Clone)]
struct RelayAttemptState {
    health: RelayHealthStatus,
    consecutive_failures: u32,
    last_attempt_at_unix_ms: Option<u64>,
    latency_ms: Option<u32>,
}

#[derive(Debug, Default)]
struct RelaySelectionTracker {
    states: HashMap<String, RelayAttemptState>,
}

impl RelaySelectionTracker {
    fn relay_id(endpoint: &RelayEndpoint) -> Option<String> {
        let provider_node_id = endpoint.provider_node_id.as_ref()?;
        Some(format!("{provider_node_id}@{}", endpoint.addr))
    }

    fn candidate(
        &self,
        relay_id: &str,
        now_unix_ms: u64,
        policy: &RelaySelectionPolicy,
    ) -> RelayCandidate {
        let state = self.states.get(relay_id);
        let mut health = state
            .map(|state| state.health)
            .unwrap_or(RelayHealthStatus::Degraded);
        let mut consecutive_failures = state
            .map(|state| state.consecutive_failures)
            .unwrap_or_default();
        let mut last_attempt_at_unix_ms = state.and_then(|state| state.last_attempt_at_unix_ms);

        // A failed path receives one bounded recovery probe after its cooldown.
        // Until that probe succeeds it remains degraded, never healthy.
        if consecutive_failures > 0
            && last_attempt_at_unix_ms.is_some_and(|last_attempt| {
                now_unix_ms.saturating_sub(last_attempt) >= policy.switch_cooldown_ms
            })
        {
            health = RelayHealthStatus::Degraded;
            consecutive_failures = 0;
            last_attempt_at_unix_ms = None;
        }

        RelayCandidate {
            relay_id: relay_id.to_string(),
            authorized: true,
            health,
            // The endpoint list was observed now. The health value remains
            // conservative and records only connection/forwarding evidence.
            observed_at_unix_ms: now_unix_ms,
            // No signed load observation is currently transported with relay
            // endpoints. Unknown load sorts behind paths that have observable
            // latency instead of being reported as an idle Relay.
            load_percent: u8::MAX,
            latency_ms: state.and_then(|state| state.latency_ms),
            consecutive_failures,
            last_attempt_at_unix_ms,
            loop_detected: false,
        }
    }

    fn mark_connected(&mut self, relay_id: &str, latency_ms: u32) {
        let state = self
            .states
            .entry(relay_id.to_string())
            .or_insert_with(|| RelayAttemptState {
                health: RelayHealthStatus::Degraded,
                consecutive_failures: 0,
                last_attempt_at_unix_ms: None,
                latency_ms: None,
            });
        // Handshake and registration prove reachability and authorization, but
        // not that overlay packets have been forwarded.
        state.health = RelayHealthStatus::Degraded;
        state.consecutive_failures = 0;
        state.last_attempt_at_unix_ms = None;
        state.latency_ms = Some(latency_ms);
    }

    fn mark_forwarding_success(&mut self, relay_id: &str) {
        let state = self
            .states
            .entry(relay_id.to_string())
            .or_insert_with(|| RelayAttemptState {
                health: RelayHealthStatus::Healthy,
                consecutive_failures: 0,
                last_attempt_at_unix_ms: None,
                latency_ms: None,
            });
        state.health = RelayHealthStatus::Healthy;
        state.consecutive_failures = 0;
        state.last_attempt_at_unix_ms = None;
    }

    fn mark_failure(&mut self, relay_id: &str, now_unix_ms: u64) {
        let state = self
            .states
            .entry(relay_id.to_string())
            .or_insert_with(|| RelayAttemptState {
                health: RelayHealthStatus::Unavailable,
                consecutive_failures: 0,
                last_attempt_at_unix_ms: None,
                latency_ms: None,
            });
        state.health = RelayHealthStatus::Unavailable;
        state.consecutive_failures = state.consecutive_failures.saturating_add(1);
        state.last_attempt_at_unix_ms = Some(now_unix_ms);
    }
}

#[derive(Clone)]
struct RelayConnectCandidate {
    endpoint: RelayEndpoint,
    grant: RelayMemberGrant,
    candidate: RelayCandidate,
}

struct ConnectedRelay {
    client: QuicRelayClient,
    relay_id: String,
    provider_node_id: NodeId,
}

fn relay_connect_candidates(
    tracker: &RelaySelectionTracker,
    endpoints: &[RelayEndpoint],
    grants: &[RelayMemberGrant],
    identity: &Identity,
    now_unix_ms: u64,
    policy: &RelaySelectionPolicy,
) -> Vec<RelayConnectCandidate> {
    let consumer_node_id = NodeId::from_public_key(&identity.public_key());
    endpoints
        .iter()
        .filter_map(|endpoint| {
            let grant = relay_grant_for_endpoint(grants, identity, endpoint)?.clone();
            if grant.consumer_node_id != consumer_node_id {
                return None;
            }
            let relay_id = RelaySelectionTracker::relay_id(endpoint)?;
            let candidate = tracker.candidate(&relay_id, now_unix_ms, policy);
            Some(RelayConnectCandidate {
                endpoint: endpoint.clone(),
                grant,
                candidate,
            })
        })
        .collect()
}

async fn connect_quic_relay(
    signaling: &SignalingClient,
    identity: &Identity,
    assigned_ip: Ipv6Addr,
    network_id: Option<&NetworkId>,
    node_snapshot: Option<&NodeSnapshot>,
    tracker: &RwLock<RelaySelectionTracker>,
) -> Option<ConnectedRelay> {
    let endpoints = signaling.relay_endpoints().await;
    if endpoints.is_empty() {
        tracing::debug!("协调端未下发 QUIC 中继 endpoint，使用信令中继 fallback");
        return None;
    }

    let now_unix_ms = unix_time_ms();
    let grants = match (network_id, node_snapshot) {
        (Some(network_id), Some(snapshot)) => {
            issue_relay_member_grants(&endpoints, identity, network_id, snapshot, now_unix_ms)
        }
        _ => {
            tracing::warn!(
                diagnostic_id = "relay.grant.authorization_context_missing",
                "缺少可信本地策略或网络上下文，拒绝连接 QUIC 中继"
            );
            Vec::new()
        }
    };
    if grants.is_empty() {
        tracing::debug!("没有可用的 Relay 使用方签名授权，使用信令中继 fallback");
        return None;
    }

    let policy = RelaySelectionPolicy {
        max_attempts: RELAY_SELECTION_MAX_ATTEMPTS,
        max_health_age_ms: RELAY_SELECTION_HEALTH_MAX_AGE_MS,
        max_consecutive_failures: RELAY_SELECTION_MAX_CONSECUTIVE_FAILURES,
        allow_fallback: true,
        switch_cooldown_ms: RELAY_SELECTION_SWITCH_COOLDOWN_MS,
    };
    let (connect_candidates, outcome) = {
        let tracker = tracker.read().await;
        let connect_candidates = relay_connect_candidates(
            &tracker,
            &endpoints,
            &grants,
            identity,
            now_unix_ms,
            &policy,
        );
        let outcome = match select_relay_candidate(
            &connect_candidates
                .iter()
                .map(|candidate| candidate.candidate.clone())
                .collect::<Vec<_>>(),
            &policy,
            now_unix_ms,
        ) {
            Ok(outcome) => outcome,
            Err(error) => {
                tracing::warn!(
                    diagnostic_id = "relay.selection.invalid_candidate",
                    error = %error,
                    "Relay 候选选择失败，使用信令中继 fallback"
                );
                return None;
            }
        };
        (connect_candidates, outcome)
    };

    if !outcome.is_usable() {
        let denied = outcome
            .denied
            .iter()
            .map(|denial| format!("{}={:?}", denial.relay_id, denial.reason))
            .collect::<Vec<_>>()
            .join(",");
        tracing::warn!(
            diagnostic_id = %outcome.reason,
            denied = %denied,
            "没有满足授权、健康、冷却和环路约束的 QUIC Relay 候选，使用信令中继 fallback"
        );
        return None;
    }

    let attempts_allowed = outcome.attempts_allowed.max(1) as usize;
    for (attempt_index, selected) in outcome.candidates.iter().take(attempts_allowed).enumerate() {
        let Some(connect_candidate) = connect_candidates
            .iter()
            .find(|candidate| candidate.candidate.relay_id == selected.relay_id)
        else {
            tracing::warn!(
                diagnostic_id = "relay.selection.candidate_mapping_lost",
                relay_id = %selected.relay_id,
                "Relay 候选映射失效，停止本次选路并降级"
            );
            return None;
        };
        let Some(provider_node_id) = connect_candidate.endpoint.provider_node_id.clone() else {
            tracing::warn!(
                diagnostic_id = "relay.selection.provider_identity_missing",
                relay_id = %selected.relay_id,
                "Relay 候选缺少提供方身份，停止本次选路并降级"
            );
            return None;
        };
        let started_at = Instant::now();
        let result = tokio::time::timeout(
            RELAY_CONNECT_ATTEMPT_TIMEOUT,
            QuicRelayClient::connect(
                connect_candidate.endpoint.addr,
                connect_candidate.endpoint.public_key_pin,
                &connect_candidate.endpoint.server_name,
                identity,
                assigned_ip,
                &connect_candidate.grant,
            ),
        )
        .await;
        let completed_at_unix_ms = unix_time_ms();
        match result {
            Ok(Ok(client)) => {
                let latency_ms =
                    u32::try_from(started_at.elapsed().as_millis()).unwrap_or(u32::MAX);
                tracker
                    .write()
                    .await
                    .mark_connected(&selected.relay_id, latency_ms);
                tracing::info!(
                    diagnostic_id = "relay.selection.connected",
                    relay_id = %selected.relay_id,
                    address = %connect_candidate.endpoint.addr,
                    attempt = attempt_index + 1,
                    attempts_allowed,
                    latency_ms,
                    "已连接 QUIC 中继；在真实转发验证前保持 degraded"
                );
                return Some(ConnectedRelay {
                    client,
                    relay_id: selected.relay_id.clone(),
                    provider_node_id,
                });
            }
            Ok(Err(error)) => {
                tracker
                    .write()
                    .await
                    .mark_failure(&selected.relay_id, completed_at_unix_ms);
                tracing::warn!(
                    diagnostic_id = "relay.selection.attempt_failed",
                    relay_id = %selected.relay_id,
                    address = %connect_candidate.endpoint.addr,
                    attempt = attempt_index + 1,
                    attempts_allowed,
                    error = %error,
                    "QUIC Relay 连接失败，按显式回退策略尝试下一候选"
                );
            }
            Err(_) => {
                tracker
                    .write()
                    .await
                    .mark_failure(&selected.relay_id, completed_at_unix_ms);
                tracing::warn!(
                    diagnostic_id = "relay.selection.attempt_timeout",
                    relay_id = %selected.relay_id,
                    address = %connect_candidate.endpoint.addr,
                    attempt = attempt_index + 1,
                    attempts_allowed,
                    timeout = ?RELAY_CONNECT_ATTEMPT_TIMEOUT,
                    "QUIC Relay 连接超时，按显式回退策略尝试下一候选"
                );
            }
        }
    }

    tracing::warn!(
        diagnostic_id = "relay.selection.exhausted",
        attempts_allowed,
        "QUIC Relay 候选已在有限预算内耗尽，使用信令中继 fallback"
    );
    None
}

#[derive(Debug, Clone, Copy)]
struct RelayGrantAuthorization {
    role: NetworkRole,
    revision: u64,
}

fn relay_grant_authorization(
    snapshot: &NodeSnapshot,
    identity: &Identity,
    network_id: &NetworkId,
    now_unix_ms: u64,
) -> Option<RelayGrantAuthorization> {
    let node_id = NodeId::from_public_key(&identity.public_key());
    let membership = snapshot.memberships.iter().find(|membership| {
        membership.network_id == network_id.as_str() && membership.node_id == node_id.as_str()
    })?;
    if !membership.state.can_access_network_resources() {
        tracing::warn!(
            diagnostic_id = "relay.grant.membership_inactive",
            state = ?membership.state,
            "本机成员关系不允许访问网络资源，拒绝签发 Relay 使用授权"
        );
        return None;
    }

    let local_policy = snapshot.access_control.local_policy.as_ref()?;
    if local_policy.node_id != node_id {
        tracing::warn!(
            diagnostic_id = "relay.grant.local_policy_identity_mismatch",
            "本机 Relay 使用授权策略属于其他节点，拒绝签发"
        );
        return None;
    }
    if !local_policy.network_scope.allows(network_id) {
        tracing::warn!(
            diagnostic_id = "relay.grant.network_out_of_scope",
            "本机 Relay 使用授权策略不允许当前网络，拒绝签发"
        );
        return None;
    }
    if let Err(error) = local_policy.validate_at(now_unix_ms) {
        tracing::warn!(
            diagnostic_id = "relay.grant.local_policy_invalid",
            error = %error,
            "本机 Relay 使用授权策略无效或已过期，拒绝签发"
        );
        return None;
    }

    let request = PermissionRequest {
        action: PolicyAction::Use,
        resource: ResourceRef {
            kind: ResourceKind::Relay,
            network_id: network_id.clone(),
            object_id: Some("relay".into()),
        },
        intent: PermissionIntent::Mutating,
    };
    let context = AccessContext {
        network_id: network_id.clone(),
        actor: node_id,
        role: match membership.role {
            MembershipRole::Owner => NetworkRole::Owner,
            MembershipRole::Administrator => NetworkRole::Administrator,
            MembershipRole::Member => NetworkRole::Member,
        },
        groups: Default::default(),
        membership_active: true,
        recent_reauthentication: false,
        emergency_coordinator: false,
    };
    let decision = evaluate_effective_access(
        &[],
        &[],
        None,
        Some(local_policy),
        &request,
        &context,
        now_unix_ms,
    );
    if !decision.allowed {
        tracing::warn!(
            diagnostic_id = "relay.grant.local_policy_denied",
            code = ?decision.code,
            "本机 Relay 使用授权策略未明确允许 Relay Use，拒绝签发"
        );
        return None;
    }

    Some(RelayGrantAuthorization {
        role: context.role,
        revision: local_policy
            .revision
            .max(membership.local_policy_revision)
            .max(membership.network_policy_revision.unwrap_or(0))
            .max(1),
    })
}

fn issue_relay_member_grants(
    endpoints: &[RelayEndpoint],
    identity: &Identity,
    network_id: &NetworkId,
    snapshot: &NodeSnapshot,
    now_unix_ms: u64,
) -> Vec<RelayMemberGrant> {
    let Some(authorization) =
        relay_grant_authorization(snapshot, identity, network_id, now_unix_ms)
    else {
        return Vec::new();
    };
    if authorization.role == NetworkRole::TemporaryCoordinator {
        tracing::warn!(
            diagnostic_id = "relay.grant.temporary_coordinator_denied",
            "临时 Coordinator 不能签发扩大既有成员授权的 Relay 使用授权"
        );
        return Vec::new();
    }

    let expires_at_unix_ms = now_unix_ms
        .saturating_add(u64::try_from(RELAY_MEMBER_GRANT_TTL.as_millis()).unwrap_or(u64::MAX));
    let mut grants = Vec::new();
    for endpoint in endpoints {
        let Some(provider_node_id) = endpoint.provider_node_id.clone() else {
            tracing::warn!(
                diagnostic_id = "relay.grant.provider_identity_missing",
                address = %endpoint.addr,
                "Relay endpoint 缺少提供方身份，拒绝签发授权"
            );
            continue;
        };
        let mut nonce = [0u8; 16];
        if let Err(error) = getrandom::fill(&mut nonce) {
            tracing::error!(%error, "无法生成 Relay 使用授权 nonce");
            continue;
        }
        match RelayMemberGrant::new(
            network_id.clone(),
            provider_node_id,
            identity,
            hex_encode(&nonce),
            authorization.revision,
            now_unix_ms,
            expires_at_unix_ms,
            RelayLimitPolicy::default(),
        ) {
            Ok(grant) => grants.push(grant),
            Err(error) => tracing::warn!(
                diagnostic_id = "relay.grant.sign_failed",
                error = %error,
                "无法签发 Relay 使用授权"
            ),
        }
    }
    grants
}

fn relay_grant_for_endpoint<'a>(
    grants: &'a [RelayMemberGrant],
    identity: &Identity,
    endpoint: &RelayEndpoint,
) -> Option<&'a RelayMemberGrant> {
    let provider_node_id = endpoint.provider_node_id.as_ref()?;
    let consumer_node_id = NodeId::from_public_key(&identity.public_key());
    grants.iter().find(|grant| {
        &grant.provider_node_id == provider_node_id && grant.consumer_node_id == consumer_node_id
    })
}

fn relay_grant_renewal_due(expires_at_unix_ms: u64, now_unix_ms: u64) -> bool {
    let renew_ahead_ms =
        u64::try_from(RELAY_MEMBER_GRANT_RENEW_AHEAD.as_millis()).unwrap_or(u64::MAX);
    now_unix_ms.saturating_add(renew_ahead_ms) >= expires_at_unix_ms
}

fn renewed_relay_grant<'a>(
    grants: &'a [RelayMemberGrant],
    current: &RelayMemberGrant,
) -> Option<&'a RelayMemberGrant> {
    grants.iter().find(|grant| {
        grant.network_id == current.network_id
            && grant.provider_node_id == current.provider_node_id
            && grant.consumer_node_id == current.consumer_node_id
            && grant.issued_at_unix_ms >= current.issued_at_unix_ms
            && grant.expires_at_unix_ms > current.expires_at_unix_ms
            && grant.nonce != current.nonce
    })
}

pub(crate) type DisconnectReply = tokio::sync::oneshot::Sender<std::result::Result<(), String>>;

#[cfg(target_os = "linux")]
async fn refresh_linux_recovery_paths(runtime: &mut ClientRuntime) {
    runtime.exit_recovery_path = match runtime.route_engine.routing_mode() {
        fuckipv6_common::RoutingMode::MeshOnly => PathRecoveryState::Unknown,
        fuckipv6_common::RoutingMode::BypassMainland
        | fuckipv6_common::RoutingMode::GlobalProxy => {
            if let Some(exit_node_id) = runtime.intended_exit() {
                let exit_ip = runtime
                    .p2p
                    .resolve_exit_peer_by_node_id(&exit_node_id)
                    .await;
                let established = match exit_ip {
                    Some(exit_ip) => {
                        runtime
                            .p2p
                            .has_established_session_by_ip(IpAddr::V6(exit_ip))
                            .await
                    }
                    None => false,
                };
                exit_path_from_session_evidence(exit_ip, established)
            } else {
                PathRecoveryState::Disconnected
            }
        }
    };
    runtime.relay_recovery_path = if runtime.outbound.relay_handle().await.is_some() {
        PathRecoveryState::Connected
    } else {
        PathRecoveryState::Disconnected
    };
}

#[cfg(target_os = "linux")]
fn exit_path_from_session_evidence(
    exit_ip: Option<Ipv6Addr>,
    session_established: bool,
) -> PathRecoveryState {
    if exit_ip.is_some() && session_established {
        PathRecoveryState::Connected
    } else {
        PathRecoveryState::Disconnected
    }
}

#[cfg(target_os = "linux")]
async fn drive_exit_connection_watchdog(runtime: &mut ClientRuntime) {
    let Some(watchdog_target) = runtime
        .exit_connection_watchdog
        .as_ref()
        .map(|watchdog| watchdog.target.clone())
    else {
        return;
    };
    let Some(intended_exit) = runtime.intended_exit() else {
        runtime.exit_connection_watchdog = None;
        return;
    };
    if watchdog_target != intended_exit
        || (runtime.exit_selection_gate.pending_exit().is_none()
            && runtime.traffic_session.state.may_carry_traffic())
        || runtime.traffic_session.state == TrafficProtectionState::FailClosed
    {
        runtime.exit_connection_watchdog = None;
        return;
    }

    let exit_ip = runtime
        .p2p
        .resolve_exit_peer_by_node_id(&intended_exit)
        .await;
    let session_established = match exit_ip {
        Some(exit_ip) => {
            runtime
                .p2p
                .has_established_session_by_ip(IpAddr::V6(exit_ip))
                .await
        }
        None => false,
    };
    runtime.exit_recovery_path = if session_established {
        PathRecoveryState::Connected
    } else {
        PathRecoveryState::Disconnected
    };

    let now_unix_ms = unix_time_ms();
    let action = runtime
        .exit_connection_watchdog
        .as_mut()
        .expect("watchdog was checked above")
        .observe(now_unix_ms, session_established);
    match action {
        ExitConnectionWatchdogAction::Wait => {}
        ExitConnectionWatchdogAction::Validate(trigger) => {
            if !runtime.recovery_trigger_tx.try_trigger(trigger) {
                tracing::warn!(
                    diagnostic_id = "recovery.trigger_queue_unavailable",
                    trigger = ?trigger,
                    "Exit 已建立 Secure Session，但恢复验证无法排队"
                );
                runtime
                    .enter_fail_closed(
                        TrafficFailureKind::ConnectionTimeout,
                        now_unix_ms,
                        "traffic.session.validation_schedule_failed",
                    )
                    .await;
            }
        }
        ExitConnectionWatchdogAction::TimedOut => {
            tracing::warn!(
                exit = %intended_exit,
                timeout_secs = runtime.config.p2p.connect_timeout,
                "Exit 未在连接截止时间内完成 Secure Session，请求失败关闭恢复"
            );
            if !runtime
                .recovery_trigger_tx
                .try_trigger(RecoveryTrigger::ExitConnectionTimeout)
            {
                tracing::warn!(
                    diagnostic_id = "recovery.trigger_queue_unavailable",
                    "Exit 连接超时恢复无法排队，直接失败关闭"
                );
                runtime
                    .enter_fail_closed(
                        TrafficFailureKind::ConnectionTimeout,
                        now_unix_ms,
                        "traffic.session.connection_timeout_schedule_failed",
                    )
                    .await;
            }
        }
    }
}

#[cfg(not(target_os = "linux"))]
async fn drive_exit_connection_watchdog(runtime: &mut ClientRuntime) {
    let _ = runtime;
}

#[cfg(target_os = "linux")]
async fn current_linux_recovery_route_context(
    runtime: &ClientRuntime,
) -> TrafficFailureRouteContext {
    let mode = runtime.route_engine.routing_mode();
    if mode == fuckipv6_common::RoutingMode::MeshOnly {
        return linux_recovery_route_context(mode, None, None);
    }
    let Some(selected_exit_id) = runtime.intended_exit() else {
        return TrafficFailureRouteContext::new(TrafficAction::Reject);
    };
    let fixed_exit = runtime
        .exit_selection_gate
        .resolution(unix_time_ms())
        .ok()
        .and_then(|resolution| resolution.fixed_exit);
    linux_recovery_route_context(mode, Some(&selected_exit_id), fixed_exit.as_ref())
}

#[cfg(target_os = "linux")]
fn invalidate_linux_recovery_path(
    trigger: RecoveryTrigger,
    exit_path: &mut PathRecoveryState,
    relay_path: &mut PathRecoveryState,
) {
    match trigger {
        RecoveryTrigger::ExitDisconnected
        | RecoveryTrigger::ExitConnectionTimeout
        | RecoveryTrigger::ExitMigration => {
            *exit_path = PathRecoveryState::Disconnected;
        }
        RecoveryTrigger::RelayDisconnected | RecoveryTrigger::HandshakeInterrupted => {
            *relay_path = PathRecoveryState::Disconnected;
        }
        RecoveryTrigger::RuntimeDisconnected => {}
        RecoveryTrigger::SleepWake | RecoveryTrigger::NetworkChanged | RecoveryTrigger::Manual => {}
    }
}

#[cfg(target_os = "linux")]
fn linux_recovery_route_context(
    mode: fuckipv6_common::RoutingMode,
    selected_exit: Option<&NodeId>,
    fixed_exit: Option<&NodeId>,
) -> TrafficFailureRouteContext {
    if mode == fuckipv6_common::RoutingMode::MeshOnly {
        return TrafficFailureRouteContext::new(TrafficAction::Direct);
    }
    match selected_exit {
        Some(exit_node_id) => TrafficFailureRouteContext {
            requested_action: TrafficAction::Exit(exit_node_id.clone()),
            fixed_exit: fixed_exit.cloned(),
            fixed_network: None,
        },
        None => TrafficFailureRouteContext::new(TrafficAction::Reject),
    }
}

#[cfg(target_os = "linux")]
fn linux_recovery_expectations(
    mode: fuckipv6_common::RoutingMode,
    global_proxy_active: bool,
) -> RecoveryExpectations {
    let mesh_only = mode == fuckipv6_common::RoutingMode::MeshOnly;
    RecoveryExpectations {
        routes_required: !mesh_only,
        dns_required: !mesh_only && global_proxy_active,
        exit_path_required: !mesh_only,
        relay_path_required: false,
        ..RecoveryExpectations::default()
    }
}

fn linux_recovery_request(
    mode: fuckipv6_common::RoutingMode,
    global_proxy_active: bool,
    trigger: RecoveryTrigger,
    route_context: TrafficFailureRouteContext,
) -> RecoveryRequest {
    RecoveryRequest::new(trigger, route_context, TrafficFailurePolicy::fail_closed())
        .with_expectations(linux_recovery_expectations(mode, global_proxy_active))
}

#[cfg(target_os = "linux")]
fn linux_recovery_audit_root(config_path: &str) -> std::path::PathBuf {
    Path::new(config_path)
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf()
}

#[cfg(target_os = "linux")]
fn log_linux_recovery_report(report: &RecoveryReport) {
    let failed_probe = report
        .probes
        .iter()
        .find(|probe| probe.failed_stage.is_some() || probe.error_code.is_some());
    let failed_probe_stage = failed_probe.and_then(|probe| probe.failed_stage);
    let probe_error_code = failed_probe
        .and_then(|probe| probe.error_code.as_deref())
        .unwrap_or("none");
    if report.is_success() {
        tracing::info!(
            diagnostic_id = %report.diagnostic_id,
            trigger = ?report.trigger,
            status = ?report.status,
            duration_ms = report.duration_ms(),
            error_code = report.error_code.as_deref().unwrap_or("none"),
            tun_recreated = report.repair.tun_recreated,
            routes_restored = report.repair.routes_restored,
            residual_routes_removed = report.repair.residual_routes_removed,
            duplicate_routes_removed = report.repair.duplicate_routes_removed,
            stale_interface_routes_removed = report.repair.stale_interface_routes_removed,
            dns_reconfigured = report.repair.dns_reconfigured,
            failed_probe_stage = ?failed_probe_stage,
            probe_error_code,
            "Linux 网络自动恢复完成"
        );
    } else {
        tracing::warn!(
            diagnostic_id = %report.diagnostic_id,
            trigger = ?report.trigger,
            status = ?report.status,
            duration_ms = report.duration_ms(),
            error_code = report.error_code.as_deref().unwrap_or("none"),
            tun_recreated = report.repair.tun_recreated,
            routes_restored = report.repair.routes_restored,
            residual_routes_removed = report.repair.residual_routes_removed,
            duplicate_routes_removed = report.repair.duplicate_routes_removed,
            stale_interface_routes_removed = report.repair.stale_interface_routes_removed,
            dns_reconfigured = report.repair.dns_reconfigured,
            failed_probe_stage = ?failed_probe_stage,
            probe_error_code,
            "Linux 网络自动恢复未完全成功，失效路径保持失败关闭"
        );
    }
}

#[cfg(target_os = "linux")]
fn recovery_started_reason_code(trigger: RecoveryTrigger) -> &'static str {
    match trigger {
        RecoveryTrigger::SleepWake => "recovery.sleep_wake.started",
        RecoveryTrigger::NetworkChanged => "recovery.network_changed.started",
        RecoveryTrigger::ExitDisconnected => "recovery.exit_disconnected.started",
        RecoveryTrigger::ExitConnectionTimeout => "recovery.connection_timeout.started",
        RecoveryTrigger::ExitMigration => "recovery.exit_migration.started",
        RecoveryTrigger::RelayDisconnected => "recovery.relay_disconnected.started",
        RecoveryTrigger::HandshakeInterrupted => "recovery.handshake_interrupted.started",
        RecoveryTrigger::RuntimeDisconnected => "recovery.runtime_disconnected.started",
        RecoveryTrigger::Manual => "recovery.manual.started",
    }
}

#[cfg(target_os = "linux")]
fn recovery_failure_reason_code(trigger: RecoveryTrigger) -> &'static str {
    match trigger {
        RecoveryTrigger::SleepWake => "recovery.sleep_wake.failed_closed",
        RecoveryTrigger::NetworkChanged => "recovery.network_changed.failed_closed",
        RecoveryTrigger::ExitDisconnected => "recovery.exit_unavailable.failed_closed",
        RecoveryTrigger::ExitConnectionTimeout => "recovery.connection_timeout.failed_closed",
        RecoveryTrigger::ExitMigration => "recovery.exit_migration.failed_closed",
        RecoveryTrigger::RelayDisconnected => "recovery.relay_disconnected.failed_closed",
        RecoveryTrigger::HandshakeInterrupted => "recovery.handshake_interrupted.failed_closed",
        RecoveryTrigger::RuntimeDisconnected => "recovery.runtime_disconnected.failed_closed",
        RecoveryTrigger::Manual => "recovery.validation.failed_closed",
    }
}

#[cfg(target_os = "linux")]
fn recovery_failure_kind(trigger: RecoveryTrigger, report: &RecoveryReport) -> TrafficFailureKind {
    report
        .failure_decisions
        .last()
        .map(|decision| decision.failure)
        .or_else(|| trigger.failure_kind())
        .or_else(|| {
            let code = report.error_code.as_deref().unwrap_or_default();
            if code.contains("dns") {
                Some(TrafficFailureKind::DnsResolutionTimeout)
            } else if code.contains("exit") {
                Some(TrafficFailureKind::ExitUnavailable)
            } else if code.contains("relay") {
                Some(TrafficFailureKind::NetworkSelectionUnavailable)
            } else {
                None
            }
        })
        .unwrap_or(TrafficFailureKind::ConnectionTimeout)
}

#[cfg(target_os = "linux")]
async fn run_linux_local_resource_recovery(
    runtime: &mut ClientRuntime,
    trigger: RecoveryTrigger,
) -> (RecoveryReport, RecoveryExpectations) {
    let mode = runtime.route_engine.routing_mode();
    let global_proxy_active = runtime.route_manager.global_proxy_active();
    let expectations = linux_recovery_expectations(mode, global_proxy_active);
    let route_context = current_linux_recovery_route_context(runtime).await;
    let request = linux_recovery_request(mode, global_proxy_active, trigger, route_context.clone())
        .with_expectations(expectations)
        .local_resources_only();
    let audit = RecoveryAuditContext {
        config_root: linux_recovery_audit_root(&runtime.config_path),
        actor: NodeId::from_public_key(&runtime.identity.public_key_bytes()),
        network_id: runtime.network_id.clone(),
        route_context,
    };
    let exit_path = runtime.exit_recovery_path;
    let relay_path = runtime.relay_recovery_path;
    let mut adapter = LinuxRecoveryAdapter::new(
        &mut runtime.tun_io,
        &mut runtime.route_manager,
        exit_path,
        relay_path,
        audit,
        std::collections::BTreeMap::new(),
    );
    let report = RecoveryCoordinator.recover(&mut adapter, request).await;
    drop(adapter);

    refresh_linux_recovery_paths(runtime).await;
    log_linux_recovery_report(&report);
    (report, expectations)
}

#[cfg(target_os = "linux")]
async fn handle_linux_recovery_trigger(runtime: &mut ClientRuntime, trigger: RecoveryTrigger) {
    invalidate_linux_recovery_path(
        trigger,
        &mut runtime.exit_recovery_path,
        &mut runtime.relay_recovery_path,
    );

    match trigger {
        // A lost Exit must first pass through the same administrator policy
        // used by packet routing. A staged replacement waits for its own
        // Secure Session and watchdog before recovery can validate it.
        RecoveryTrigger::ExitDisconnected => {
            runtime.reconcile_exit_selection(unix_time_ms()).await;
            if runtime.traffic_session.state == TrafficProtectionState::FailClosed
                || runtime.exit_selection_gate.pending_exit().is_some()
            {
                return;
            }
        }
        // The selected target never completed key confirmation. Discard it so
        // the recovery pass records a fail-closed outcome for this attempt
        // instead of silently promoting another target.
        RecoveryTrigger::ExitConnectionTimeout => {
            runtime.exit_connection_watchdog = None;
            runtime.exit_selection_gate.discard_pending_exit();
        }
        _ => {}
    }
    if matches!(
        trigger,
        RecoveryTrigger::Manual | RecoveryTrigger::ExitMigration
    ) {
        refresh_linux_recovery_paths(runtime).await;
    }
    let expected_exit = runtime.intended_exit();
    runtime.begin_exit_recovery(trigger, unix_time_ms()).await;

    let (report, expectations) = run_linux_local_resource_recovery(runtime, trigger).await;
    let local_validation = report.local_resource_validation(expectations);
    if report.status != RecoveryStatus::Recovered || local_validation.is_none() {
        runtime
            .enter_fail_closed(
                recovery_failure_kind(trigger, &report),
                unix_time_ms(),
                recovery_failure_reason_code(trigger),
            )
            .await;
        return;
    }

    let Some(target) = expected_exit else {
        runtime
            .enter_fail_closed(
                TrafficFailureKind::NetworkSelectionUnavailable,
                unix_time_ms(),
                "recovery.exit_readiness.target_missing",
            )
            .await;
        return;
    };
    if runtime.intended_exit().as_ref() != Some(&target) {
        runtime
            .enter_fail_closed(
                TrafficFailureKind::ExitUnavailable,
                unix_time_ms(),
                "recovery.exit_readiness.target_changed",
            )
            .await;
        return;
    }

    let Some(exit_ip) = runtime.p2p.resolve_exit_peer_by_node_id(&target).await else {
        runtime
            .enter_fail_closed(
                TrafficFailureKind::ExitUnavailable,
                unix_time_ms(),
                "recovery.exit_readiness.peer_missing",
            )
            .await;
        return;
    };
    if !runtime
        .p2p
        .has_established_session_by_ip(IpAddr::V6(exit_ip))
        .await
    {
        runtime
            .enter_fail_closed(
                TrafficFailureKind::ConnectionTimeout,
                unix_time_ms(),
                "recovery.exit_readiness.session_missing",
            )
            .await;
        return;
    }

    let mut nonce = [0u8; 16];
    if getrandom::fill(&mut nonce).is_err() {
        runtime
            .enter_fail_closed(
                TrafficFailureKind::ConnectionTimeout,
                unix_time_ms(),
                "recovery.exit_readiness.nonce_failed",
            )
            .await;
        return;
    }
    let targets = default_recovery_probes();
    let frame = match ExitReadinessFrame::request(nonce, targets.clone()) {
        Ok(frame) => frame,
        Err(error) => {
            tracing::error!(error = %error, "无法构建 Exit readiness 请求");
            runtime
                .enter_fail_closed(
                    TrafficFailureKind::ConnectionTimeout,
                    unix_time_ms(),
                    "recovery.exit_readiness.request_invalid",
                )
                .await;
            return;
        }
    };
    let payload = match encode_exit_readiness_frame(&frame) {
        Ok(payload) => payload,
        Err(error) => {
            tracing::error!(error = %error, "无法编码 Exit readiness 请求");
            runtime
                .enter_fail_closed(
                    TrafficFailureKind::ConnectionTimeout,
                    unix_time_ms(),
                    "recovery.exit_readiness.encode_failed",
                )
                .await;
            return;
        }
    };
    let deadline = Instant::now() + EXIT_READINESS_TIMEOUT;
    runtime.pending_exit_readiness = Some(PendingExitReadiness {
        target: target.clone(),
        nonce,
        targets,
        deadline,
    });
    let outbound = OutboundPacket::owned(
        IpAddr::V6(runtime.assigned_ip),
        IpAddr::V6(exit_ip),
        payload,
    );
    match runtime.outbound.submit(outbound) {
        OutboundSubmitOutcome::Queued => {
            tracing::info!(
                exit = %target,
                "本地资源校验完成，已向所选 Exit 发送认证 readiness 请求"
            );
        }
        OutboundSubmitOutcome::QueueFull | OutboundSubmitOutcome::WorkerUnavailable => {
            runtime.stats.record_p2p_send_error().await;
            runtime
                .enter_fail_closed(
                    TrafficFailureKind::ConnectionTimeout,
                    unix_time_ms(),
                    "recovery.exit_readiness.send_failed",
                )
                .await;
        }
    }
}

enum RuntimeCommandOutcome {
    Continue,
    Disconnect(DisconnectReply),
}

pub(crate) async fn run_client_event_loop_with_shutdown(
    runtime: &mut ClientRuntime,
    shutdown: &mut tokio::sync::watch::Receiver<bool>,
) -> Result<Option<DisconnectReply>> {
    let mut node_state_updates = runtime.node_state_updates.clone();
    let mut exit_contribution_directory_updates =
        runtime.exit_contribution_directory_updates.take();
    let mut exit_connection_watchdog = tokio::time::interval(EXIT_CONNECTION_WATCHDOG_INTERVAL);
    exit_connection_watchdog.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        let exit_drain_deadline = local_exit_drain_deadline(runtime);
        let exit_drain = wait_for_local_exit_drain(exit_drain_deadline);
        let exit_readiness_deadline = pending_exit_readiness_deadline(runtime);
        let exit_readiness = wait_for_pending_exit_readiness(exit_readiness_deadline);
        tokio::select! {
            // 从TUN设备读取数据包
            Some(packets) = runtime.tun_io.packet_rx().recv() => {
                for packet in packets {
                    handle_tun_reader_packet(runtime, packet).await;
                }
            }

            // 从P2P接收已认证明文；不会再次经过 replay window。
            Some(packets) = runtime.p2p.recv_batch(DATA_PLANE_BATCH_SIZE) => {
                handle_inbound_packet_batch(runtime, packets, "P2P packet").await;
            }

            // 处理信令事件
            Some(event) = runtime.signaling.recv_event() => {
                if let Err(error) = handle_runtime_signaling_event(runtime, event).await {
                    let error = error.context(
                        "信令重连后虚拟网络租约无法维持，客户端已失败关闭",
                    );
                    tracing::error!("{error:#}");
                    return Err(error);
                }
            }

            // Linux suspend/resume and stable network changes are consumed by
            // the main loop so recovery requests cannot accumulate in an
            // unobserved background channel.
            trigger = runtime.recovery_events.next_trigger() => {
                match trigger {
                    Some(trigger) => {
                        #[cfg(target_os = "linux")]
                        handle_linux_recovery_trigger(runtime, trigger).await;
                        #[cfg(not(target_os = "linux"))]
                        tracing::warn!(
                            trigger = ?trigger,
                            diagnostic_id = "recovery.platform_unavailable",
                            "当前平台尚未接入生产 TUN、路由和 DNS 自动恢复适配器"
                        );
                    }
                    None => tracing::warn!(
                        diagnostic_id = "recovery.linux.event_channel_closed",
                        "Linux 恢复事件通道已关闭；本次运行不再接收系统休眠或网络变化事件"
                    ),
                }
            }

            // A selected Exit is not routable until its Secure Session has
            // completed key confirmation. This bounded poll turns that fact
            // into exactly one validation trigger or one timeout recovery,
            // while packet admission remains closed in the meantime.
            _ = exit_connection_watchdog.tick() => {
                drive_exit_connection_watchdog(runtime).await;
            }

            // Membership terms and the owner-authored local policy can change
            // while the data plane is live. Reapply them before the next packet
            // admission decision.
            changed = async {
                match node_state_updates.as_mut() {
                    Some(updates) => updates.changed().await,
                    None => std::future::pending().await,
                }
            } => {
                if changed.is_ok() {
                    if let Some(updates) = node_state_updates.as_ref() {
                        let snapshot = updates.borrow().clone();
                        let local_exit_changed =
                            runtime.local_exit_snapshot.as_ref() != Some(&snapshot.local_exit);
                        runtime.local_exit_snapshot = Some(snapshot.local_exit.clone());
                        runtime.apply_exit_contribution_snapshot(&snapshot).await;
                        if local_exit_changed {
                            runtime.report_local_exit_availability("state_changed").await;
                        }
                    }
                } else {
                    node_state_updates = None;
                    runtime.exit_contribution_gate.clear_network_context();
                    runtime.exit_contribution_gate.clear_member_directory();
                    runtime.clear_exit_selection("exit.selection.state_channel_closed")
                        .await;
                }
            }

            directory_update = async {
                match exit_contribution_directory_updates.as_mut() {
                    Some(updates) => updates.recv().await,
                    None => std::future::pending().await,
                }
            } => {
                match directory_update {
                    Some(ExitContributionDirectoryUpdate {
                        directory: Some(directory),
                        received_at_unix_ms: Some(received_at_unix_ms),
                    }) => {
                        let selection_directory = directory.clone();
                        if let Err(denial) = runtime
                            .exit_contribution_gate
                            .replace_member_directory(directory, received_at_unix_ms)
                        {
                            tracing::warn!(
                                reason_code = denial.reason_code(),
                                "拒绝不匹配的成员目录，Exit 贡献授权已失败关闭"
                            );
                            runtime.exit_contribution_gate.clear_member_directory();
                        }
                        runtime
                            .replace_exit_selection_directory(
                                selection_directory,
                                received_at_unix_ms,
                            )
                            .await;
                    }
                    Some(_) => {
                        runtime.exit_contribution_gate.clear_member_directory();
                        runtime
                            .handle_exit_selection_directory_unavailable(
                                "exit.selection.directory_unavailable",
                            )
                            .await;
                    }
                    None => exit_contribution_directory_updates = None,
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
                        if let Some(relay) = runtime
                            .outbound
                            .clear_relay_after_failure(
                                generation,
                                RelayFailureReason::ReceiveStreamClosed,
                            )
                            .await
                        {
                            #[cfg(target_os = "linux")]
                            {
                                runtime.relay_recovery_path = PathRecoveryState::Disconnected;
                            }
                            tracing::warn!("QUIC 中继接收流已断开，重置中继等待自愈重连");
                            relay.client.close().await;
                        }
                    }
                }
            }

            // 本地控制接口下发的运行时命令
            Some(command) = runtime.control_rx.recv() => {
                match handle_runtime_command(runtime, command).await {
                    RuntimeCommandOutcome::Continue => {}
                    RuntimeCommandOutcome::Disconnect(reply) => return Ok(Some(reply)),
                }
            }

            // Local Exit disable is a two-phase operation: ingress is fenced
            // immediately, then this timer gives established conntrack flows a
            // bounded grace period before all owned kernel resources are
            // removed. Keeping the wait in the event loop means packet and
            // signalling processing continue while the grace period elapses.
            _ = exit_drain => complete_local_exit_drain(runtime).await,

            _ = exit_readiness => complete_pending_exit_readiness_timeout(runtime).await,

            // 定期心跳与 QUIC 中继自愈检测
            _ = runtime.heartbeat_interval.tick() => {
                if runtime
                    .relay_reconnect_task
                    .as_ref()
                    .is_some_and(JoinHandle::is_finished)
                {
                    runtime.relay_reconnect_task.take();
                    runtime.outbound.finish_relay_reconnect();
                }
                if runtime
                    .relay_grant_renewal_task
                    .as_ref()
                    .is_some_and(JoinHandle::is_finished)
                {
                    runtime.relay_grant_renewal_task.take();
                }
                if runtime.signaling.send_heartbeat().is_ok() {
                    runtime.stats.record_signaling_sent().await;
                }
                if runtime.outbound.relay_handle().await.is_none()
                    && runtime.outbound.begin_relay_reconnect().await
                {
                    runtime.relay_reconnect_task = Some(spawn_relay_reconnect(runtime));
                } else if runtime.outbound.relay_handle().await.is_some() {
                    #[cfg(target_os = "linux")]
                    {
                        runtime.relay_recovery_path = PathRecoveryState::Connected;
                    }
                }
                if let Some(relay) = runtime.outbound.relay_handle().await {
                    let current = relay.client.current_member_grant().await;
                    let renewal_idle = runtime
                        .relay_grant_renewal_task
                        .as_ref()
                        .is_none_or(JoinHandle::is_finished);
                    if renewal_idle
                        && relay_grant_renewal_due(current.expires_at_unix_ms, unix_time_ms())
                    {
                        runtime.relay_grant_renewal_task =
                            Some(spawn_relay_grant_renewal(runtime));
                    }
                }
            }

            _ = runtime.local_exit_availability_report_interval.tick() => {
                runtime.report_local_exit_availability("periodic").await;
            }

            // 监听停止信号
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    tracing::info!("收到停止信号，正在退出运行时事件循环...");
                    break;
                }
            }
        }
    }

    Ok(None)
}

fn local_exit_drain_deadline(runtime: &ClientRuntime) -> Option<Instant> {
    #[cfg(target_os = "linux")]
    {
        runtime.exit_drain_deadline
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = runtime;
        None
    }
}

async fn wait_for_local_exit_drain(deadline: Option<Instant>) {
    match deadline {
        Some(deadline) => sleep(deadline.saturating_duration_since(Instant::now())).await,
        None => std::future::pending::<()>().await,
    }
}

fn pending_exit_readiness_deadline(runtime: &ClientRuntime) -> Option<Instant> {
    #[cfg(target_os = "linux")]
    {
        runtime
            .pending_exit_readiness
            .as_ref()
            .map(|pending| pending.deadline)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = runtime;
        None
    }
}

async fn wait_for_pending_exit_readiness(deadline: Option<Instant>) {
    match deadline {
        Some(deadline) => sleep(deadline.saturating_duration_since(Instant::now())).await,
        None => std::future::pending::<()>().await,
    }
}

async fn complete_pending_exit_readiness_timeout(runtime: &mut ClientRuntime) {
    #[cfg(target_os = "linux")]
    {
        if runtime.pending_exit_readiness.is_some() {
            runtime.pending_exit_readiness = None;
            runtime
                .enter_fail_closed(
                    TrafficFailureKind::ConnectionTimeout,
                    unix_time_ms(),
                    "recovery.exit_readiness.timeout",
                )
                .await;
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = runtime;
    }
}

async fn complete_local_exit_drain(runtime: &mut ClientRuntime) {
    #[cfg(target_os = "linux")]
    finish_runtime_local_exit_drain(runtime).await;
    #[cfg(not(target_os = "linux"))]
    {
        let _ = runtime;
    }
}

async fn handle_runtime_command(
    runtime: &mut ClientRuntime,
    command: ipc::RuntimeCommand,
) -> RuntimeCommandOutcome {
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
            RuntimeCommandOutcome::Continue
        }
        ipc::RuntimeCommand::UpdateRouting {
            mode,
            exit_node,
            is_exit_node,
            reply,
        } => {
            let result = apply_runtime_routing(runtime, &mode, exit_node, is_exit_node).await;
            let _ = reply.send(result);
            RuntimeCommandOutcome::Continue
        }
        ipc::RuntimeCommand::SetLocalExitCapability { enabled, reply } => {
            let result = apply_runtime_local_exit_capability(runtime, enabled).await;
            let _ = reply.send(result);
            RuntimeCommandOutcome::Continue
        }
        ipc::RuntimeCommand::LocalExitStatus { reply } => {
            let _ = reply.send(Ok(local_exit_runtime_status(runtime)));
            RuntimeCommandOutcome::Continue
        }
        ipc::RuntimeCommand::Disconnect { reply } => {
            tracing::info!("收到本地控制接口的断开请求，等待 launcher 完成资源清理");
            RuntimeCommandOutcome::Disconnect(reply)
        }
    }
}

fn parse_cidr_list(values: &[String], field: &str) -> Result<Vec<IpNet>> {
    values
        .iter()
        .enumerate()
        .map(|(index, value)| {
            value.parse::<IpNet>().with_context(|| {
                format!(
                    "{field}[{index}] 不是有效 CIDR，拒绝启动或应用路由: {}",
                    value.trim()
                )
            })
        })
        .collect()
}

fn parse_optional_ipv6_cidr(value: Option<&str>, field: &str) -> Result<Option<Ipv6Net>> {
    value
        .map(|value| {
            value.parse::<Ipv6Net>().with_context(|| {
                format!(
                    "{field} 必须是 IPv6 CIDR，拒绝启动或应用路由: {}",
                    value.trim()
                )
            })
        })
        .transpose()
}

fn parse_default_virtual_ipv4_network() -> Result<Ipv4Net> {
    "10.8.0.0/24".parse().context("内置默认虚拟 IPv4 网络无效")
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
    Drain,
    Resume,
}

#[cfg(target_os = "linux")]
fn exit_forwarding_action(
    enabled: bool,
    has_forwarder: bool,
    draining: bool,
) -> ExitForwardingAction {
    match (enabled, has_forwarder, draining) {
        (true, false, _) => ExitForwardingAction::Install,
        (true, true, true) => ExitForwardingAction::Resume,
        (false, true, false) => ExitForwardingAction::Drain,
        _ => ExitForwardingAction::Keep,
    }
}

#[cfg(target_os = "linux")]
fn runtime_exit_manager_enabled(runtime: &ClientRuntime) -> bool {
    let effective =
        local_exit_effective(runtime.local_exit_enabled, runtime.config.exit_node.enabled);
    runtime
        .exit_forwarder
        .as_ref()
        .is_some_and(|forwarder| effective || forwarder.is_draining())
}

fn local_exit_effective(owner_enabled: bool, network_allowed: bool) -> bool {
    owner_enabled && network_allowed
}

/// Convert the authoritative local Exit projection into the only availability
/// evidence the Coordinator may expose. A configured or declared Exit is never
/// healthy until the data plane is enabled and healthy; reason codes are stable
/// and contain no host paths, ports, peer identities or diagnostics.
fn local_exit_availability_report(
    snapshot: &LocalExitCapabilitySnapshot,
) -> (bool, Option<&'static str>) {
    if !snapshot.owner_exit_enabled {
        return (false, Some("exit.availability.owner_disabled"));
    }
    if snapshot.stage == LocalExitCapabilityStage::Draining {
        return (false, Some("exit.availability.draining"));
    }
    if snapshot.stage == LocalExitCapabilityStage::Cleaning {
        return (false, Some("exit.availability.cleaning"));
    }
    if !snapshot.network_allowed {
        return (false, Some("exit.availability.network_not_allowed"));
    }
    if snapshot.stage == LocalExitCapabilityStage::Enabled
        && snapshot.healthy
        && snapshot.problem.is_none()
    {
        return (true, None);
    }
    if matches!(
        snapshot.stage,
        LocalExitCapabilityStage::Listening | LocalExitCapabilityStage::HealthChecking
    ) {
        return (false, Some("exit.availability.health_checking"));
    }
    if snapshot.stage == LocalExitCapabilityStage::WaitingForNetworkAuthorization {
        return (
            false,
            Some("exit.availability.waiting_for_network_authorization"),
        );
    }
    (false, Some("exit.availability.forwarder_unhealthy"))
}

fn repeated_local_exit_disable_is_noop(
    enabled: bool,
    owner_exit_enabled: bool,
    has_forwarder: bool,
    draining: bool,
) -> bool {
    !enabled && !owner_exit_enabled && (!has_forwarder || draining)
}

fn local_exit_runtime_stage(
    effective: bool,
    has_forwarder: bool,
    draining: bool,
    has_problem: bool,
) -> ipc::LocalExitRuntimeStage {
    if has_problem {
        ipc::LocalExitRuntimeStage::Failed
    } else if has_forwarder && draining {
        ipc::LocalExitRuntimeStage::Draining
    } else if has_forwarder && effective {
        ipc::LocalExitRuntimeStage::Enabled
    } else if !has_forwarder && !effective {
        ipc::LocalExitRuntimeStage::Disabled
    } else {
        ipc::LocalExitRuntimeStage::Failed
    }
}

fn local_exit_runtime_status(runtime: &ClientRuntime) -> ipc::LocalExitRuntimeStatus {
    let network_allowed = runtime.config.exit_node.enabled;
    let effective = local_exit_effective(runtime.local_exit_enabled, network_allowed);
    #[cfg(target_os = "linux")]
    let (has_forwarder, draining, active_clients, total_clients) = runtime
        .exit_forwarder
        .as_ref()
        .map_or((false, false, 0, 0), |forwarder| {
            (
                true,
                forwarder.is_draining(),
                forwarder.active_clients(),
                forwarder.total_clients(),
            )
        });
    #[cfg(not(target_os = "linux"))]
    let (has_forwarder, draining, active_clients, total_clients) = (false, false, 0, 0);

    let mut problem = runtime.local_exit_runtime_problem.clone();
    if problem.is_none() {
        problem = if effective && !has_forwarder {
            Some(
                NodeProblem::new(
                    NodeProblemCode::Unavailable,
                    "本机 Exit 已授权，但转发数据面未运行",
                )
                .retryable(true),
            )
        } else if has_forwarder && !effective && !draining {
            Some(
                NodeProblem::new(
                    NodeProblemCode::Unavailable,
                    "本机 Exit 已关闭，但转发数据面仍处于开放状态",
                )
                .retryable(true),
            )
        } else {
            None
        };
    }
    let stage = local_exit_runtime_stage(effective, has_forwarder, draining, problem.is_some());
    ipc::LocalExitRuntimeStatus {
        owner_exit_enabled: runtime.local_exit_enabled,
        network_allowed,
        effective,
        healthy: stage == ipc::LocalExitRuntimeStage::Enabled,
        stage,
        problem,
        active_clients,
        total_clients,
    }
}

#[cfg(target_os = "linux")]
fn local_exit_forwarder_problem_code(message: &str) -> NodeProblemCode {
    let message = message.to_ascii_lowercase();
    if [
        "permission denied",
        "operation not permitted",
        "must be root",
        "权限不足",
    ]
    .iter()
    .any(|pattern| message.contains(pattern))
        || ["os error 1)", "os error 13)"]
            .iter()
            .any(|pattern| message.contains(pattern))
    {
        NodeProblemCode::PermissionDenied
    } else if [
        "resource temporarily unavailable",
        "no space left on device",
        "cannot allocate memory",
        "out of memory",
        "too many open files",
        "file table overflow",
        "no buffer space available",
        "资源暂时不可用",
        "资源不足",
    ]
    .iter()
    .any(|pattern| message.contains(pattern))
        || [
            "os error 11)",
            "os error 12)",
            "os error 23)",
            "os error 24)",
            "os error 28)",
            "os error 105)",
        ]
        .iter()
        .any(|pattern| message.contains(pattern))
    {
        NodeProblemCode::ResourceExhausted
    } else if [
        "not supported",
        "table does not exist",
        "can't initialize iptables table",
        "no such file or directory",
        "not found",
        "不支持",
        "未找到",
    ]
    .iter()
    .any(|pattern| message.contains(pattern))
    {
        NodeProblemCode::Unsupported
    } else {
        NodeProblemCode::Unavailable
    }
}

#[cfg(target_os = "linux")]
fn local_exit_forwarder_problem(message: &str) -> NodeProblem {
    let code = local_exit_forwarder_problem_code(message);
    let retryable = matches!(
        code,
        NodeProblemCode::Unavailable | NodeProblemCode::ResourceExhausted
    );
    NodeProblem::new(code, message).retryable(retryable)
}

#[cfg(target_os = "linux")]
fn local_exit_forwarder_runtime_error(problem: &NodeProblem) -> RuntimeControlError {
    match problem.code {
        NodeProblemCode::PermissionDenied => {
            RuntimeControlError::PermissionDenied(problem.message.clone())
        }
        NodeProblemCode::Unsupported => RuntimeControlError::Unsupported(problem.message.clone()),
        NodeProblemCode::ResourceExhausted => {
            RuntimeControlError::ResourceExhausted(problem.message.clone())
        }
        _ => RuntimeControlError::Unavailable(problem.message.clone()),
    }
}

#[cfg(target_os = "linux")]
fn set_local_exit_runtime_problem(
    runtime: &mut ClientRuntime,
    message: impl Into<String>,
) -> RuntimeControlError {
    let message = message.into();
    let problem = local_exit_forwarder_problem(&message);
    let error = local_exit_forwarder_runtime_error(&problem);
    runtime.local_exit_runtime_problem = Some(problem);
    error
}

fn clear_local_exit_runtime_problem(runtime: &mut ClientRuntime) {
    runtime.local_exit_runtime_problem = None;
}

async fn apply_runtime_local_exit_capability(
    runtime: &mut ClientRuntime,
    enabled: bool,
) -> std::result::Result<ipc::LocalExitRuntimeStatus, RuntimeControlError> {
    #[cfg(target_os = "linux")]
    let (has_forwarder, draining) = runtime
        .exit_forwarder
        .as_ref()
        .map_or((false, false), |forwarder| (true, forwarder.is_draining()));
    #[cfg(not(target_os = "linux"))]
    let (has_forwarder, draining) = (false, false);

    if repeated_local_exit_disable_is_noop(
        enabled,
        runtime.local_exit_enabled,
        has_forwarder,
        draining,
    ) {
        #[cfg(target_os = "linux")]
        {
            if draining {
                runtime
                    .exit_drain_deadline
                    .get_or_insert_with(|| Instant::now() + LOCAL_EXIT_DRAIN_TIMEOUT);
            } else {
                runtime.exit_drain_deadline = None;
            }
        }
        runtime.exit_node_manager.set_enabled(draining);
        return Ok(local_exit_runtime_status(runtime));
    }

    // Persist the owner decision in memory before touching resources. Disable
    // therefore vetoes every ingress path immediately; enable still requires a
    // successfully installed data plane before the Exit manager is re-enabled.
    runtime.local_exit_enabled = enabled;
    runtime.exit_node_manager.set_enabled(false);
    let network_allowed = runtime.config.exit_node.enabled;
    let effective = local_exit_effective(enabled, network_allowed);

    #[cfg(target_os = "linux")]
    if !enabled {
        let drain_error = match runtime.exit_forwarder.as_mut() {
            Some(forwarder) => forwarder.begin_drain().await.err(),
            None => None,
        };
        if let Some(error) = drain_error {
            let rollback_error = match runtime.exit_forwarder.take() {
                Some(mut forwarder) => {
                    match forwarder.rollback_within(Duration::from_secs(4)).await {
                        Ok(()) => None,
                        Err(rollback_error) => {
                            runtime.exit_forwarder = Some(forwarder);
                            Some(rollback_error)
                        }
                    }
                }
                None => None,
            };
            runtime.exit_drain_deadline = runtime
                .exit_forwarder
                .is_some()
                .then(|| Instant::now() + LOCAL_EXIT_DRAIN_RETRY_TIMEOUT);
            runtime
                .exit_node_manager
                .set_enabled(runtime.exit_forwarder.is_some());
            return match rollback_error {
                None => Err(set_local_exit_runtime_problem(
                    runtime,
                    format!("进入 Exit Node 排空失败，已失败关闭并清理资源: {error:#}"),
                )),
                Some(rollback_error) => {
                    let message = format!(
                        "进入 Exit Node 排空失败且清理失败，可重试: {error:#}; {rollback_error:#}"
                    );
                    Err(set_local_exit_runtime_problem(runtime, message))
                }
            };
        }

        clear_local_exit_runtime_problem(runtime);
        if runtime.exit_forwarder.is_some() {
            runtime
                .exit_drain_deadline
                .get_or_insert_with(|| Instant::now() + LOCAL_EXIT_DRAIN_TIMEOUT);
        } else {
            runtime.exit_drain_deadline = None;
        }
        // Keep authorization and the forwarding data plane alive during drain so
        // established conntrack entries can finish. The drain DROP rule is the
        // authoritative fence for new flows.
        runtime
            .exit_node_manager
            .set_enabled(runtime.exit_forwarder.is_some());
    } else if effective {
        let cancel_error = match runtime.exit_forwarder.as_mut() {
            Some(forwarder) if forwarder.is_draining() => forwarder.cancel_drain().await.err(),
            _ => None,
        };
        if let Some(error) = cancel_error {
            let message = format!("取消 Exit Node 排空失败，无法重新开放数据面: {error:#}");
            runtime
                .exit_drain_deadline
                .get_or_insert_with(|| Instant::now() + LOCAL_EXIT_DRAIN_RETRY_TIMEOUT);
            return Err(set_local_exit_runtime_problem(runtime, message));
        }
        if runtime.exit_forwarder.is_none() {
            let forwarder =
                match LinuxExitForwarder::install(&runtime.config.exit_node, runtime.tun_io.name())
                    .await
                {
                    Ok(Some(forwarder)) => forwarder,
                    Ok(None) => {
                        let message = "Exit Node 网络角色已启用，但未创建内核转发器".to_string();
                        return Err(set_local_exit_runtime_problem(runtime, message));
                    }
                    Err(error) => {
                        let message = format!("安装 Exit Node 内核转发失败: {error:#}");
                        return Err(set_local_exit_runtime_problem(runtime, message));
                    }
                };
            runtime.exit_forwarder = Some(forwarder);
        }
        clear_local_exit_runtime_problem(runtime);
        runtime.exit_drain_deadline = None;
        runtime.exit_node_manager.set_enabled(true);
    } else {
        clear_local_exit_runtime_problem(runtime);
        runtime.exit_node_manager.set_enabled(false);
    }

    #[cfg(not(target_os = "linux"))]
    {
        if effective {
            let message = "当前平台不支持本机 Exit 数据面".to_string();
            runtime.local_exit_runtime_problem = Some(
                NodeProblem::new(NodeProblemCode::Unsupported, message.clone()).retryable(false),
            );
            return Err(RuntimeControlError::Unsupported(message));
        }
    }

    Ok(local_exit_runtime_status(runtime))
}

fn unix_time_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

#[cfg(target_os = "linux")]
fn local_exit_accepts_inbound(runtime: &ClientRuntime) -> bool {
    runtime.local_exit_enabled
        || runtime
            .exit_forwarder
            .as_ref()
            .is_some_and(LinuxExitForwarder::is_draining)
}

#[cfg(target_os = "linux")]
async fn finish_runtime_local_exit_drain(runtime: &mut ClientRuntime) {
    let effective =
        local_exit_effective(runtime.local_exit_enabled, runtime.config.exit_node.enabled);
    if effective {
        if let Some(forwarder) = runtime.exit_forwarder.as_mut() {
            if forwarder.is_draining() {
                if let Err(error) = forwarder.cancel_drain().await {
                    let message = format!(
                        "Exit Node 已重新获得授权，但取消排空失败，保持已有 flow 并自动重试: {error:#}"
                    );
                    set_local_exit_runtime_problem(runtime, message.clone());
                    runtime.exit_drain_deadline =
                        Some(Instant::now() + LOCAL_EXIT_DRAIN_RETRY_TIMEOUT);
                    runtime.exit_node_manager.set_enabled(true);
                    tracing::warn!("{message}");
                    return;
                }
            }
        }
        clear_local_exit_runtime_problem(runtime);
        runtime.exit_drain_deadline = None;
        runtime
            .exit_node_manager
            .set_enabled(runtime_exit_manager_enabled(runtime));
        return;
    }

    runtime.exit_drain_deadline = None;
    let Some(mut forwarder) = runtime.exit_forwarder.take() else {
        clear_local_exit_runtime_problem(runtime);
        runtime.exit_node_manager.set_enabled(false);
        return;
    };

    match forwarder.rollback_within(Duration::from_secs(4)).await {
        Ok(()) => {
            clear_local_exit_runtime_problem(runtime);
            runtime.exit_node_manager.set_enabled(false);
            tracing::info!("Exit Node 排空窗口结束，内核转发资源已清理");
        }
        Err(error) => {
            let message =
                format!("Exit Node 排空后的资源清理失败，保留排空栅栏并自动重试: {error:#}");
            runtime.exit_forwarder = Some(forwarder);
            runtime.exit_drain_deadline = Some(Instant::now() + LOCAL_EXIT_DRAIN_RETRY_TIMEOUT);
            set_local_exit_runtime_problem(runtime, message.clone());
            runtime
                .exit_node_manager
                .set_enabled(runtime_exit_manager_enabled(runtime));
            tracing::warn!("{message}");
        }
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
    let effective_exit_enabled = local_exit_effective(runtime.local_exit_enabled, is_exit_node);

    // Stage new kernel resources before changing policy routes. A disabled Exit
    // Node keeps its current resources until persistence succeeds, so a failed
    // config write can leave the old runtime completely untouched.
    #[cfg(target_os = "linux")]
    let exit_action = exit_forwarding_action(
        effective_exit_enabled,
        runtime.exit_forwarder.is_some(),
        runtime
            .exit_forwarder
            .as_ref()
            .is_some_and(LinuxExitForwarder::is_draining),
    );
    #[cfg(target_os = "linux")]
    let mut staged_exit_forwarder = if exit_action == ExitForwardingAction::Install {
        let forwarder =
            LinuxExitForwarder::install(&updated_config.exit_node, runtime.tun_io.name())
                .await
                .map_err(|error| format!("安装 Exit Node 内核转发失败: {error:#}"))?;
        Some(forwarder.ok_or_else(|| "Exit Node 配置已启用，但未创建内核转发器".to_string())?)
    } else {
        None
    };

    #[cfg(target_os = "linux")]
    if let Err(error) = apply_linux_routing_mode(runtime, parsed_mode).await {
        if let Some(mut forwarder) = staged_exit_forwarder.take() {
            let _ = forwarder.rollback();
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
            let _ = forwarder.rollback();
        }

        return match rollback_error {
            Some(rollback_error) => Err(format!(
                "持久化路由配置失败，且恢复原策略路由也失败: {error:#}; 回滚错误: {rollback_error:#}"
            )),
            None => Err(format!("持久化路由配置失败，运行态已回滚: {error:#}")),
        };
    }

    runtime.route_engine.set_routing_mode(parsed_mode);
    runtime.configured_exit_node = exit_node.clone();
    runtime.config = updated_config;
    #[cfg(target_os = "linux")]
    refresh_linux_recovery_paths(runtime).await;

    #[cfg(target_os = "linux")]
    match exit_action {
        ExitForwardingAction::Install => {
            runtime.exit_forwarder = staged_exit_forwarder.take();
            clear_local_exit_runtime_problem(runtime);
        }
        ExitForwardingAction::Drain => {
            let drain_error = match runtime.exit_forwarder.as_mut() {
                Some(forwarder) => forwarder.begin_drain().await.err(),
                None => None,
            };
            if let Some(error) = drain_error {
                let rollback_error = match runtime.exit_forwarder.take() {
                    Some(mut forwarder) => forwarder
                        .rollback_within(Duration::from_secs(4))
                        .await
                        .err()
                        .inspect(|_| {
                            runtime.exit_forwarder = Some(forwarder);
                        }),
                    None => None,
                };
                runtime.exit_drain_deadline = runtime
                    .exit_forwarder
                    .is_some()
                    .then(|| Instant::now() + LOCAL_EXIT_DRAIN_RETRY_TIMEOUT);
                runtime
                    .exit_node_manager
                    .set_enabled(runtime_exit_manager_enabled(runtime));
                return match rollback_error {
                    None => {
                        clear_local_exit_runtime_problem(runtime);
                        Err(format!(
                            "进入 Exit Node 排空失败，已失败关闭并清理资源: {error:#}"
                        ))
                    }
                    Some(rollback_error) => {
                        let message = format!(
                            "进入 Exit Node 排空失败且清理失败，可重试: {error:#}; {rollback_error:#}"
                        );
                        set_local_exit_runtime_problem(runtime, message.clone());
                        Err(message)
                    }
                };
            }
            clear_local_exit_runtime_problem(runtime);
        }
        ExitForwardingAction::Resume => {
            let cancel_error = match runtime.exit_forwarder.as_mut() {
                Some(forwarder) => forwarder.cancel_drain().await.err(),
                None => None,
            };
            if let Some(error) = cancel_error {
                let message =
                    format!("取消 Exit Node 排空失败，保持现有 flow 并等待重试: {error:#}");
                set_local_exit_runtime_problem(runtime, message.clone());
                runtime
                    .exit_drain_deadline
                    .get_or_insert_with(|| Instant::now() + LOCAL_EXIT_DRAIN_RETRY_TIMEOUT);
                runtime.exit_node_manager.set_enabled(true);
                return Err(message);
            }
            clear_local_exit_runtime_problem(runtime);
        }
        ExitForwardingAction::Keep => {}
    }

    #[cfg(target_os = "linux")]
    {
        if runtime
            .exit_forwarder
            .as_ref()
            .is_some_and(LinuxExitForwarder::is_draining)
        {
            runtime
                .exit_drain_deadline
                .get_or_insert_with(|| Instant::now() + LOCAL_EXIT_DRAIN_TIMEOUT);
        } else {
            runtime.exit_drain_deadline = None;
        }
        runtime
            .exit_node_manager
            .set_enabled(runtime_exit_manager_enabled(runtime));
    }
    #[cfg(not(target_os = "linux"))]
    runtime
        .exit_node_manager
        .set_enabled(effective_exit_enabled);

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
    if let Some(network) = parse_optional_ipv6_cidr(
        runtime.config.network.virtual_network.as_deref(),
        "network.virtual_network",
    )? {
        virtual_cidrs.push(IpNet::V6(network));
    }
    virtual_cidrs.push(IpNet::V4(parse_default_virtual_ipv4_network()?));
    let direct_cidrs =
        parse_cidr_list(&runtime.config.routing.direct_cidrs, "routing.direct_cidrs")?;

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
            runtime.tun_io.name(),
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

    let selected_exit = runtime.exit_for_packet(unix_time_ms()).await;

    let Some((src_ip, dst_ip)) = route_tun_packet(
        &runtime.p2p,
        &runtime.assigned_ips,
        &runtime.route_engine,
        selected_exit.as_ref(),
        payload,
    )
    .await
    else {
        recycle_tun_buffer(runtime, packet);
        return;
    };
    let outbound = OutboundPacket::pooled(src_ip, dst_ip, packet, runtime.tun_io.buffer_tx());

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
    let _ = runtime.tun_io.buffer_tx().send(packet.recycle());
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

#[cfg(target_os = "linux")]
async fn handle_exit_readiness_frame(
    runtime: &mut ClientRuntime,
    src_ip: IpAddr,
    peer: PeerRouteInfo,
    frame: ExitReadinessFrame,
) {
    match frame {
        ExitReadinessFrame::Response { nonce, results } => {
            handle_exit_readiness_response(runtime, peer, nonce, results).await;
        }
        ExitReadinessFrame::Request { nonce, targets } => {
            let now_unix_ms = unix_time_ms();
            if !exit_readiness_request_authorized(runtime, &peer, now_unix_ms) {
                return;
            }

            let outbound = runtime.outbound.clone();
            let stats = runtime.stats.clone();
            let response_src = IpAddr::V6(runtime.assigned_ip);
            let response_dst = IpAddr::V6(peer.virtual_ip);
            tracing::debug!(
                source = %src_ip,
                peer = %peer.node_id,
                "已接受已认证的 Exit readiness 探测请求"
            );
            tokio::spawn(async move {
                let mut results = Vec::with_capacity(targets.len());
                for target in targets {
                    results.push(probe_exit_readiness_target(target).await);
                }
                let response = match ExitReadinessFrame::response(nonce, results) {
                    Ok(response) => response,
                    Err(error) => {
                        tracing::warn!(
                            peer = %peer.node_id,
                            "Exit readiness 探测结果无法构建响应: {error}"
                        );
                        return;
                    }
                };
                let payload = match encode_exit_readiness_frame(&response) {
                    Ok(payload) => payload,
                    Err(error) => {
                        tracing::warn!(
                            peer = %peer.node_id,
                            "Exit readiness 响应无法编码: {error}"
                        );
                        return;
                    }
                };
                let packet = OutboundPacket::owned(response_src, response_dst, payload);
                match outbound.submit(packet) {
                    OutboundSubmitOutcome::Queued => {}
                    OutboundSubmitOutcome::QueueFull | OutboundSubmitOutcome::WorkerUnavailable => {
                        stats.record_p2p_send_error().await;
                        tracing::warn!(
                            peer = %peer.node_id,
                            "Exit readiness 响应发送队列不可用"
                        );
                    }
                }
            });
        }
    }
}

#[cfg(target_os = "linux")]
async fn handle_exit_readiness_response(
    runtime: &mut ClientRuntime,
    peer: PeerRouteInfo,
    nonce: [u8; 16],
    results: Vec<crate::routing::recovery::RecoveryProbeResult>,
) {
    let Some(pending) = runtime.pending_exit_readiness.clone() else {
        tracing::debug!(
            peer = %peer.node_id,
            nonce = ?nonce,
            "当前没有待匹配的 Exit readiness 请求，丢弃响应"
        );
        return;
    };
    if peer.node_id != pending.target {
        tracing::warn!(
            expected = %pending.target,
            actual = %peer.node_id,
            "Exit readiness 响应来源与待验证目标不一致，失败关闭"
        );
        runtime
            .enter_fail_closed(
                TrafficFailureKind::ExitUnavailable,
                unix_time_ms(),
                "recovery.exit_readiness.peer_mismatch",
            )
            .await;
        return;
    }

    let response = ExitReadinessFrame::Response { nonce, results };
    if response
        .validate_response(pending.nonce, &pending.targets)
        .is_err()
        || !response.all_probes_passed()
    {
        tracing::warn!(
            exit = %pending.target,
            "Exit readiness 响应不匹配或未通过全部探测，失败关闭"
        );
        runtime
            .enter_fail_closed(
                TrafficFailureKind::ExitUnavailable,
                unix_time_ms(),
                "recovery.exit_readiness.failed",
            )
            .await;
        return;
    }
    if runtime.intended_exit().as_ref() != Some(&pending.target) {
        runtime
            .enter_fail_closed(
                TrafficFailureKind::ExitUnavailable,
                unix_time_ms(),
                "recovery.exit_readiness.target_changed",
            )
            .await;
        return;
    }

    if let Some(staged) = runtime.exit_selection_gate.pending_exit().cloned() {
        let committed = runtime.exit_selection_gate.commit_pending_exit();
        if committed.as_ref() != Some(&staged) || staged != pending.target {
            runtime
                .enter_fail_closed(
                    TrafficFailureKind::ExitUnavailable,
                    unix_time_ms(),
                    "recovery.exit_migration.target_mismatch",
                )
                .await;
            return;
        }
    } else if runtime.exit_selection_gate.current_exit() != Some(&pending.target) {
        runtime
            .enter_fail_closed(
                TrafficFailureKind::ExitUnavailable,
                unix_time_ms(),
                "recovery.exit_target_changed_during_validation",
            )
            .await;
        return;
    }

    if let Err(error) = runtime.route_manager.validate_exit_data_path().await {
        tracing::error!(
            exit = %pending.target,
            error = %error,
            "Exit 自身 readiness 通过，但本机经 TUN/P2P/Exit 的 YouTube DNS/TCP/TLS 实测失败；宿主全局路由保持未启用"
        );
        runtime
            .enter_fail_closed(
                TrafficFailureKind::ExitUnavailable,
                unix_time_ms(),
                "recovery.exit_readiness.data_path_failed",
            )
            .await;
        return;
    }

    if let Err(error) = runtime.route_manager.activate_global_proxy().await {
        tracing::error!(
            exit = %pending.target,
            error = %error,
            "Exit readiness 已通过，但启用全局代理路由/DNS 失败，恢复宿主网络"
        );
        runtime
            .enter_fail_closed(
                TrafficFailureKind::ExitUnavailable,
                unix_time_ms(),
                "recovery.exit_readiness.activation_failed",
            )
            .await;
        return;
    }

    // The pre-activation recovery pass deliberately checked only base routes.
    // Re-read TUN, catch-all routes and DNS now, so Protected is never based on
    // evidence collected before host traffic was actually intercepted.
    let (report, expectations) =
        run_linux_local_resource_recovery(runtime, RecoveryTrigger::Manual).await;
    let Some(local_validation) = report.local_resource_validation(expectations) else {
        tracing::error!(
            exit = %pending.target,
            status = ?report.status,
            error_code = ?report.error_code,
            "Exit readiness 已通过，但启用后的本地资源复验失败，恢复宿主网络"
        );
        runtime
            .enter_fail_closed(
                TrafficFailureKind::ExitUnavailable,
                unix_time_ms(),
                "recovery.exit_readiness.local_resources_failed",
            )
            .await;
        return;
    };

    let validation = fuckipv6_common::domain::TrafficSessionValidation::passed(
        local_validation.tun_validated,
        local_validation.routes_validated,
        local_validation.dns_validated,
        true,
        unix_time_ms().max(local_validation.observed_at_unix_ms.max(1)),
    );
    if let Err(error) = runtime
        .traffic_session
        .mark_protected(validation, unix_time_ms().max(1))
    {
        tracing::error!(error = %error, "Exit readiness 证据无法建立受保护流量会话");
        runtime
            .enter_fail_closed(
                TrafficFailureKind::ConnectionTimeout,
                unix_time_ms(),
                "recovery.validation_application_failed",
            )
            .await;
        return;
    }
    runtime.pending_exit_readiness = None;
    runtime.exit_selection_blocked = false;
    runtime.exit_connection_watchdog = None;
    runtime.publish_traffic_session().await;
    tracing::info!(
        exit = %pending.target,
        "Exit readiness 全部通过，流量会话已进入受保护状态"
    );
}

#[cfg(target_os = "linux")]
fn exit_readiness_request_authorized(
    runtime: &ClientRuntime,
    peer: &PeerRouteInfo,
    now_unix_ms: u64,
) -> bool {
    if !runtime.local_exit_enabled
        || runtime.local_exit_runtime_problem.is_some()
        || !runtime.exit_node_manager.is_exit_node()
    {
        tracing::debug!("本机未处于可服务的 Exit 状态，拒绝 readiness 探测");
        return false;
    }
    let Some(forwarder) = runtime.exit_forwarder.as_ref() else {
        tracing::debug!("Exit 转发器尚未安装，拒绝 readiness 探测");
        return false;
    };
    if forwarder.is_draining() {
        tracing::debug!("Exit 转发器正在排空，拒绝新会话 readiness 探测");
        return false;
    }
    if !runtime
        .exit_node_manager
        .is_client_allowed(peer.client_id.as_ref())
    {
        tracing::warn!("Exit Node 拒绝未授权客户端的 readiness 探测");
        return false;
    }
    if let Err(denial) = runtime
        .exit_contribution_gate
        .authorize(&peer.node_id, now_unix_ms)
    {
        tracing::warn!(
            client = %peer.node_id,
            reason_code = denial.reason_code(),
            "Exit Node 拒绝未通过贡献治理授权的 readiness 探测"
        );
        return false;
    }
    true
}

#[cfg(not(target_os = "linux"))]
async fn handle_exit_readiness_frame(
    _runtime: &mut ClientRuntime,
    _src_ip: IpAddr,
    peer: PeerRouteInfo,
    _frame: ExitReadinessFrame,
) {
    tracing::debug!(
        peer = %peer.node_id,
        "当前平台没有可用的 Exit readiness 数据面，丢弃控制帧"
    );
}

async fn handle_inbound_packet(runtime: &mut ClientRuntime, packet: InboundPacket, source: &str) {
    match admit_inbound_packet(runtime, packet).await {
        Some(InboundAdmission::Payload(payload)) => {
            if let Some(tun) = runtime.tun_io.tun_mut() {
                write_payload_to_tun(tun, &runtime.stats, &payload, source).await;
            }
        }
        Some(InboundAdmission::ExitReadiness {
            src_ip,
            peer,
            frame,
        }) => {
            handle_exit_readiness_frame(runtime, src_ip, peer, frame).await;
        }
        None => {}
    }
}

async fn handle_inbound_packet_batch(
    runtime: &mut ClientRuntime,
    packets: Vec<InboundPacket>,
    source: &str,
) {
    let mut payloads = Vec::with_capacity(packets.len());
    for packet in packets {
        match admit_inbound_packet(runtime, packet).await {
            Some(InboundAdmission::Payload(payload)) => payloads.push(payload),
            Some(InboundAdmission::ExitReadiness {
                src_ip,
                peer,
                frame,
            }) => {
                handle_exit_readiness_frame(runtime, src_ip, peer, frame).await;
            }
            None => {}
        }
    }
    if let Some(tun) = runtime.tun_io.tun_mut() {
        write_payloads_to_tun(tun, &runtime.stats, &payloads, source).await;
    }
}

enum InboundAdmission {
    Payload(InboundPayload),
    ExitReadiness {
        src_ip: IpAddr,
        peer: PeerRouteInfo,
        frame: ExitReadinessFrame,
    },
}

/// Authenticate, authorize, and classify one ingress packet without writing it
/// to TUN. The returned owned payload remains valid until a batch write finishes.
async fn admit_inbound_packet(
    runtime: &mut ClientRuntime,
    packet: InboundPacket,
) -> Option<InboundAdmission> {
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
                    return None;
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
        return None;
    }
    let Some(peer) = peer else {
        tracing::debug!("丢弃未注册来源 {src_ip} 的入站数据包");
        return None;
    };
    if is_exit_readiness_frame(&plain_payload) {
        let frame = match decode_exit_readiness_frame(&plain_payload) {
            Ok(frame) => frame,
            Err(error) => {
                tracing::debug!("丢弃来自 {} 的无效 Exit readiness 帧: {error}", src_ip);
                return None;
            }
        };
        record_p2p_rx_for_peer(&runtime.stats, &peer, plain_payload.len()).await;
        return Some(InboundAdmission::ExitReadiness {
            src_ip,
            peer,
            frame,
        });
    }
    // Use one authoritative time sample for all authorization and accounting
    // decisions made while admitting this packet.
    let now_unix_ms = unix_time_ms();
    let trusted_exit_peer = match runtime.exit_for_packet(now_unix_ms).await {
        Some(exit_node_id) => {
            runtime
                .p2p
                .resolve_exit_peer_by_node_id(&exit_node_id)
                .await
        }
        None => None,
    };
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
        return None;
    };
    let Some(target_dst) = tun_packet_destination(&plain_payload) else {
        tracing::debug!("丢弃来源 {src_ip} 的无目的地址入站载荷");
        return None;
    };

    // Packets addressed to this node's assigned virtual IP are always Mesh
    // traffic, even when the configured IPv4 virtual prefix does not match the
    // address allocated by the coordinator. Any other destination is
    // privileged exit traffic and must pass every policy check.
    if !is_mesh_inbound_destination(&runtime.route_engine, &runtime.assigned_ips, target_dst) {
        #[cfg(target_os = "linux")]
        let owner_accepts_exit = local_exit_accepts_inbound(runtime);
        #[cfg(not(target_os = "linux"))]
        let owner_accepts_exit = runtime.local_exit_enabled;
        if !owner_accepts_exit || !runtime.exit_node_manager.is_exit_node() {
            tracing::debug!(
                "本机 Exit owner intent 或网络角色未启用，丢弃 {} -> {target_dst}",
                packet_src
            );
            return None;
        }
        if !runtime.exit_node_manager.should_forward_as_exit(target_dst) {
            tracing::debug!("Exit Node 策略拒绝目标地址 {target_dst}");
            return None;
        }
        if !runtime
            .exit_node_manager
            .is_client_allowed(peer.client_id.as_ref())
        {
            tracing::warn!("Exit Node 拒绝未授权客户端 {}", peer.client_id);
            return None;
        }
        if let Err(denial) = runtime
            .exit_contribution_gate
            .authorize(&peer.node_id, now_unix_ms)
        {
            tracing::warn!(
                client = %peer.node_id,
                reason_code = denial.reason_code(),
                "Exit Node 拒绝未通过贡献治理授权的数据包"
            );
            return None;
        }

        #[cfg(target_os = "linux")]
        {
            let Some(forwarder) = runtime.exit_forwarder.as_mut() else {
                tracing::warn!("Exit Node 已启用但转发器尚未安装，丢弃数据包");
                return None;
            };
            if !forwarder.supports(target_dst) {
                tracing::debug!("Exit Node 没有目标地址族 {target_dst} 的出站路径");
                return None;
            }
            if let Err(error) = forwarder
                .allow_client(peer.client_id.as_ref(), &peer.virtual_ips)
                .await
            {
                tracing::warn!(
                    "无法为客户端 {} 安装 Exit Node 转发规则: {error:#}",
                    peer.client_id
                );
                return None;
            }
        }

        #[cfg(not(target_os = "linux"))]
        {
            tracing::warn!("当前平台不支持内核 Exit Node 转发，丢弃数据包");
            return None;
        }

        // Reaching this point means every Exit policy, contribution, and
        // kernel-forwarding check has succeeded. Only accepted bytes count
        // toward owner bandwidth and quota limits.
        runtime
            .exit_contribution_gate
            .record_forwarded(plain_payload.len(), now_unix_ms);

        #[cfg(target_os = "linux")]
        tracing::debug!(
            "Exit Node 转发已认证数据包: client={}, {} -> {target_dst}, {} 字节",
            peer.node_id,
            packet_src,
            plain_payload.len()
        );
    }

    record_p2p_rx_for_peer(&runtime.stats, &peer, plain_payload.len()).await;
    Some(InboundAdmission::Payload(plain_payload))
}

fn is_mesh_inbound_destination(
    route_engine: &SmartRouteEngine,
    assigned_ips: &VirtualIps,
    target_dst: IpAddr,
) -> bool {
    assigned_ips.contains(target_dst) || route_engine.is_virtual_network(target_dst)
}

async fn handle_runtime_signaling_event(
    runtime: &mut ClientRuntime,
    event: SignalingEvent,
) -> Result<()> {
    runtime.stats.record_signaling_received().await;
    let registration_confirmed = matches!(event, SignalingEvent::Registered { .. });

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
                if let Err(error) = runtime
                    .peer_route_manager
                    .sync_peer(&peer.client_id, &peer.virtual_ips)
                    .await
                {
                    tracing::warn!(
                        "无法更新节点 {} 的虚拟地址精确定向路由: {error:#}",
                        peer.client_id
                    );
                }
            }
            sync_linux_relay_bypasses(runtime).await;
        }
        SignalingAction::PeerRemoved {
            client_id,
            removed_peer: _,
        } => {
            if let Err(error) = runtime.route_manager.remove_peer_bypasses(client_id).await {
                tracing::warn!("无法删除节点 {client_id} 的物理网络旁路: {error:#}");
            }
            let current_peer = runtime
                .p2p
                .get_peers()
                .await
                .into_iter()
                .find(|peer| peer.client_id == *client_id);
            let route_update = match current_peer {
                Some(peer) => {
                    runtime
                        .peer_route_manager
                        .sync_peer(&peer.client_id, &peer.virtual_ips)
                        .await
                }
                None => runtime.peer_route_manager.remove_peer(client_id).await,
            };
            if let Err(error) = route_update {
                tracing::warn!("无法更新节点 {client_id} 的虚拟地址精确定向路由: {error:#}");
            }
            if let Some(forwarder) = runtime.exit_forwarder.as_mut() {
                if let Err(error) = forwarder.remove_client(client_id).await {
                    tracing::warn!("无法删除节点 {client_id} 的 Exit Node 转发规则: {error:#}");
                }
            }
        }
        SignalingAction::Reconnected
        | SignalingAction::RelayedPacket { .. }
        | SignalingAction::SecureControlPacket { .. } => {}
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
        SignalingAction::SecureControlPacket { src_ip, payload } => {
            if let Err(error) = runtime
                .p2p
                .handle_secure_control_from_signaling(src_ip, payload)
                .await
            {
                tracing::debug!("丢弃信令中继的 Secure Session v2 控制帧: {error:#}");
            }
        }
        SignalingAction::Reconnected => {
            runtime.stats.record_signaling_reconnect().await;
        }
        SignalingAction::PeersUpsert(peers) => {
            tracing::debug!("已更新 {} 个节点的运行时状态", peers.len());
        }
        SignalingAction::PeerRemoved {
            client_id,
            removed_peer,
        } => {
            request_exit_recovery_on_offline(
                &runtime.recovery_trigger_tx,
                removed_peer.as_ref(),
                runtime.configured_exit_node.as_deref(),
            );
            tracing::debug!("已移除节点 {client_id} 的运行时状态");
        }
        SignalingAction::None => {}
    }

    if registration_confirmed {
        runtime
            .report_local_exit_availability("registration_confirmed")
            .await;
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

async fn write_payloads_to_tun(
    tun: &mut TunDevice,
    stats: &StatsCollector,
    payloads: &[InboundPayload],
    source: &str,
) {
    if payloads.is_empty() {
        return;
    }

    let packet_refs: Vec<&[u8]> = payloads.iter().map(InboundPayload::as_slice).collect();
    let results = tun.write_batch(&packet_refs).await;
    for (payload, result) in payloads.iter().zip(results) {
        match result {
            Ok(()) => stats.record_tun_tx(payload.len()).await,
            Err(error) => {
                stats.record_tun_write_error().await;
                tracing::error!("TUN 批量写入数据失败 (来源: {}): {:?}", source, error);
            }
        }
    }
}

fn abort_task(task: &Option<JoinHandle<()>>) {
    if let Some(task) = task {
        task.abort();
    }
}

async fn stop_runtime_task(task: &mut Option<JoinHandle<()>>, deadline: Instant) -> CleanupStatus {
    let Some(handle) = task.as_mut() else {
        return CleanupStatus::NotOwned;
    };

    // Request cancellation first, then join the handle so the task can release
    // cloned TUN/socket ownership before the corresponding device is removed.
    handle.abort();
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        return CleanupStatus::Deferred("任务取消超过总期限，句柄已保留".into());
    }
    match tokio::time::timeout(remaining, &mut *handle).await {
        Ok(Ok(())) => {
            *task = None;
            CleanupStatus::Removed
        }
        Ok(Err(error)) if error.is_cancelled() => {
            *task = None;
            CleanupStatus::Removed
        }
        Ok(Err(error)) => {
            *task = None;
            CleanupStatus::Failed(error.to_string())
        }
        Err(_) => CleanupStatus::Deferred("任务在取消后未于总期限内退出，句柄已保留".into()),
    }
}

fn cleanup_status_from_result(result: Result<()>) -> CleanupStatus {
    match result {
        Ok(()) => CleanupStatus::Removed,
        Err(error) => {
            let message = format!("{error:#}");
            if message.contains("保留")
                || message.contains("总期限")
                || message.contains("未完成")
                || message.contains("状态未知")
            {
                CleanupStatus::Deferred(message)
            } else {
                CleanupStatus::Failed(message)
            }
        }
    }
}

fn phase_timeout(deadline: Instant, maximum: Duration) -> Option<Duration> {
    let remaining = deadline.saturating_duration_since(Instant::now());
    (!remaining.is_zero()).then(|| remaining.min(maximum))
}

fn cleanup_deadline_exhausted() -> CleanupStatus {
    CleanupStatus::Deferred("运行时清理总期限已耗尽，资源仍由运行时持有".into())
}

// Resource teardown is ordered by dependency: producers -> transport clients
// -> workers -> P2P socket -> host routes -> TUN. Every phase uses a budget and
// the global deadline stays below runtime_control::STOP_TIMEOUT so the caller
// can retain the runtime for a safe retry instead of timing out opaquely.
async fn shutdown_runtime(runtime: &mut ClientRuntime) -> RuntimeCleanupOutcome {
    let mut outcome = RuntimeCleanupOutcome::default();
    let deadline = Instant::now() + RUNTIME_CLEANUP_TIMEOUT;
    let task_deadline = deadline.min(Instant::now() + TASK_STOP_TIMEOUT);

    runtime.recovery_events.shutdown();

    // TUN reader owns a device handle and must be joined before delete(). The
    // config watcher, private IPC listener, relay reconnect and grant renewal
    // are stopped under one shared deadline. A task that cannot be joined
    // remains in its owning Option so a later cleanup retry can still observe it.
    let (ipc, config_reload, tun_reader, relay_reconnect, relay_grant_renewal) = tokio::join!(
        stop_runtime_task(&mut runtime.ipc_task, task_deadline),
        stop_runtime_task(&mut runtime.config_reload_task, task_deadline),
        runtime.tun_io.stop_reader_within(task_deadline),
        stop_runtime_task(&mut runtime.relay_reconnect_task, task_deadline),
        stop_runtime_task(&mut runtime.relay_grant_renewal_task, task_deadline),
    );
    outcome.record("控制监听任务", ipc);
    outcome.record("配置监听任务", config_reload);
    outcome.record("TUN 读取任务", tun_reader);
    outcome.record("中继重连任务", relay_reconnect);
    outcome.record("中继授权续签任务", relay_grant_renewal);

    let signaling = match phase_timeout(deadline, TASK_STOP_TIMEOUT) {
        Some(timeout) => match runtime.signaling.close_bounded(timeout).await {
            Ok(()) => CleanupStatus::Removed,
            Err(error) if error.is_timed_out() => CleanupStatus::Deferred(error.to_string()),
            Err(error) => CleanupStatus::Failed(error.to_string()),
        },
        None => cleanup_deadline_exhausted(),
    };
    outcome.record("信令连接", signaling);

    let relay = match phase_timeout(deadline, RELAY_CLOSE_TIMEOUT) {
        Some(timeout) => runtime.outbound.close_relay(timeout).await,
        None => cleanup_deadline_exhausted(),
    };
    outcome.record("QUIC 中继", relay);

    let workers = match phase_timeout(deadline, OUTBOUND_CLOSE_TIMEOUT) {
        Some(timeout) => runtime.outbound.close_workers(timeout).await,
        None => cleanup_deadline_exhausted(),
    };
    outcome.record("出站 worker", workers);

    let p2p = match phase_timeout(deadline, TASK_STOP_TIMEOUT) {
        Some(timeout) => match runtime.p2p.shutdown_bounded(timeout).await {
            Ok(()) => CleanupStatus::Removed,
            Err(error) if error.is_timed_out() => CleanupStatus::Deferred(error.to_string()),
            Err(error) => CleanupStatus::Failed(error.to_string()),
        },
        None => cleanup_deadline_exhausted(),
    };
    outcome.record("P2P UDP 监听与子任务", p2p);

    #[cfg(target_os = "linux")]
    {
        let peer_routes = match phase_timeout(deadline, TASK_STOP_TIMEOUT) {
            Some(timeout) => cleanup_status_from_result(
                runtime.peer_route_manager.rollback_within(timeout).await,
            ),
            None => cleanup_deadline_exhausted(),
        };
        outcome.record("节点虚拟地址路由", peer_routes);

        let exit = match phase_timeout(deadline, TASK_STOP_TIMEOUT) {
            Some(timeout) => match runtime.exit_forwarder.as_mut() {
                Some(forwarder) => {
                    let status =
                        cleanup_status_from_result(forwarder.rollback_within(timeout).await);
                    if status.is_clean() {
                        runtime.exit_forwarder = None;
                    }
                    status
                }
                None => CleanupStatus::NotOwned,
            },
            None if runtime.exit_forwarder.is_some() => cleanup_deadline_exhausted(),
            None => CleanupStatus::NotOwned,
        };
        outcome.record("Exit 转发规则", exit);

        let routes = match phase_timeout(deadline, TASK_STOP_TIMEOUT) {
            Some(timeout) => {
                cleanup_status_from_result(runtime.route_manager.rollback_within(timeout).await)
            }
            None => cleanup_deadline_exhausted(),
        };
        outcome.record("路由与 DNS", routes);
    }

    let tun = match phase_timeout(deadline, TASK_STOP_TIMEOUT) {
        Some(timeout) => match tokio::time::timeout(timeout, runtime.tun_io.delete()).await {
            Ok(Ok(())) => CleanupStatus::Removed,
            Ok(Err(error)) => cleanup_status_from_result(Err(error)),
            Err(_) => CleanupStatus::Deferred("TUN 删除超过总期限，资源仍由运行时持有".into()),
        },
        None => cleanup_deadline_exhausted(),
    };
    outcome.record("TUN 设备", tun);

    outcome
}

fn spawn_tun_reader_pool(
    tun: &TunDevice,
    stats: &StatsCollector,
) -> (
    mpsc::UnboundedReceiver<Vec<TunPacket>>,
    mpsc::UnboundedSender<TunPacket>,
    JoinHandle<()>,
) {
    let (tun_packet_tx, tun_packet_rx) = mpsc::unbounded_channel::<Vec<TunPacket>>();
    let (tun_buffer_tx, tun_buffer_rx) = mpsc::unbounded_channel::<TunPacket>();
    for _ in 0..TUN_BUFFER_POOL_SIZE {
        let _ = tun_buffer_tx.send(TunPacket::empty(vec![0u8; TUN_BUFFER_ALLOCATION_SIZE]));
    }
    let reader_task = spawn_tun_reader(
        tun.clone(),
        tun_packet_tx,
        tun_buffer_rx,
        tun_buffer_tx.clone(),
        stats.clone(),
    );
    (tun_packet_rx, tun_buffer_tx, reader_task)
}

fn spawn_tun_reader(
    mut tun: TunDevice,
    packet_tx: mpsc::UnboundedSender<Vec<TunPacket>>,
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
            while packets.len() < DATA_PLANE_BATCH_SIZE {
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
                    if packet_tx.send(packets).is_err() {
                        for packet in unread {
                            let _ = buffer_tx.send(packet.recycle());
                        }
                        return;
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
    selected_exit: Option<&NodeId>,
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
                let target_v6 = match selected_exit {
                    Some(exit_node_id) => p2p.resolve_exit_peer_by_node_id(exit_node_id).await,
                    None => None,
                };
                if let Some(target_v6) = target_v6 {
                    tracing::debug!(
                        "智能分流判定为 Proxy，经由 Exit Node {} 转发流量至 {}",
                        target_v6,
                        dst_ip
                    );
                    IpAddr::V6(target_v6)
                } else {
                    tracing::warn!(
                        "智能分流判定为 Proxy 但没有管理员授权的可用 Exit，丢弃数据包至 {}",
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
    relay_selection: Arc<RwLock<RelaySelectionTracker>>,
}

impl RuntimeFallbackTransport {
    fn new(
        signaling: Arc<SignalingClient>,
        relay_state: RelayState,
        relay_selection: Arc<RwLock<RelaySelectionTracker>>,
    ) -> Self {
        Self {
            signaling,
            relay_state,
            relay_selection,
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
                Ok(Ok(())) => {
                    if !relay.forwarding_verified.swap(true, Ordering::AcqRel) {
                        self.relay_selection
                            .write()
                            .await
                            .mark_forwarding_success(&relay.relay_id);
                        tracing::info!(
                            diagnostic_id = "relay.health.forwarding_verified",
                            relay_id = %relay.relay_id,
                            "QUIC Relay 已通过真实转发验证"
                        );
                    }
                    RelayForwardOutcome::Sent
                }
                Ok(Err(error)) => {
                    self.relay_selection
                        .write()
                        .await
                        .mark_failure(&relay.relay_id, unix_time_ms());
                    if let Some(failed) = self
                        .relay_state
                        .clear_relay_after_failure(
                            relay.generation,
                            RelayFailureReason::ForwardSendFailed,
                        )
                        .await
                    {
                        failed.client.close().await;
                    }
                    RelayForwardOutcome::Failed(error)
                }
                Err(_) => {
                    self.relay_selection
                        .write()
                        .await
                        .mark_failure(&relay.relay_id, unix_time_ms());
                    if let Some(failed) = self
                        .relay_state
                        .clear_relay_after_failure(
                            relay.generation,
                            RelayFailureReason::ForwardSendTimeout,
                        )
                        .await
                    {
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

const RELAY_FAILOVER_AUDIT_QUEUE_CAPACITY: usize = 32;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RelayFailureReason {
    ReceiveStreamClosed,
    ForwardSendFailed,
    ForwardSendTimeout,
}

impl RelayFailureReason {
    const fn as_str(self) -> &'static str {
        match self {
            Self::ReceiveStreamClosed => "relay.failure.receive_stream_closed",
            Self::ForwardSendFailed => "relay.failure.forward_send_failed",
            Self::ForwardSendTimeout => "relay.failure.forward_send_timeout",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RelayFailoverAuditEvent {
    actor: NodeId,
    network_id: Option<NetworkId>,
    failed_generation: u64,
    installed_generation: u64,
    failed_provider_node_id: Option<NodeId>,
    provider_node_id: NodeId,
    failure_reason: RelayFailureReason,
    occurred_at_unix_ms: u64,
}

impl RelayFailoverAuditEvent {
    fn audit_entry(&self) -> AuditEntry {
        let mut details = std::collections::BTreeMap::new();
        details.insert("reason_code".into(), self.failure_reason.as_str().into());
        details.insert("failure_code".into(), self.failure_reason.as_str().into());
        details.insert(
            "failed_generation".into(),
            self.failed_generation.to_string(),
        );
        details.insert(
            "installed_generation".into(),
            self.installed_generation.to_string(),
        );
        details.insert("provider_node_id".into(), self.provider_node_id.to_string());
        if let Some(failed_provider) = &self.failed_provider_node_id {
            details.insert(
                "failed_provider_node_id".into(),
                failed_provider.to_string(),
            );
        }
        details.insert("actor_role".into(), "relay_consumer".into());
        details.insert("object_kind".into(), "relay_session".into());
        details.insert("object_id".into(), self.provider_node_id.to_string());
        if let Some(network_id) = &self.network_id {
            details.insert("network_id".into(), network_id.to_string());
        }
        let audit_id_material = format!(
            "{}:{}:{}:{}:{}",
            self.actor,
            self.network_id
                .as_ref()
                .map(ToString::to_string)
                .as_deref()
                .unwrap_or("-"),
            self.failed_generation,
            self.installed_generation,
            self.occurred_at_unix_ms,
        );
        AuditEntry {
            audit_id: AuditId::from_public_key(audit_id_material.as_bytes()),
            operation_id: None,
            actor: self.actor.clone(),
            network_id: self.network_id.clone(),
            action: RelayGovernanceAction::Failover.wire_name().into(),
            resource: "relay_session".into(),
            outcome: AuditOutcome::Succeeded,
            occurred_at_unix_ms: self.occurred_at_unix_ms,
            summary: "Relay 使用方已切换到新的中继会话".into(),
            details,
            sensitive_keys: Default::default(),
        }
    }
}

#[derive(Debug, Clone)]
struct RelayFailoverAuditSink {
    actor: NodeId,
    network_id: Option<NetworkId>,
    sender: Option<mpsc::Sender<RelayFailoverAuditEvent>>,
    dropped: Arc<AtomicU64>,
}

impl RelayFailoverAuditSink {
    fn try_record(
        &self,
        failed_generation: u64,
        installed_generation: u64,
        failed_provider_node_id: Option<NodeId>,
        provider_node_id: NodeId,
        failure_reason: RelayFailureReason,
    ) -> bool {
        let Some(sender) = self.sender.as_ref() else {
            return false;
        };
        let event = RelayFailoverAuditEvent {
            actor: self.actor.clone(),
            network_id: self.network_id.clone(),
            failed_generation,
            installed_generation,
            failed_provider_node_id,
            provider_node_id,
            failure_reason,
            occurred_at_unix_ms: unix_time_ms(),
        };
        match sender.try_send(event) {
            Ok(()) => true,
            Err(mpsc::error::TrySendError::Full(_)) => {
                self.dropped.fetch_add(1, Ordering::Relaxed);
                tracing::warn!(
                    diagnostic_id = "relay.failover.audit_queue_full",
                    "Relay 故障转移审计队列已满，丢弃一条非阻塞生命周期记录"
                );
                false
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                self.dropped.fetch_add(1, Ordering::Relaxed);
                tracing::warn!(
                    diagnostic_id = "relay.failover.audit_worker_unavailable",
                    "Relay 故障转移审计 worker 已停止，记录未持久化"
                );
                false
            }
        }
    }

    #[cfg(test)]
    fn dropped_count(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }
}

fn spawn_relay_failover_audit_worker(
    audit: Arc<OwnerSecurityAudit>,
    state: Option<NodeStateStore>,
    actor: NodeId,
    network_id: Option<NetworkId>,
) -> RelayFailoverAuditSink {
    let (sender, mut receiver) =
        mpsc::channel::<RelayFailoverAuditEvent>(RELAY_FAILOVER_AUDIT_QUEUE_CAPACITY);
    tokio::spawn(async move {
        while let Some(event) = receiver.recv().await {
            let ledger = audit.clone();
            let entry = event.audit_entry();
            match tokio::task::spawn_blocking(move || ledger.append_entry(entry)).await {
                Ok(Ok(record)) => {
                    if let Some(state) = state.as_ref() {
                        state
                            .merge_recent_activity([record.entry.project_activity()])
                            .await;
                    }
                }
                Ok(Err(error)) => tracing::warn!(
                    diagnostic_id = "relay.failover.audit_write_failed",
                    error = %format!("{error:#}"),
                    "Relay 故障转移审计写入失败"
                ),
                Err(error) => tracing::warn!(
                    diagnostic_id = "relay.failover.audit_worker_failed",
                    error = %error,
                    "Relay 故障转移审计 worker 异常退出"
                ),
            }
        }
    });
    RelayFailoverAuditSink {
        actor,
        network_id,
        sender: Some(sender),
        dropped: Arc::new(AtomicU64::new(0)),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RelayFailureContext {
    failed_generation: u64,
    provider_node_id: Option<NodeId>,
    reason: RelayFailureReason,
}

#[derive(Default)]
struct PendingRelayFailover {
    context: Mutex<Option<RelayFailureContext>>,
}

impl PendingRelayFailover {
    fn record(&self, context: RelayFailureContext) -> bool {
        let mut pending = self
            .context
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if pending.is_some() {
            return false;
        }
        *pending = Some(context);
        true
    }

    fn take_after_install(&self, installed_generation: u64) -> Option<RelayFailureContext> {
        let mut pending = self
            .context
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if pending
            .as_ref()
            .is_some_and(|context| context.failed_generation < installed_generation)
        {
            pending.take()
        } else {
            None
        }
    }
}

#[derive(Clone)]
struct RelayHandle {
    generation: u64,
    relay_id: String,
    provider_node_id: NodeId,
    client: QuicRelayClient,
    forwarding_verified: Arc<AtomicBool>,
}

#[derive(Clone, Default)]
struct RelayState {
    slot: Arc<RwLock<Option<RelayHandle>>>,
    next_generation: Arc<AtomicU64>,
    reconnecting: Arc<AtomicBool>,
    closing: Arc<AtomicBool>,
    pending_failover: Arc<PendingRelayFailover>,
    failover_audit: Option<RelayFailoverAuditSink>,
    recovery_trigger_tx: Option<RuntimeRecoveryTriggerSender>,
}

impl RelayState {
    fn with_recovery_trigger_sender(sender: RuntimeRecoveryTriggerSender) -> Self {
        Self {
            recovery_trigger_tx: Some(sender),
            ..Self::default()
        }
    }

    fn with_recovery_and_failover_audit(
        sender: RuntimeRecoveryTriggerSender,
        failover_audit: RelayFailoverAuditSink,
    ) -> Self {
        Self {
            failover_audit: Some(failover_audit),
            recovery_trigger_tx: Some(sender),
            ..Self::default()
        }
    }

    async fn relay_handle(&self) -> Option<RelayHandle> {
        self.slot.read().await.clone()
    }

    async fn install_relay(
        &self,
        client: QuicRelayClient,
        relay_id: String,
        provider_node_id: NodeId,
    ) -> std::result::Result<Option<RelayHandle>, QuicRelayClient> {
        if self.closing.load(Ordering::Acquire) {
            return Err(client);
        }
        let generation = self.next_generation.fetch_add(1, Ordering::AcqRel) + 1;
        let mut slot = self.slot.write().await;
        if self.closing.load(Ordering::Acquire) {
            return Err(client);
        }
        let previous = slot.replace(RelayHandle {
            generation,
            relay_id,
            provider_node_id: provider_node_id.clone(),
            client,
            forwarding_verified: Arc::new(AtomicBool::new(false)),
        });
        if let Some(failure) = self.pending_failover.take_after_install(generation) {
            if let Some(audit) = self.failover_audit.as_ref() {
                audit.try_record(
                    failure.failed_generation,
                    generation,
                    failure.provider_node_id,
                    provider_node_id,
                    failure.reason,
                );
            }
        }
        Ok(previous)
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

    async fn clear_relay_with_recovery(&self, generation: u64) -> Option<RelayHandle> {
        let relay = self.clear_relay(generation).await?;
        self.notify_recovery_trigger(RecoveryTrigger::RelayDisconnected);
        Some(relay)
    }

    async fn clear_relay_after_failure(
        &self,
        generation: u64,
        reason: RelayFailureReason,
    ) -> Option<RelayHandle> {
        let relay = self.clear_relay(generation).await?;
        self.pending_failover.record(RelayFailureContext {
            failed_generation: relay.generation,
            provider_node_id: Some(relay.provider_node_id.clone()),
            reason,
        });
        self.notify_recovery_trigger(RecoveryTrigger::RelayDisconnected);
        Some(relay)
    }

    fn notify_recovery_trigger(&self, trigger: RecoveryTrigger) {
        let Some(sender) = self.recovery_trigger_tx.as_ref() else {
            tracing::warn!(
                trigger = ?trigger,
                diagnostic_id = "recovery.trigger_queue_unavailable",
                "恢复触发队列未配置"
            );
            return;
        };
        if !sender.try_trigger(trigger) {
            tracing::warn!(
                trigger = ?trigger,
                diagnostic_id = "recovery.trigger_queue_unavailable",
                "恢复触发队列已满或已关闭"
            );
        }
    }

    async fn begin_reconnect(&self) -> bool {
        if self.closing.load(Ordering::Acquire) {
            return false;
        }
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

    fn mark_closing(&self) {
        self.closing.store(true, Ordering::Release);
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
    workers: Arc<Mutex<HashMap<IpAddr, OutboundWorker>>>,
    shutdown_started: Arc<AtomicBool>,
}

struct OutboundWorker {
    sender: Option<mpsc::Sender<OutboundPacket>>,
    task: JoinHandle<()>,
}

impl OutboundDispatcher {
    fn new(transport: Arc<dyn OutboundTransport>, relay_state: RelayState) -> Self {
        Self {
            transport,
            relay_state,
            workers: Arc::new(Mutex::new(HashMap::new())),
            shutdown_started: Arc::new(AtomicBool::new(false)),
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
        // Serialize this check with close_workers() taking the registry.
        // Otherwise submit could observe the old flag and install a worker
        // after shutdown has captured the registry.
        if self.shutdown_started.load(Ordering::Acquire) {
            return OutboundSubmitOutcome::WorkerUnavailable;
        }
        let sender = workers
            .get(&dst_ip)
            .and_then(|worker| worker.sender.clone())
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
        workers: &mut HashMap<IpAddr, OutboundWorker>,
        dst_ip: IpAddr,
    ) -> mpsc::Sender<OutboundPacket> {
        let (sender, receiver) = mpsc::channel(OUTBOUND_QUEUE_CAPACITY);
        let transport = self.transport.clone();
        let task = tokio::spawn(run_outbound_worker(dst_ip, receiver, transport));
        workers.insert(
            dst_ip,
            OutboundWorker {
                sender: Some(sender.clone()),
                task,
            },
        );
        sender
    }

    async fn close_workers(&self, total_timeout: Duration) -> CleanupStatus {
        self.shutdown_started.store(true, Ordering::Release);
        let workers = {
            let mut registry = self
                .workers
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            std::mem::take(&mut *registry)
        };
        if workers.is_empty() {
            return CleanupStatus::NotOwned;
        }

        let deadline = Instant::now() + total_timeout;
        let mut tasks = Vec::with_capacity(workers.len());
        for (dst_ip, mut worker) in workers {
            // Dropping the last sender closes the receiver queue. Abort also
            // interrupts a worker currently blocked in its transport send.
            worker.sender.take();
            worker.task.abort();
            tasks.push((dst_ip, worker.task));
        }

        let mut unfinished = HashMap::new();
        let mut failed = false;
        for (dst_ip, mut task) in tasks {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                unfinished.insert(dst_ip, OutboundWorker { sender: None, task });
                continue;
            }
            match tokio::time::timeout(remaining, &mut task).await {
                Ok(Ok(())) => {}
                Ok(Err(error)) if error.is_cancelled() => {}
                Ok(Err(error)) => {
                    tracing::warn!(%dst_ip, %error, "出站 worker 异常退出");
                    failed = true;
                }
                Err(_) => {
                    task.abort();
                    unfinished.insert(dst_ip, OutboundWorker { sender: None, task });
                }
            }
        }

        let remaining = unfinished.len();
        {
            let mut registry = self
                .workers
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            *registry = unfinished;
        }

        if remaining > 0 {
            CleanupStatus::Deferred(format!(
                "{remaining} 个出站 worker 未在期限内退出，句柄已保留"
            ))
        } else if failed {
            CleanupStatus::Failed("出站 worker 异常退出".into())
        } else {
            CleanupStatus::Removed
        }
    }

    async fn relay_handle(&self) -> Option<RelayHandle> {
        self.relay_state.relay_handle().await
    }

    async fn install_relay(
        &self,
        relay: QuicRelayClient,
        relay_id: String,
        provider_node_id: NodeId,
    ) -> std::result::Result<Option<RelayHandle>, QuicRelayClient> {
        self.relay_state
            .install_relay(relay, relay_id, provider_node_id)
            .await
    }

    async fn clear_relay_with_recovery(&self, generation: u64) -> Option<RelayHandle> {
        self.relay_state.clear_relay_with_recovery(generation).await
    }

    async fn clear_relay_after_failure(
        &self,
        generation: u64,
        reason: RelayFailureReason,
    ) -> Option<RelayHandle> {
        self.relay_state
            .clear_relay_after_failure(generation, reason)
            .await
    }

    async fn close_relay(&self, total_timeout: Duration) -> CleanupStatus {
        self.relay_state.mark_closing();
        let Some(relay) = self.relay_state.relay_handle().await else {
            return CleanupStatus::NotOwned;
        };
        let generation = relay.generation;
        match tokio::time::timeout(total_timeout, relay.client.close()).await {
            Ok(()) => {}
            Err(_) => {
                return CleanupStatus::Deferred(format!(
                    "QUIC 中继关闭超过 {total_timeout:?}，句柄已保留"
                ));
            }
        }
        if self.relay_state.clear_relay(generation).await.is_some() {
            CleanupStatus::Removed
        } else {
            CleanupStatus::Deferred("QUIC 中继句柄在关闭期间被替换，新句柄仍受管理".into())
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
        relay_selection: Arc<RwLock<RelaySelectionTracker>>,
    ) -> Self {
        Self {
            p2p,
            fallback: Arc::new(RuntimeFallbackTransport::new(
                signaling,
                relay_state,
                relay_selection,
            )),
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
        let stats_peer_ip = self
            .p2p
            .peer_route_by_ip(destination)
            .await
            .map(|peer| IpAddr::V6(peer.virtual_ip))
            .unwrap_or(destination);
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
                    self.stats.record_p2p_tx(stats_peer_ip, plaintext_len).await;
                }
                Ok(InPlaceSendOutcome::SealedForFallback { encoded_len, error }) => {
                    self.stats.record_p2p_send_error().await;
                    tracing::debug!(
                        "Failed to send packet to {} over P2P, using sealed fallback packet: {:?}",
                        packet.dst_ip,
                        error
                    );
                    self.send_fallback(packet, encoded_len, stats_peer_ip).await;
                }
                Err(error) => {
                    self.stats.record_p2p_send_error().await;
                    tracing::debug!("Failed to seal packet for {}: {:?}", packet.dst_ip, error);
                }
            }
        }
    }

    async fn send_fallback(
        &self,
        packet: OutboundPacket,
        encoded_len: usize,
        stats_peer_ip: IpAddr,
    ) {
        let plaintext_len = packet.plaintext_len();
        let src_ip = packet.src_ip;
        let dst_ip = packet.dst_ip;
        let encrypted = packet.into_encrypted_bytes(encoded_len);
        if let (IpAddr::V6(src_v6), IpAddr::V6(dst_v6)) = (src_ip, dst_ip) {
            match self.fallback.relay(src_v6, dst_v6, encrypted.clone()).await {
                RelayForwardOutcome::Sent => {
                    self.stats.record_p2p_tx(stats_peer_ip, plaintext_len).await;
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
                self.stats.record_p2p_tx(stats_peer_ip, plaintext_len).await;
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

fn spawn_relay_reconnect(runtime: &ClientRuntime) -> JoinHandle<()> {
    let signaling = runtime.signaling.clone();
    let identity = runtime.identity.clone();
    let assigned_ip = runtime.assigned_ip;
    let outbound = runtime.outbound.clone();
    let network_id = runtime.network_id.clone();
    let node_state = runtime.node_state.clone();
    let relay_selection = runtime.relay_selection.clone();

    tokio::spawn(async move {
        let snapshot = match node_state.as_ref() {
            Some(state) => Some(state.snapshot().await),
            None => None,
        };
        let relay = connect_quic_relay(
            signaling.as_ref(),
            &identity,
            assigned_ip,
            network_id.as_ref(),
            snapshot.as_ref(),
            relay_selection.as_ref(),
        )
        .await;
        if let Some(relay) = relay {
            match outbound
                .install_relay(relay.client, relay.relay_id, relay.provider_node_id)
                .await
            {
                Ok(Some(previous)) => previous.client.close().await,
                Ok(None) => {}
                Err(relay) => relay.close().await,
            }
        }
        outbound.finish_relay_reconnect();
    })
}

fn spawn_relay_grant_renewal(runtime: &ClientRuntime) -> JoinHandle<()> {
    let signaling = runtime.signaling.clone();
    let identity = runtime.identity.clone();
    let outbound = runtime.outbound.clone();
    let network_id = runtime.network_id.clone();
    let node_state = runtime.node_state.clone();

    tokio::spawn(async move {
        let Some(relay) = outbound.relay_handle().await else {
            return;
        };
        let generation = relay.generation;
        let current = relay.client.current_member_grant().await;

        let snapshot = match node_state.as_ref() {
            Some(state) => Some(state.snapshot().await),
            None => None,
        };
        let Some(network_id) = network_id.as_ref() else {
            tracing::warn!(
                diagnostic_id = "relay.grant.renewal_context_missing",
                "缺少网络上下文，无法续签 Relay 使用授权"
            );
            if outbound
                .clear_relay_with_recovery(generation)
                .await
                .is_some()
            {
                relay.client.close().await;
            }
            return;
        };
        let Some(snapshot) = snapshot.as_ref() else {
            tracing::warn!(
                diagnostic_id = "relay.grant.renewal_context_missing",
                "缺少可信节点状态，无法续签 Relay 使用授权"
            );
            if outbound
                .clear_relay_with_recovery(generation)
                .await
                .is_some()
            {
                relay.client.close().await;
            }
            return;
        };

        let endpoints = signaling.relay_endpoints().await;
        let grants =
            issue_relay_member_grants(&endpoints, &identity, network_id, snapshot, unix_time_ms());
        let Some(renewed) = renewed_relay_grant(&grants, &current).cloned() else {
            tracing::warn!(
                diagnostic_id = "relay.grant.renewal_denied",
                provider_node_id = %current.provider_node_id,
                "没有取得延长有效期的 Relay 使用授权，移除当前失效连接"
            );
            if outbound
                .clear_relay_with_recovery(generation)
                .await
                .is_some()
            {
                relay.client.close().await;
            }
            return;
        };

        match tokio::time::timeout(
            RELAY_AUTHORIZATION_UPDATE_TIMEOUT,
            relay.client.update_authorization(&identity, &renewed),
        )
        .await
        {
            Ok(Ok(())) => tracing::info!(
                diagnostic_id = "relay.grant.renewed",
                provider_node_id = %renewed.provider_node_id,
                expires_at_unix_ms = renewed.expires_at_unix_ms,
                "Relay 使用授权已在现有连接上续签"
            ),
            Ok(Err(error)) => {
                tracing::warn!(
                    diagnostic_id = "relay.grant.renewal_failed",
                    provider_node_id = %renewed.provider_node_id,
                    error = %error,
                    "Relay 使用授权续签被拒绝，移除当前连接并等待自愈重连"
                );
                if outbound
                    .clear_relay_with_recovery(generation)
                    .await
                    .is_some()
                {
                    relay.client.close().await;
                }
            }
            Err(_) => {
                tracing::warn!(
                    diagnostic_id = "relay.grant.renewal_timeout",
                    provider_node_id = %renewed.provider_node_id,
                    timeout = ?RELAY_AUTHORIZATION_UPDATE_TIMEOUT,
                    "Relay 使用授权续签超时，移除当前连接并等待自愈重连"
                );
                if outbound
                    .clear_relay_with_recovery(generation)
                    .await
                    .is_some()
                {
                    relay.client.close().await;
                }
            }
        }
    })
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
    PeerRemoved {
        client_id: String,
        removed_peer: Option<PeerInfo>,
    },
    RelayedPacket {
        src_ip: IpAddr,
        payload: Bytes,
    },
    SecureControlPacket {
        src_ip: IpAddr,
        payload: Bytes,
    },
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
            let removed_peer = remove_offline_peer(p2p, stats, client_id.clone()).await;
            return SignalingAction::PeerRemoved {
                client_id,
                removed_peer,
            };
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

        SignalingEvent::SecureControlPacket {
            src_ip,
            dst_ip,
            payload,
        } => {
            tracing::debug!(
                "收到中继 Secure Session v2 控制帧: {} -> {}, {} bytes",
                src_ip,
                dst_ip,
                payload.len()
            );
            return SignalingAction::SecureControlPacket { src_ip, payload };
        }

        SignalingEvent::JoinRequestStatus {
            request_id,
            state,
            revision,
            ..
        } => {
            tracing::debug!(
                "审批请求状态更新: request={}, state={:?}, revision={}",
                request_id,
                state,
                revision
            );
        }

        SignalingEvent::JoinRequestListResponse { requests } => {
            tracing::debug!("收到 {} 条审批请求", requests.len());
        }

        SignalingEvent::JoinRequestReviewAck { reviews, .. } => {
            tracing::debug!("审批请求确认 {} 条", reviews.len());
        }

        SignalingEvent::JoinRequestChanged {
            request_id,
            state,
            revision,
        } => {
            tracing::debug!(
                "审批请求变更通知: request={}, state={:?}, revision={}",
                request_id,
                state,
                revision
            );
        }

        SignalingEvent::InvitationChanged {
            invitation_id,
            state,
            revision,
        } => {
            tracing::debug!(
                "邀请变更通知: invitation={}, state={:?}, revision={}",
                invitation_id,
                state,
                revision
            );
        }

        SignalingEvent::CoordinatorCandidateStatusChanged { network_id } => {
            tracing::debug!("Coordinator 候选状态变更通知: network={network_id}");
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

async fn remove_offline_peer(
    p2p: &P2pEngine,
    stats: &StatsCollector,
    client_id: String,
) -> Option<PeerInfo> {
    tracing::info!("节点离线: {}", client_id);
    let removed_peer = p2p.remove_peer_by_id(&client_id).await?;
    let stats_ip = removed_peer
        .virtual_ips
        .primary_ip()
        .unwrap_or(IpAddr::V6(removed_peer.virtual_ip));
    stats.remove_peer(stats_ip).await;
    Some(removed_peer)
}

/// Whether an actually removed peer is the Exit target from this runtime's
/// configuration. The configured target accepts either a client ID or one of
/// the peer's virtual IP addresses, matching route resolution semantics.
fn peer_is_configured_exit(peer: &PeerInfo, configured_exit_node: Option<&str>) -> bool {
    let Some(target) = configured_exit_node
        .map(str::trim)
        .filter(|target| !target.is_empty())
    else {
        return false;
    };

    if let Ok(ipv6) = target.parse::<Ipv6Addr>() {
        return peer.virtual_ip == ipv6;
    }
    if let Ok(ipv4) = target.parse::<Ipv4Addr>() {
        return peer.virtual_ips.ipv4 == Some(ipv4);
    }
    peer.client_id == target
}

fn request_exit_recovery_on_offline(
    sender: &RuntimeRecoveryTriggerSender,
    removed_peer: Option<&PeerInfo>,
    configured_exit_node: Option<&str>,
) {
    if !removed_peer.is_some_and(|peer| peer_is_configured_exit(peer, configured_exit_node)) {
        return;
    }
    if !sender.try_trigger(RecoveryTrigger::ExitDisconnected) {
        tracing::warn!(
            trigger = ?RecoveryTrigger::ExitDisconnected,
            diagnostic_id = "recovery.trigger_queue_unavailable",
            "Exit 已下线，但恢复触发队列已满或已关闭"
        );
    }
}

pub(crate) async fn cli_node_session() -> Result<RemoteNodeSession> {
    RemoteNodeSession::start_with_role_async(
        RuntimeOptions {
            headless: true,
            ui_mode: crate::ui::resolve_ui_request(
                crate::ui::UiModeRequest::Auto,
                crate::ui::UiCapabilities::detect(),
            )
            .ok(),
            ..RuntimeOptions::default()
        },
        IpcRole::Cli,
    )
    .await
}

/// Force the CLI adapter to consume the node's latest authoritative snapshot.
pub(crate) async fn refresh_cli_node_snapshot(session: &RemoteNodeSession) -> Result<()> {
    match execute_cli_node_command(session, NodeCommand::GetSnapshot).await? {
        NodeCommandOutcome::Snapshot(snapshot) => {
            session.state().replace_authoritative(*snapshot).await;
            Ok(())
        }
        other => anyhow::bail!("获取节点权威快照返回了意外结果: {other:?}"),
    }
}

/// Submit one CLI operation through the authenticated node command bus.
pub(crate) async fn execute_cli_node_command(
    session: &RemoteNodeSession,
    command: NodeCommand,
) -> Result<NodeCommandOutcome> {
    static NEXT_REQUEST_ID: AtomicU64 = AtomicU64::new(1);
    let request_id = NEXT_REQUEST_ID.fetch_add(1, Ordering::Relaxed);
    let snapshot = session.state().snapshot().await;
    let context = NodeCommandContext::local_owner_for_snapshot(&snapshot, "cli");
    let result = session
        .commands()
        .execute(context, NodeCommandRequest::new(request_id, command))
        .await
        .map_err(|error| anyhow::anyhow!(error.to_string()))?;
    match result.outcome {
        NodeCommandOutcome::Rejected(problem) => {
            anyhow::bail!("节点拒绝了命令 ({:?}): {}", problem.code, problem.message)
        }
        outcome => Ok(outcome),
    }
}

fn node_network_status_label(status: NodeNetworkStatus) -> &'static str {
    match status {
        NodeNetworkStatus::Offline => "离线",
        NodeNetworkStatus::Connecting => "连接中",
        NodeNetworkStatus::Online => "在线",
        NodeNetworkStatus::Degraded => "运行降级",
        NodeNetworkStatus::Stopping => "停止中",
        NodeNetworkStatus::Failed => "启动失败",
    }
}

fn node_network_roles(network: &NodeNetworkSnapshot) -> String {
    network
        .roles
        .iter()
        .map(|role| match role {
            crate::node::NodeNetworkRole::Member => "Member",
            crate::node::NodeNetworkRole::Discovery => "Discovery",
            crate::node::NodeNetworkRole::Relay => "Relay",
            crate::node::NodeNetworkRole::ExitNode => "ExitNode",
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// Print the authoritative node snapshot through the shared node control plane.
pub async fn node_status(json: bool) -> Result<()> {
    let session = cli_node_session().await?;
    let snapshot = session.state().snapshot().await;

    if json {
        println!("{}", serde_json::to_string_pretty(&snapshot)?);
        session.shutdown();
        return Ok(());
    }

    println!("节点状态 (Node Status)");
    println!("─────────────────────────────────────");
    println!("生命周期:        {:?}", snapshot.lifecycle.phase);
    println!(
        "网络:            活动 {} / 已配置 {}",
        snapshot.lifecycle.active_networks, snapshot.lifecycle.configured_networks
    );
    println!(
        "在线 Peer:       {}",
        snapshot.peers.iter().filter(|peer| peer.online).count()
    );

    for service in &snapshot.embedded_services {
        println!(
            "控制服务:        {} · {:?} · signaling={} relay={}",
            service.network_name,
            service.status,
            service
                .signaling_addr
                .map(|addr| addr.to_string())
                .unwrap_or_else(|| "-".to_string()),
            service
                .relay_addr
                .map(|addr| addr.to_string())
                .unwrap_or_else(|| "-".to_string())
        );
    }

    if snapshot.networks.is_empty() {
        println!();
        println!("当前没有已启动的网络。");
    }

    for network in &snapshot.networks {
        println!();
        println!("🌐 虚拟网络:        {}", network.name);
        println!(
            "   状态:            {}",
            node_network_status_label(network.status)
        );
        println!("   TUN 接口:        {}", network.tun_name);
        println!(
            "   虚拟 IPv6:       {}",
            network
                .assigned_ipv6
                .map(|ip| ip.to_string())
                .unwrap_or_else(|| "-".to_string())
        );
        println!(
            "   虚拟 IPv4:       {}",
            network
                .assigned_ipv4
                .map(|ip| ip.to_string())
                .unwrap_or_else(|| "-".to_string())
        );
        println!(
            "   P2P 监听:        {}",
            network
                .p2p_listen_addr
                .map(|addr| addr.to_string())
                .unwrap_or_else(|| "-".to_string())
        );
        println!("   角色:            {}", node_network_roles(network));
        println!(
            "   运行时间:        {}",
            format_duration(network.uptime_secs)
        );
        println!("   在线对等节点:    {}", network.peer_count);
        println!(
            "   TUN 流量:        {} 收 / {} 发 数据包 ({} 字节)",
            network.traffic.tun_rx_packets,
            network.traffic.tun_tx_packets,
            network
                .traffic
                .tun_rx_bytes
                .saturating_add(network.traffic.tun_tx_bytes)
        );
        println!(
            "   P2P 流量:        {} 收 / {} 发 数据包 ({} 字节)",
            network.traffic.p2p_rx_packets,
            network.traffic.p2p_tx_packets,
            network
                .traffic
                .p2p_rx_bytes
                .saturating_add(network.traffic.p2p_tx_bytes)
        );
        println!("   错误计数:        {}", network.traffic.error_count);
    }

    if let Some(error) = &snapshot.last_error {
        println!();
        println!("最近错误:        {:?}: {}", error.code, error.message);
    }

    session.shutdown();
    Ok(())
}

/// Run the layered node diagnostics and print the server-authoritative report.
///
/// The default output is the bounded, redacted `sanitized_report` suitable for
/// sharing. `--json` emits the full diagnostics snapshot for tooling.
pub async fn node_diagnostics(json: bool) -> Result<()> {
    let session = cli_node_session().await?;
    let outcome = execute_cli_node_command(&session, NodeCommand::GetDiagnostics).await?;
    match outcome {
        NodeCommandOutcome::Diagnostics(diagnostics) => {
            if json {
                println!("{}", serde_json::to_string_pretty(&diagnostics)?);
            } else {
                println!("{}", diagnostics.sanitized_report);
            }
        }
        other => anyhow::bail!("诊断命令返回了意外结果: {other:?}"),
    }
    session.shutdown();
    Ok(())
}

fn invitation_role(role: &str) -> Result<MembershipRole> {
    match role.trim() {
        "member" => Ok(MembershipRole::Member),
        "administrator" | "admin" => Ok(MembershipRole::Administrator),
        _ => anyhow::bail!("邀请角色必须是 member 或 administrator"),
    }
}

fn invitation_capabilities(capabilities: &[String]) -> Result<Vec<CapabilityKind>> {
    capabilities
        .iter()
        .map(|capability| match capability.trim() {
            "coordinator" => Ok(CapabilityKind::Coordinator),
            "relay" => Ok(CapabilityKind::Relay),
            "exit" | "exit_node" | "exit-node" => Ok(CapabilityKind::Exit),
            "file" => Ok(CapabilityKind::File),
            _ => anyhow::bail!("未知邀请能力: {capability}"),
        })
        .collect()
}

fn invitation_network_id(network_id: &str) -> Result<NetworkId> {
    NetworkId::parse(network_id.trim()).context("网络 ID 格式无效")
}

/// Create a scoped invitation through the running node's authoritative command bus.
pub async fn create_invitation(
    network_id: &str,
    role: &str,
    capabilities: &[String],
    max_uses: u32,
    expires_in_ms: u64,
    json: bool,
) -> Result<()> {
    let command = NodeCreateInvitationCommand {
        network_id: invitation_network_id(network_id)?,
        role: invitation_role(role)?,
        capabilities: invitation_capabilities(capabilities)?,
        max_uses,
        expires_in_ms,
    };
    let session = cli_node_session().await?;
    let outcome =
        execute_cli_node_command(&session, NodeCommand::CreateInvitation(command)).await?;
    match outcome {
        NodeCommandOutcome::InvitationCreated(issued) => {
            if json {
                println!("{}", serde_json::to_string_pretty(&issued)?);
            } else {
                println!("邀请已创建");
                println!("网络:       {}", issued.status.invitation.network_id);
                println!("邀请 ID:    {}", issued.status.invitation.invitation_id);
                println!("角色:       {:?}", issued.status.invitation.role);
                println!("剩余次数:   {}", issued.status.remaining_uses);
                println!(
                    "过期时间:   {} ms",
                    issued.status.invitation.expires_at_unix_ms
                );
                println!("邀请码:     {}", issued.code.as_str());
            }
        }
        other => anyhow::bail!("创建邀请返回了意外结果: {other:?}"),
    }
    session.shutdown();
    Ok(())
}

/// Preview the current invitation terms without creating a local membership.
pub async fn preview_invitation(
    network_id: &str,
    coordinator_url: &str,
    code: &str,
    json: bool,
) -> Result<()> {
    let command = NodePreviewInvitationCommand {
        network_id: invitation_network_id(network_id)?,
        coordinator_url: coordinator_url.to_owned(),
        code: InvitationCode::parse(code.trim()).context("邀请码格式无效")?,
    };
    let session = cli_node_session().await?;
    let outcome =
        execute_cli_node_command(&session, NodeCommand::PreviewInvitation(command)).await?;
    match outcome {
        NodeCommandOutcome::InvitationPreview(preview) => {
            if json {
                println!("{}", serde_json::to_string_pretty(&preview)?);
            } else {
                print_invitation_preview(&preview);
            }
        }
        other => anyhow::bail!("预览邀请返回了意外结果: {other:?}"),
    }
    session.shutdown();
    Ok(())
}

/// Re-preview and redeem an invitation with an explicit acknowledgement.
pub async fn redeem_invitation(
    network_id: &str,
    coordinator_url: &str,
    code: &str,
    accept_exit_contribution: bool,
    yes: bool,
    json: bool,
) -> Result<()> {
    if !yes {
        anyhow::bail!("兑换邀请前必须复核条款并显式传入 --yes");
    }
    let network_id = invitation_network_id(network_id)?;
    let code = InvitationCode::parse(code.trim()).context("邀请码格式无效")?;
    let session = cli_node_session().await?;
    let preview_outcome = execute_cli_node_command(
        &session,
        NodeCommand::PreviewInvitation(NodePreviewInvitationCommand {
            network_id: network_id.clone(),
            coordinator_url: coordinator_url.to_owned(),
            code: code.clone(),
        }),
    )
    .await?;
    let NodeCommandOutcome::InvitationPreview(preview) = preview_outcome else {
        anyhow::bail!("复核邀请返回了意外结果: {preview_outcome:?}");
    };
    if !preview.can_accept
        || preview.denial_key.is_some()
        || !preview.state.is_active()
        || preview.remaining_uses == 0
        || preview.expires_at_unix_ms <= unix_time_ms()
    {
        anyhow::bail!(
            "邀请当前不可接受: {}",
            preview
                .denial_key
                .as_deref()
                .unwrap_or("已过期、撤销或耗尽")
        );
    }
    let requires_exit_contribution = preview
        .terms
        .exit_contribution_mode
        .requires_join_commitment();
    if requires_exit_contribution && !accept_exit_contribution {
        anyhow::bail!(
            "该网络要求加入时贡献 Exit 能力，请复核条款后传入 --accept-exit-contribution"
        );
    }

    let acknowledged_at_unix_ms = unix_time_ms();
    let redeem = NodeRedeemInvitationCommand {
        coordinator_url: coordinator_url.to_owned(),
        request: InvitationRedeemRequest {
            network_id: preview.network_id.clone(),
            code,
            invitation_id: preview.invitation_id.clone(),
            expected_invitation_revision: preview.invitation_revision,
            preview_digest: preview.preview_digest.clone(),
            terms_hash: preview.terms_hash.clone(),
            acknowledged_at_unix_ms,
            exit_contribution_acknowledged: Some(requires_exit_contribution),
        },
    };
    let outcome = execute_cli_node_command(&session, NodeCommand::RedeemInvitation(redeem)).await?;
    match outcome {
        NodeCommandOutcome::InvitationRedeemed(redemption) => {
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "preview": &preview,
                        "redemption": &redemption,
                    }))?
                );
            } else {
                print_invitation_preview(&preview);
                println!();
                println!("邀请已兑换，网络成员关系已保存");
                println!("节点 ID:    {}", redemption.node_id);
                println!("角色:       {:?}", redemption.role);
                println!("加入时间:   {}", redemption.joined_at_unix_ms);
            }
        }
        other => anyhow::bail!("兑换邀请返回了意外结果: {other:?}"),
    }
    session.shutdown();
    Ok(())
}

fn print_invitation_preview(preview: &fuckipv6_common::domain::InvitationPreview) {
    println!("邀请条款预览");
    println!(
        "网络:       {} ({})",
        preview.network_name, preview.network_id
    );
    println!("邀请 ID:    {}", preview.invitation_id);
    println!("角色:       {:?}", preview.role);
    println!("能力:       {:?}", preview.capabilities);
    println!("剩余次数:   {}", preview.remaining_uses);
    println!("过期时间:   {} ms", preview.expires_at_unix_ms);
    println!("状态:       {:?}", preview.state);
    println!(
        "网络条款:   revision={} privacy={}",
        preview.terms.network_policy_revision, preview.terms.privacy_policy_revision
    );
    println!("Exit 义务:  {:?}", preview.terms.exit_contribution_mode);
    println!("条款哈希:   {}", preview.terms_hash.as_str());
    println!("预览摘要:   {}", preview.preview_digest.as_str());
    println!("可接受:     {}", preview.can_accept);
}

/// Enable or disable the local owner Exit intent.
///
/// This is independent from the network Exit role: the data plane only becomes
/// effective when the owner intent and the network authorization are both true.
pub async fn set_local_exit(enabled: bool) -> Result<()> {
    let session = cli_node_session().await?;
    let outcome = execute_cli_node_command(
        &session,
        NodeCommand::SetLocalExitCapability(NodeSetLocalExitCapabilityCommand { enabled }),
    )
    .await?;
    match outcome {
        NodeCommandOutcome::LocalExitCapability(snapshot) => {
            println!("{}", serde_json::to_string_pretty(&snapshot)?);
        }
        other => anyhow::bail!("Exit 能力命令返回了意外结果: {other:?}"),
    }
    session.shutdown();
    Ok(())
}

/// Print the current local Exit capability projection from the node snapshot.
pub async fn local_exit_status(json: bool) -> Result<()> {
    let session = cli_node_session().await?;
    let snapshot = session.state().snapshot().await;
    let local_exit = snapshot.local_exit;
    if json {
        println!("{}", serde_json::to_string_pretty(&local_exit)?);
        session.shutdown();
        return Ok(());
    }

    println!("本机 Exit 能力 (Local Exit Capability)");
    println!("─────────────────────────────────────");
    println!("Owner intent:    {}", local_exit.owner_exit_enabled);
    println!("网络授权:        {}", local_exit.network_allowed);
    println!("阶段:            {:?}", local_exit.stage);
    println!("健康:            {}", local_exit.healthy);
    println!(
        "客户端:          活动 {} / 累计 {}",
        local_exit.active_clients, local_exit.total_clients
    );
    if let Some(problem) = &local_exit.problem {
        println!("问题:            {:?}: {}", problem.code, problem.message);
    }
    session.shutdown();
    Ok(())
}

/// Print peers from the same canonical snapshot consumed by WebUI and GPUI.
pub async fn node_peers() -> Result<()> {
    let session = cli_node_session().await?;
    let snapshot = session.state().snapshot().await;
    let peers = snapshot
        .peers
        .iter()
        .filter(|peer| peer.online)
        .collect::<Vec<_>>();

    println!("已连接对等节点 (Connected Peers)");
    println!("─────────────────────────────────────");
    println!();

    if peers.is_empty() {
        println!("当前无已连接的对等节点。");
        session.shutdown();
        return Ok(());
    }

    println!(
        "{:<12}  {:<18}  {:<26}  {:<10}  {:<8}  {:<8}  {:<10}  {:<12}  公网物理端点",
        "所属网络",
        "虚拟 IP",
        "客户端标识 (Client ID)",
        "链路模式",
        "往返时延",
        "丢包率",
        "收/发包",
        "流量字节"
    );
    println!("{}", "─".repeat(150));

    for peer in peers {
        let path = if peer.p2p_active && peer.p2p_healthy {
            "P2P直连"
        } else {
            "QUIC中继"
        };
        let rtt = peer
            .rtt_us
            .map(format_rtt)
            .unwrap_or_else(|| "-".to_string());
        let packets = format!(
            "{}/{}",
            peer.traffic.p2p_rx_packets, peer.traffic.p2p_tx_packets
        );
        let traffic_bytes = peer
            .traffic
            .p2p_rx_bytes
            .saturating_add(peer.traffic.p2p_tx_bytes);
        let endpoint = peer
            .endpoints
            .first()
            .map(|addr| addr.to_string())
            .unwrap_or_else(|| "-".to_string());

        println!(
            "{:<12}  {:<18}  {:<26}  {:<10}  {:<8}  {:<8}  {:<10}  {:<12}  {}",
            peer.network_name,
            peer.virtual_ipv6,
            abbreviate(&peer.peer_id, 26),
            path,
            rtt,
            format!("{:.1}%", peer.loss_rate * 100.0),
            packets,
            traffic_bytes,
            endpoint
        );
    }

    session.shutdown();
    Ok(())
}

fn select_probe_network<'a>(
    snapshot: &'a NodeSnapshot,
    target: &str,
) -> Result<&'a NodeNetworkSnapshot> {
    if let Some(peer) = snapshot.peers.iter().find(|peer| {
        peer.peer_id == target
            || peer.peer_id.starts_with(target)
            || peer.virtual_ipv6.to_string() == target
            || peer.virtual_ipv4.is_some_and(|ip| ip.to_string() == target)
    }) {
        return snapshot
            .networks
            .iter()
            .find(|network| network.name == peer.network_name)
            .with_context(|| format!("匹配到 Peer，但其网络 {} 当前未运行", peer.network_name));
    }

    snapshot
        .networks
        .iter()
        .find(|network| {
            matches!(
                network.status,
                NodeNetworkStatus::Online
                    | NodeNetworkStatus::Degraded
                    | NodeNetworkStatus::Connecting
            )
        })
        .or_else(|| snapshot.networks.first())
        .context("当前没有可探测的网络")
}

/// Probe a peer through the node runtime instead of shelling out to system ping.
pub async fn node_ping(target: &str, count: u32) -> Result<()> {
    let target = target.trim();
    if target.is_empty() {
        anyhow::bail!("探测目标不能为空");
    }

    let session = cli_node_session().await?;
    let snapshot = session.state().snapshot().await;
    let network = select_probe_network(&snapshot, target)?;
    let network_name = network.name.clone();
    let attempts = count.max(1);
    let mut succeeded = 0_u32;
    let context = NodeCommandContext::local_owner_for_snapshot(&snapshot, "cli_ping");

    println!("正在通过节点运行时探测: {target} (网络: {network_name})");
    println!("─────────────────────────────────────");

    for attempt in 1..=attempts {
        let result = session
            .commands()
            .execute(
                context.clone(),
                NodeCommandRequest::new(
                    u64::from(attempt),
                    NodeCommand::ProbePeer(NodeProbePeerCommand {
                        network_name: network_name.clone(),
                        target: target.to_string(),
                        timeout_ms: 3_000,
                    }),
                ),
            )
            .await
            .map_err(|error| anyhow::anyhow!(error.to_string()))?;

        match result.outcome {
            NodeCommandOutcome::Probe(probe) => {
                if probe.timed_out {
                    println!("第 {attempt} 次: 超时");
                } else {
                    succeeded = succeeded.saturating_add(1);
                    println!(
                        "第 {attempt} 次: 成功 · {} · {} · RTT {} · endpoint {}",
                        probe.peer_id,
                        if probe.p2p_active {
                            "P2P直连"
                        } else {
                            "QUIC中继"
                        },
                        probe
                            .rtt_us
                            .map(format_rtt)
                            .unwrap_or_else(|| "-".to_string()),
                        probe
                            .endpoint
                            .map(|addr| addr.to_string())
                            .unwrap_or_else(|| "-".to_string())
                    );
                }
            }
            NodeCommandOutcome::Rejected(problem) => anyhow::bail!(problem.message),
            other => anyhow::bail!("Peer 探测返回了意外结果: {other:?}"),
        }

        if attempt < attempts {
            sleep(Duration::from_secs(1)).await;
        }
    }

    session.shutdown();
    if succeeded == 0 {
        anyhow::bail!("全部 {attempts} 次节点运行时探测均失败");
    }
    println!();
    println!("探测完成: {succeeded}/{attempts} 次成功");
    Ok(())
}

/// Print canonical runtime counters for every active network.
pub async fn node_stats() -> Result<()> {
    let session = cli_node_session().await?;
    let snapshot = session.state().snapshot().await;

    println!("网络统计信息 (Node Runtime Statistics)");
    println!("─────────────────────────────────────");

    if snapshot.networks.is_empty() {
        println!();
        println!("当前没有活跃网络，暂无运行时流量统计。");
        session.shutdown();
        return Ok(());
    }

    for network in &snapshot.networks {
        println!();
        println!("🌐 {}", network.name);
        println!(
            "   状态:       {}",
            node_network_status_label(network.status)
        );
        println!("   运行时间:   {}", format_duration(network.uptime_secs));
        println!(
            "   TUN:        {} 包收 / {} 包发 · {} 字节收 / {} 字节发",
            network.traffic.tun_rx_packets,
            network.traffic.tun_tx_packets,
            network.traffic.tun_rx_bytes,
            network.traffic.tun_tx_bytes
        );
        println!(
            "   P2P:        {} 包收 / {} 包发 · {} 字节收 / {} 字节发",
            network.traffic.p2p_rx_packets,
            network.traffic.p2p_tx_packets,
            network.traffic.p2p_rx_bytes,
            network.traffic.p2p_tx_bytes
        );
        println!("   错误计数:   {}", network.traffic.error_count);
    }

    session.shutdown();
    Ok(())
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
    use fuckipv6_common::domain::RelayAuthorizationDenial;
    use std::path::{Path, PathBuf};
    use std::time::{SystemTime, UNIX_EPOCH};
    use tokio::sync::Notify;

    #[test]
    fn relay_grant_renews_only_inside_the_ahead_window() {
        let now = 1_700_000_000_000;
        let renew_ahead_ms =
            u64::try_from(RELAY_MEMBER_GRANT_RENEW_AHEAD.as_millis()).unwrap_or(u64::MAX);
        assert!(!relay_grant_renewal_due(now + renew_ahead_ms + 1, now));
        assert!(relay_grant_renewal_due(now + renew_ahead_ms, now));
        assert!(relay_grant_renewal_due(now.saturating_sub(1), now));
    }

    #[test]
    fn renewed_relay_grant_must_extend_the_same_binding() {
        let identity = Identity::generate().expect("identity");
        let consumer = NodeId::from_public_key(&identity.public_key());
        let network = NetworkId::from_public_key(b"renewal-network");
        let provider = NodeId::from_public_key(b"renewal-provider");
        let other_provider = NodeId::from_public_key(b"other-provider");
        let current = RelayMemberGrant::new(
            network.clone(),
            provider.clone(),
            &identity,
            "current",
            1,
            1_000,
            2_000,
            RelayLimitPolicy::default(),
        )
        .expect("current grant");
        let renewed = RelayMemberGrant::new(
            network,
            provider,
            &identity,
            "renewed",
            2,
            2_000,
            3_000,
            RelayLimitPolicy::default(),
        )
        .expect("renewed grant");
        let other_provider_grant = RelayMemberGrant::new(
            NetworkId::from_public_key(b"renewal-network"),
            other_provider,
            &identity,
            "other-provider",
            2,
            2_000,
            3_000,
            RelayLimitPolicy::default(),
        )
        .expect("other provider grant");

        assert_eq!(
            renewed_relay_grant(std::slice::from_ref(&renewed), &current),
            Some(&renewed)
        );
        assert!(renewed_relay_grant(std::slice::from_ref(&current), &current).is_none());
        assert!(renewed_relay_grant(&[other_provider_grant], &current).is_none());
        assert_eq!(renewed.consumer_node_id, consumer);
    }

    fn relay_selection_policy(max_attempts: u32) -> RelaySelectionPolicy {
        RelaySelectionPolicy {
            max_attempts,
            max_health_age_ms: RELAY_SELECTION_HEALTH_MAX_AGE_MS,
            max_consecutive_failures: RELAY_SELECTION_MAX_CONSECUTIVE_FAILURES,
            allow_fallback: true,
            switch_cooldown_ms: RELAY_SELECTION_SWITCH_COOLDOWN_MS,
        }
    }

    fn relay_endpoint(provider: &NodeId, port: u16) -> RelayEndpoint {
        RelayEndpoint {
            addr: format!("127.0.0.1:{port}").parse().expect("relay address"),
            server_name: format!("relay-{port}.test"),
            public_key_pin: [port as u8; 32],
            provider_node_id: Some(provider.clone()),
        }
    }

    fn relay_grant(
        identity: &Identity,
        network_id: &NetworkId,
        provider: &NodeId,
        nonce: &str,
    ) -> RelayMemberGrant {
        RelayMemberGrant::new(
            network_id.clone(),
            provider.clone(),
            identity,
            nonce,
            1,
            1_000,
            u64::MAX,
            RelayLimitPolicy::default(),
        )
        .expect("relay grant")
    }

    fn relay_failover_test_dir(label: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let dir =
            std::env::temp_dir().join(format!("fuckipv6-{label}-{}-{nonce}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create audit test directory");
        dir
    }

    #[test]
    fn relay_failover_audit_entry_uses_canonical_action_and_minimal_fields() {
        let actor = NodeId::from_public_key(b"relay-failover-consumer");
        let network_id = NetworkId::from_public_key(b"relay-failover-network");
        let failed_provider = NodeId::from_public_key(b"relay-failover-provider-old");
        let provider = NodeId::from_public_key(b"relay-failover-provider-new");
        let entry = RelayFailoverAuditEvent {
            actor,
            network_id: Some(network_id.clone()),
            failed_generation: 7,
            installed_generation: 9,
            failed_provider_node_id: Some(failed_provider.clone()),
            provider_node_id: provider.clone(),
            failure_reason: RelayFailureReason::ForwardSendFailed,
            occurred_at_unix_ms: 1_700_000_000_000,
        }
        .audit_entry();

        assert_eq!(entry.action, RelayGovernanceAction::Failover.wire_name());
        assert_eq!(entry.resource, "relay_session");
        assert_eq!(entry.details["failed_generation"], "7");
        assert_eq!(entry.details["installed_generation"], "9");
        assert_eq!(
            entry.details["failed_provider_node_id"],
            failed_provider.to_string()
        );
        assert_eq!(entry.details["provider_node_id"], provider.to_string());
        assert_eq!(
            entry.details["reason_code"],
            "relay.failure.forward_send_failed"
        );
        assert_eq!(entry.network_id, Some(network_id));
        assert_eq!(entry.details.len(), 10);

        let serialized = serde_json::to_string(&entry).expect("serialize audit entry");
        for forbidden in [
            "127.0.0.1",
            "relay.test",
            "payload",
            "private_key",
            "session_secret",
        ] {
            assert!(!serialized.contains(forbidden), "audit leaked {forbidden}");
        }
    }

    #[test]
    fn relay_failover_audit_sink_drops_without_blocking_when_full() {
        let actor = NodeId::from_public_key(b"relay-failover-queue-consumer");
        let provider = NodeId::from_public_key(b"relay-failover-queue-provider");
        let (sender, _receiver) = mpsc::channel(1);
        let sink = RelayFailoverAuditSink {
            actor,
            network_id: None,
            sender: Some(sender),
            dropped: Arc::new(AtomicU64::new(0)),
        };

        assert!(sink.try_record(
            1,
            2,
            None,
            provider.clone(),
            RelayFailureReason::ReceiveStreamClosed,
        ));
        assert!(!sink.try_record(2, 3, None, provider, RelayFailureReason::ForwardSendTimeout,));
        assert_eq!(sink.dropped_count(), 1);
    }

    #[test]
    fn pending_relay_failover_pairs_only_with_a_newer_generation_once() {
        let pending = PendingRelayFailover::default();
        let failed_provider = NodeId::from_public_key(b"relay-pending-failed-provider");
        assert!(pending.record(RelayFailureContext {
            failed_generation: 4,
            provider_node_id: Some(failed_provider.clone()),
            reason: RelayFailureReason::ForwardSendTimeout,
        }));
        assert!(!pending.record(RelayFailureContext {
            failed_generation: 5,
            provider_node_id: None,
            reason: RelayFailureReason::ReceiveStreamClosed,
        }));
        assert!(pending.take_after_install(4).is_none());
        assert!(pending.take_after_install(3).is_none());
        assert_eq!(
            pending.take_after_install(5),
            Some(RelayFailureContext {
                failed_generation: 4,
                provider_node_id: Some(failed_provider),
                reason: RelayFailureReason::ForwardSendTimeout,
            })
        );
        assert!(pending.take_after_install(6).is_none());
    }

    #[tokio::test]
    async fn relay_failover_audit_worker_persists_and_merges_activity() {
        let dir = relay_failover_test_dir("relay-failover-worker");
        let actor = NodeId::from_public_key(b"relay-failover-worker-consumer");
        let failed_provider = NodeId::from_public_key(b"relay-failover-worker-provider-old");
        let provider = NodeId::from_public_key(b"relay-failover-worker-provider-new");
        let state = NodeStateStore::new();
        let audit = Arc::new(OwnerSecurityAudit::new(&dir, actor.clone()).expect("audit ledger"));
        let sink =
            spawn_relay_failover_audit_worker(audit.clone(), Some(state.clone()), actor, None);

        assert!(sink.try_record(
            11,
            12,
            Some(failed_provider),
            provider,
            RelayFailureReason::ReceiveStreamClosed,
        ));
        let activity = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let activity = state.snapshot().await.recent_activity;
                if !activity.is_empty() {
                    break activity;
                }
                sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("activity projection is published");
        assert_eq!(activity.len(), 1);
        assert_eq!(
            activity[0].action,
            RelayGovernanceAction::Failover.wire_name()
        );
        assert_eq!(
            activity[0].reason.as_deref(),
            Some("relay.failure.receive_stream_closed")
        );

        let export = audit
            .export(&fuckipv6_common::domain::AuditExportFilter::default())
            .expect("export audit");
        assert_eq!(export.records.len(), 1);
        assert_eq!(
            export.records[0].entry.action,
            RelayGovernanceAction::Failover.wire_name()
        );

        drop(sink);
        drop(audit);
        std::fs::remove_dir_all(dir).expect("remove audit test directory");
    }

    #[test]
    fn relay_connect_candidates_require_matching_provider_grants() {
        let identity = Identity::generate().expect("identity");
        let network_id = NetworkId::from_public_key(b"relay-selection-network");
        let provider = NodeId::from_public_key(b"relay-provider-a");
        let other_provider = NodeId::from_public_key(b"relay-provider-b");
        let endpoints = vec![
            relay_endpoint(&provider, 41001),
            relay_endpoint(&other_provider, 41002),
        ];
        let grants = vec![relay_grant(&identity, &network_id, &provider, "provider-a")];
        let policy = relay_selection_policy(RELAY_SELECTION_MAX_ATTEMPTS);
        let tracker = RelaySelectionTracker::default();

        let candidates =
            relay_connect_candidates(&tracker, &endpoints, &grants, &identity, 10_000, &policy);
        assert_eq!(candidates.len(), 1);
        assert_eq!(
            candidates[0].endpoint.provider_node_id.as_ref(),
            Some(&provider)
        );
        assert_eq!(candidates[0].candidate.health, RelayHealthStatus::Degraded);

        let outcome = select_relay_candidate(&[candidates[0].candidate.clone()], &policy, 10_000)
            .expect("selection");
        assert!(outcome.is_usable());
        assert_eq!(outcome.attempts_allowed, 1);
    }

    #[test]
    fn relay_selection_falls_back_then_revalidates_after_cooldown() {
        let identity = Identity::generate().expect("identity");
        let network_id = NetworkId::from_public_key(b"relay-fallback-network");
        let first_provider = NodeId::from_public_key(b"relay-provider-first");
        let second_provider = NodeId::from_public_key(b"relay-provider-second");
        let endpoints = vec![
            relay_endpoint(&first_provider, 42001),
            relay_endpoint(&second_provider, 42002),
        ];
        let grants = vec![
            relay_grant(&identity, &network_id, &first_provider, "first"),
            relay_grant(&identity, &network_id, &second_provider, "second"),
        ];
        let policy = relay_selection_policy(2);
        let mut tracker = RelaySelectionTracker::default();
        let now = 100_000;
        let candidates =
            relay_connect_candidates(&tracker, &endpoints, &grants, &identity, now, &policy);
        let first_id = candidates[0].candidate.relay_id.clone();
        let second_id = candidates[1].candidate.relay_id.clone();

        tracker.mark_failure(&first_id, now);
        let cooled =
            relay_connect_candidates(&tracker, &endpoints, &grants, &identity, now + 100, &policy);
        let outcome = select_relay_candidate(
            &cooled
                .iter()
                .map(|candidate| candidate.candidate.clone())
                .collect::<Vec<_>>(),
            &policy,
            now + 100,
        )
        .expect("cooldown selection");
        assert_eq!(
            outcome.candidates[0].relay_id, second_id,
            "the failed Relay must not be retried during cooldown"
        );
        assert!(outcome.denied.iter().any(|denial| {
            denial.relay_id == first_id && denial.reason == RelayAuthorizationDenial::Cooldown
        }));

        let recovering = relay_connect_candidates(
            &tracker,
            &endpoints,
            &grants,
            &identity,
            now + RELAY_SELECTION_SWITCH_COOLDOWN_MS,
            &policy,
        );
        let first = recovering
            .iter()
            .find(|candidate| candidate.candidate.relay_id == first_id)
            .expect("recovery candidate");
        assert_eq!(first.candidate.health, RelayHealthStatus::Degraded);
        assert_eq!(first.candidate.consecutive_failures, 0);

        tracker.mark_connected(&first_id, 25);
        tracker.mark_forwarding_success(&first_id);
        let healthy =
            tracker.candidate(&first_id, now + RELAY_SELECTION_SWITCH_COOLDOWN_MS, &policy);
        assert_eq!(healthy.health, RelayHealthStatus::Healthy);
        assert_eq!(healthy.last_attempt_at_unix_ms, None);
    }

    #[test]
    fn relay_selection_attempt_budget_is_hard_bounded() {
        let identity = Identity::generate().expect("identity");
        let network_id = NetworkId::from_public_key(b"relay-budget-network");
        let policy = relay_selection_policy(3);
        let tracker = RelaySelectionTracker::default();
        let mut endpoints = Vec::new();
        let mut grants = Vec::new();
        for index in 0..8u16 {
            let provider = NodeId::from_public_key(format!("relay-provider-{index}").as_bytes());
            endpoints.push(relay_endpoint(&provider, 43_000 + index));
            grants.push(relay_grant(
                &identity,
                &network_id,
                &provider,
                &format!("provider-{index}"),
            ));
        }
        let candidates =
            relay_connect_candidates(&tracker, &endpoints, &grants, &identity, 200_000, &policy);
        let outcome = select_relay_candidate(
            &candidates
                .iter()
                .map(|candidate| candidate.candidate.clone())
                .collect::<Vec<_>>(),
            &policy,
            200_000,
        )
        .expect("bounded selection");
        assert_eq!(candidates.len(), 8);
        assert_eq!(outcome.attempts_allowed, 3);
        assert_eq!(outcome.candidates.len(), 3);
    }

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

    #[derive(Debug)]
    struct RoutedFallbackCall {
        src_ip: IpAddr,
        dst_ip: IpAddr,
        encrypted: Bytes,
    }

    struct RoutedFallbackTransport {
        relay_tx: mpsc::UnboundedSender<RoutedFallbackCall>,
        signaling_tx: mpsc::UnboundedSender<RoutedFallbackCall>,
    }

    impl FallbackTransport for RoutedFallbackTransport {
        fn relay<'a>(
            &'a self,
            src_ip: Ipv6Addr,
            dst_ip: Ipv6Addr,
            encrypted: Bytes,
        ) -> Pin<Box<dyn Future<Output = RelayForwardOutcome> + Send + 'a>> {
            Box::pin(async move {
                let _ = self.relay_tx.send(RoutedFallbackCall {
                    src_ip: IpAddr::V6(src_ip),
                    dst_ip: IpAddr::V6(dst_ip),
                    encrypted,
                });
                RelayForwardOutcome::Failed(anyhow::anyhow!("injected relay failure"))
            })
        }

        fn signaling(&self, src_ip: IpAddr, dst_ip: IpAddr, encrypted: Bytes) -> Result<()> {
            let _ = self.signaling_tx.send(RoutedFallbackCall {
                src_ip,
                dst_ip,
                encrypted,
            });
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

    async fn connected_test_engines_with_ips(
        virtual_ips_a: VirtualIps,
        virtual_ips_b: VirtualIps,
    ) -> Result<(P2pEngine, P2pEngine, Ipv6Addr, Ipv6Addr)> {
        let engine_a = P2pEngine::new(0).await?;
        let engine_b = P2pEngine::new(0).await?;
        let identity_a = Identity::generate()?;
        let identity_b = Identity::generate()?;
        let ip_a = virtual_ips_a.primary_ipv6();
        let ip_b = virtual_ips_b.primary_ipv6();

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
            .add_peer(PeerInfo::new_with_ips(
                "b".to_string(),
                virtual_ips_b,
                identity_b.public_key(),
                vec![endpoint_b],
                NatType::Open,
            ))
            .await?;
        engine_b
            .add_peer(PeerInfo::new_with_ips(
                "a".to_string(),
                virtual_ips_a,
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

    async fn connected_test_engines() -> Result<(P2pEngine, P2pEngine, Ipv6Addr, Ipv6Addr)> {
        let ip_a = "fd00::1".parse::<Ipv6Addr>()?;
        let ip_b = "fd00::2".parse::<Ipv6Addr>()?;
        connected_test_engines_with_ips(VirtualIps::from_ipv6(ip_a), VirtualIps::from_ipv6(ip_b))
            .await
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
    async fn outbound_stats_use_canonical_peer_identity_for_ipv4_destination() -> Result<()> {
        let virtual_ipv4: Ipv4Addr = "10.8.0.2".parse()?;
        let virtual_ipv6: Ipv6Addr = "fd00::2".parse()?;
        let (engine_a, engine_b, ip_a, _) = connected_test_engines_with_ips(
            VirtualIps::from_ipv6("fd00::1".parse()?),
            VirtualIps::dual_stack(virtual_ipv4, virtual_ipv6),
        )
        .await?;
        let (relay_tx, _relay_rx) = mpsc::unbounded_channel();
        let (signaling_tx, _signaling_rx) = mpsc::unbounded_channel();
        let transport = P2pOutboundTransport::with_fallback(
            engine_a,
            Arc::new(MatrixFallbackTransport {
                relay_mode: RelayMode::Success,
                relay_tx,
                signaling_tx,
            }),
        );
        let payload = b"stats must use the peer's canonical IPv6 identity";

        transport
            .send_packet(OutboundPacket::owned(
                IpAddr::V6(ip_a),
                IpAddr::V4(virtual_ipv4),
                payload.to_vec(),
            ))
            .await;

        let inbound = tokio::time::timeout(Duration::from_secs(1), engine_b.recv())
            .await?
            .expect("the IPv4 destination must resolve to the authenticated IPv6 peer");
        let InboundPacket::AuthenticatedPlaintext {
            src_ip,
            payload: received,
            ..
        } = inbound
        else {
            panic!("direct P2P delivery must produce authenticated plaintext")
        };
        assert_eq!(src_ip, IpAddr::V6(ip_a));
        assert_eq!(received.as_slice(), payload);

        let snapshot = transport
            .stats
            .get_peer_stat(virtual_ipv6)
            .await
            .expect("outbound statistics must use the canonical IPv6 peer key");
        assert_eq!(snapshot.tx_packets, 1);
        assert_eq!(snapshot.tx_bytes, payload.len() as u64);
        assert!(
            transport.stats.get_peer_stat(virtual_ipv4).await.is_none(),
            "the IPv4 destination must not create an invisible peer statistics bucket"
        );
        assert_eq!(transport.stats.get_global_stats().await.p2p_tx_packets, 1);
        Ok(())
    }
    #[tokio::test]
    async fn relay_failover_rewrites_spoofed_source_and_preserves_selected_exit() -> Result<()> {
        let (engine_a, _engine_b, ip_a, ip_b) = connected_test_engines().await?;
        let exit_peer = engine_a
            .get_peers()
            .await
            .into_iter()
            .find(|peer| peer.virtual_ip == ip_b)
            .context("connected test peer must be present")?;
        let exit_node_id = NodeId::from_public_key(&exit_peer.public_key);
        let assigned_ips = VirtualIps::dual_stack("10.8.0.2".parse()?, ip_a);
        let route_engine = SmartRouteEngine::new(fuckipv6_common::RoutingMode::GlobalProxy);
        let spoofed_source: Ipv6Addr = "2001:db8::77".parse()?;
        let public_destination: Ipv6Addr = "2606:4700:4700::1111".parse()?;
        let mut packet = [0u8; 40];
        packet[0] = 0x60;
        packet[6] = 64;
        packet[8..24].copy_from_slice(&spoofed_source.octets());
        packet[24..40].copy_from_slice(&public_destination.octets());

        assert!(
            route_tun_packet(&engine_a, &assigned_ips, &route_engine, None, &packet)
                .await
                .is_none(),
            "a public Proxy destination must fail closed without an authorized Exit"
        );
        let routed = route_tun_packet(
            &engine_a,
            &assigned_ips,
            &route_engine,
            Some(&exit_node_id),
            &packet,
        )
        .await
        .context("authorized Exit must route the public destination")?;
        assert_eq!(routed, (IpAddr::V6(ip_a), IpAddr::V6(ip_b)));
        assert_ne!(
            routed.0,
            IpAddr::V6(spoofed_source),
            "the source supplied to Relay must be the assigned virtual address"
        );

        let (relay_tx, mut relay_rx) = mpsc::unbounded_channel();
        let (signaling_tx, mut signaling_rx) = mpsc::unbounded_channel();
        let transport = P2pOutboundTransport::with_fallback(
            engine_a.clone(),
            Arc::new(RoutedFallbackTransport {
                relay_tx,
                signaling_tx,
            }),
        );
        engine_a.set_fail_direct_transport(true);
        transport
            .send_packet(OutboundPacket::owned(routed.0, routed.1, packet.to_vec()))
            .await;
        engine_a.set_fail_direct_transport(false);

        let relay_call = relay_rx
            .try_recv()
            .expect("failed direct path must try the QUIC Relay");
        let signaling_call = signaling_rx
            .try_recv()
            .expect("failed QUIC Relay must try signaling");
        for call in [&relay_call, &signaling_call] {
            assert_eq!(call.src_ip, IpAddr::V6(ip_a));
            assert_eq!(call.dst_ip, IpAddr::V6(ip_b));
            assert_ne!(call.src_ip, IpAddr::V6(spoofed_source));
        }
        assert_eq!(relay_call.encrypted, signaling_call.encrypted);
        assert_eq!(
            relay_call.encrypted.as_ptr(),
            signaling_call.encrypted.as_ptr(),
            "Relay failover must preserve one sealed ciphertext allocation"
        );
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

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn outbound_worker_timeout_preserves_owner_and_retry_succeeds() {
        let dispatcher = OutboundDispatcher::for_test(Arc::new(RecordingTransport {
            completed_tx: mpsc::unbounded_channel().0,
        }));
        let peer = Ipv6Addr::new(0xfd00, 0, 0, 0, 0, 0, 0, 2);
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let task = tokio::spawn(async move {
            let _ = started_tx.send(());
            // Blocking inside a poll cannot be interrupted by JoinHandle::abort,
            // which lets this test exercise timeout ownership retention.
            let _ = release_rx.recv();
        });
        started_rx.await.expect("worker must start");
        dispatcher
            .workers
            .lock()
            .expect("outbound worker registry poisoned")
            .insert(IpAddr::V6(peer), OutboundWorker { sender: None, task });

        let first = tokio::time::timeout(
            Duration::from_secs(1),
            dispatcher.close_workers(Duration::from_millis(25)),
        )
        .await
        .expect("bounded close must return before the test timeout");
        release_tx.send(()).expect("worker must still own receiver");
        assert!(matches!(first, CleanupStatus::Deferred(_)));
        assert_eq!(
            dispatcher
                .workers
                .lock()
                .expect("outbound worker registry poisoned")
                .len(),
            1,
            "a timed-out worker must remain owned for retry"
        );

        let second = tokio::time::timeout(
            Duration::from_secs(1),
            dispatcher.close_workers(Duration::from_secs(1)),
        )
        .await
        .expect("retry must be bounded");
        assert_eq!(
            second,
            CleanupStatus::Removed,
            "released worker must be joined on retry"
        );
        assert!(dispatcher
            .workers
            .lock()
            .expect("outbound worker registry poisoned")
            .is_empty());
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
            exit_forwarding_action(true, false, false),
            ExitForwardingAction::Install
        );
        assert_eq!(
            exit_forwarding_action(true, true, false),
            ExitForwardingAction::Keep
        );
        assert_eq!(
            exit_forwarding_action(false, true, false),
            ExitForwardingAction::Drain
        );
        assert_eq!(
            exit_forwarding_action(false, false, false),
            ExitForwardingAction::Keep
        );
        assert_eq!(
            exit_forwarding_action(true, true, true),
            ExitForwardingAction::Resume
        );
        assert_eq!(
            exit_forwarding_action(false, true, true),
            ExitForwardingAction::Keep
        );
    }

    #[test]
    fn local_exit_runtime_stage_keeps_drain_and_failure_distinct() {
        assert_eq!(
            local_exit_runtime_stage(false, false, false, false),
            ipc::LocalExitRuntimeStage::Disabled
        );
        assert_eq!(
            local_exit_runtime_stage(true, true, false, false),
            ipc::LocalExitRuntimeStage::Enabled
        );
        assert_eq!(
            local_exit_runtime_stage(false, true, true, false),
            ipc::LocalExitRuntimeStage::Draining
        );
        assert_eq!(
            local_exit_runtime_stage(true, true, true, false),
            ipc::LocalExitRuntimeStage::Draining
        );
        assert_eq!(
            local_exit_runtime_stage(true, false, false, false),
            ipc::LocalExitRuntimeStage::Failed
        );
        assert_eq!(
            local_exit_runtime_stage(false, true, false, true),
            ipc::LocalExitRuntimeStage::Failed
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn local_exit_forwarder_failures_preserve_structured_cause() {
        for (message, expected, retryable, expected_message_key) in [
            (
                "iptables: Permission denied (you must be root)",
                NodeProblemCode::PermissionDenied,
                false,
                "error.permission_denied",
            ),
            (
                "can't initialize iptables table `nat': Table does not exist",
                NodeProblemCode::Unsupported,
                false,
                "error.operation_unsupported",
            ),
            (
                "iptables: Resource temporarily unavailable (os error 11)",
                NodeProblemCode::ResourceExhausted,
                true,
                "error.operation_resource_exhausted",
            ),
            (
                "write failed: No space left on device (os error 28)",
                NodeProblemCode::ResourceExhausted,
                true,
                "error.operation_resource_exhausted",
            ),
            (
                "injected forwarding failure",
                NodeProblemCode::Unavailable,
                true,
                "error.operation_unavailable",
            ),
        ] {
            let problem = local_exit_forwarder_problem(message);
            assert_eq!(problem.code, expected, "{message}");
            assert_eq!(problem.retryable, retryable, "{message}");
            let error = local_exit_forwarder_runtime_error(&problem);
            assert_eq!(error.problem_code(), expected, "{message}");
            assert_eq!(error.retryable(), retryable, "{message}");
            assert_eq!(error.message_key(), expected_message_key, "{message}");
        }
    }

    #[test]
    fn local_exit_veto_overrides_network_exit_role() {
        assert!(local_exit_effective(true, true));
        assert!(!local_exit_effective(false, true));
        assert!(!local_exit_effective(true, false));
        assert!(!local_exit_effective(false, false));
        assert_eq!(
            exit_forwarding_action(local_exit_effective(false, true), false, false),
            ExitForwardingAction::Keep
        );
    }

    #[test]
    fn repeated_local_exit_disable_does_not_restart_draining() {
        assert!(repeated_local_exit_disable_is_noop(
            false, false, true, true
        ));
        assert!(repeated_local_exit_disable_is_noop(
            false, false, false, false
        ));
        assert!(!repeated_local_exit_disable_is_noop(
            false, false, true, false
        ));
        assert!(!repeated_local_exit_disable_is_noop(
            false, true, true, true
        ));
        assert!(!repeated_local_exit_disable_is_noop(
            true, false, true, true
        ));
    }

    #[test]
    fn spoofed_tun_source_is_replaced_with_the_assigned_virtual_address() {
        let assigned =
            VirtualIps::dual_stack("10.8.0.2".parse().unwrap(), "fd00::1002".parse().unwrap());

        let mut ipv4 = [0u8; 20];
        ipv4[0] = 0x45;
        ipv4[12..16].copy_from_slice(&[192, 168, 1, 77]);
        ipv4[16..20].copy_from_slice(&[1, 1, 1, 1]);
        assert_eq!(
            get_packet_src_ip(&ipv4, &assigned),
            IpAddr::V4("10.8.0.2".parse().unwrap())
        );

        let mut ipv6 = [0u8; 40];
        ipv6[0] = 0x60;
        ipv6[8..24].copy_from_slice(&"2001:db8::77".parse::<Ipv6Addr>().unwrap().octets());
        ipv6[24..40].copy_from_slice(&"2606:4700:4700::1111".parse::<Ipv6Addr>().unwrap().octets());
        assert_eq!(
            get_packet_src_ip(&ipv6, &assigned),
            IpAddr::V6("fd00::1002".parse().unwrap())
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
    fn assigned_inbound_destination_is_mesh_even_outside_default_prefix() {
        let route_engine = SmartRouteEngine::new(fuckipv6_common::RoutingMode::GlobalProxy);
        let assigned_ips =
            VirtualIps::dual_stack("10.77.7.2".parse().unwrap(), "fd77:7::2".parse().unwrap());

        assert!(is_mesh_inbound_destination(
            &route_engine,
            &assigned_ips,
            "10.77.7.2".parse().unwrap()
        ));
        assert!(!is_mesh_inbound_destination(
            &route_engine,
            &assigned_ips,
            "1.1.1.1".parse().unwrap()
        ));
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

    #[test]
    fn configured_exit_matching_uses_only_the_removed_peer_identity() {
        let peer = PeerInfo::new_with_ips(
            "exit-peer".to_string(),
            VirtualIps::dual_stack("10.8.0.1".parse().unwrap(), "fd00::1001".parse().unwrap()),
            vec![0; 32],
            Vec::new(),
            NatType::Open,
        );

        for target in [
            Some("exit-peer"),
            Some("fd00::1001"),
            Some("10.8.0.1"),
            Some("  exit-peer  "),
        ] {
            assert!(
                peer_is_configured_exit(&peer, target),
                "configured Exit target {target:?} must match the removed peer"
            );
        }
        for target in [
            None,
            Some(""),
            Some("   "),
            Some("another-peer"),
            Some("fd00::1002"),
            Some("10.8.0.2"),
        ] {
            assert!(
                !peer_is_configured_exit(&peer, target),
                "configured Exit target {target:?} must not match the removed peer"
            );
        }
    }

    #[test]
    fn exit_packet_admission_requires_a_committed_protected_session() {
        assert!(exit_packet_admission_allowed(
            false,
            false,
            TrafficProtectionState::Protected
        ));
        assert!(exit_packet_admission_allowed(
            false,
            false,
            TrafficProtectionState::ExplicitDirectFallback
        ));

        for state in [
            TrafficProtectionState::Idle,
            TrafficProtectionState::Connecting,
            TrafficProtectionState::Migrating,
            TrafficProtectionState::FailClosed,
        ] {
            assert!(
                !exit_packet_admission_allowed(false, false, state),
                "{state:?} must not carry traffic"
            );
        }
        assert!(!exit_packet_admission_allowed(
            true,
            false,
            TrafficProtectionState::Protected
        ));
        assert!(!exit_packet_admission_allowed(
            false,
            true,
            TrafficProtectionState::Protected
        ));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn recovery_trigger_invalidates_only_the_disconnected_path() {
        let mut exit_path = PathRecoveryState::Connected;
        let mut relay_path = PathRecoveryState::Connected;

        invalidate_linux_recovery_path(
            RecoveryTrigger::ExitDisconnected,
            &mut exit_path,
            &mut relay_path,
        );
        assert_eq!(exit_path, PathRecoveryState::Disconnected);
        assert_eq!(relay_path, PathRecoveryState::Connected);

        exit_path = PathRecoveryState::Connected;
        invalidate_linux_recovery_path(
            RecoveryTrigger::RelayDisconnected,
            &mut exit_path,
            &mut relay_path,
        );
        assert_eq!(exit_path, PathRecoveryState::Connected);
        assert_eq!(relay_path, PathRecoveryState::Disconnected);

        invalidate_linux_recovery_path(
            RecoveryTrigger::NetworkChanged,
            &mut exit_path,
            &mut relay_path,
        );
        assert_eq!(exit_path, PathRecoveryState::Connected);
        assert_eq!(relay_path, PathRecoveryState::Disconnected);

        exit_path = PathRecoveryState::Connected;
        relay_path = PathRecoveryState::Connected;
        invalidate_linux_recovery_path(
            RecoveryTrigger::ExitConnectionTimeout,
            &mut exit_path,
            &mut relay_path,
        );
        assert_eq!(exit_path, PathRecoveryState::Disconnected);
        assert_eq!(relay_path, PathRecoveryState::Connected);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn exit_connection_timeout_has_stable_recovery_reason_codes() {
        assert_eq!(
            recovery_started_reason_code(RecoveryTrigger::ExitConnectionTimeout),
            "recovery.connection_timeout.started"
        );
        assert_eq!(
            recovery_failure_reason_code(RecoveryTrigger::ExitConnectionTimeout),
            "recovery.connection_timeout.failed_closed"
        );
    }

    #[test]
    fn identical_fail_closed_reason_is_already_applied() {
        let mut session = TrafficSessionSnapshot::default();
        session
            .enter_fail_closed(
                Some(TrafficFailureKind::NetworkSelectionUnavailable),
                1,
                "exit.selection.network_context_missing",
            )
            .expect("record fail-closed session");

        assert!(traffic_session_already_failed_closed(
            &session,
            "exit.selection.network_context_missing"
        ));
        assert!(!traffic_session_already_failed_closed(
            &session,
            "exit.selection.authorization_revoked"
        ));

        session.state = TrafficProtectionState::Idle;
        assert!(!traffic_session_already_failed_closed(
            &session,
            "exit.selection.network_context_missing"
        ));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn exit_connection_watchdog_validates_once_and_times_out_once() {
        let exit = NodeId::from_public_key(b"watchdog-exit");
        let mut established =
            ExitConnectionWatchdog::new(exit.clone(), 1_000, 10, RecoveryTrigger::Manual);
        assert_eq!(
            established.observe(1_001, false),
            ExitConnectionWatchdogAction::Wait
        );
        assert_eq!(
            established.observe(1_500, true),
            ExitConnectionWatchdogAction::Validate(RecoveryTrigger::Manual)
        );
        assert_eq!(
            established.observe(20_000, true),
            ExitConnectionWatchdogAction::Wait,
            "an Exit must never enqueue duplicate validation work"
        );

        let mut timed_out =
            ExitConnectionWatchdog::new(exit, 1_000, 10, RecoveryTrigger::ExitMigration);
        assert_eq!(
            timed_out.observe(10_999, false),
            ExitConnectionWatchdogAction::Wait
        );
        assert_eq!(
            timed_out.observe(11_000, true),
            ExitConnectionWatchdogAction::TimedOut,
            "the deadline takes precedence over a session that completes in the same poll"
        );
        assert_eq!(
            timed_out.observe(30_000, true),
            ExitConnectionWatchdogAction::Wait,
            "a timed-out Exit must not later be promoted by the same watchdog"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn recovery_exit_path_requires_an_established_secure_session() {
        let exit_ip = Some("fd00::1001".parse().unwrap());
        assert_eq!(
            exit_path_from_session_evidence(exit_ip, false),
            PathRecoveryState::Disconnected
        );
        assert_eq!(
            exit_path_from_session_evidence(exit_ip, true),
            PathRecoveryState::Connected
        );
        assert_eq!(
            exit_path_from_session_evidence(None, true),
            PathRecoveryState::Disconnected
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn authorized_exit_recovery_context_is_fixed_and_fail_closed() {
        let exit_id = NodeId::from_public_key(b"recovery-exit");

        let context = linux_recovery_route_context(
            fuckipv6_common::RoutingMode::GlobalProxy,
            Some(&exit_id),
            Some(&exit_id),
        );
        assert_eq!(
            context.requested_action,
            TrafficAction::Exit(exit_id.clone())
        );
        assert_eq!(context.fixed_exit, Some(exit_id));
        assert!(context.validate().is_ok());
        assert_eq!(
            linux_recovery_request(
                fuckipv6_common::RoutingMode::GlobalProxy,
                true,
                RecoveryTrigger::ExitDisconnected,
                context,
            )
            .failure_policy,
            TrafficFailurePolicy::fail_closed()
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn missing_exit_selection_never_opens_a_recovery_bypass() {
        let rejected =
            linux_recovery_route_context(fuckipv6_common::RoutingMode::GlobalProxy, None, None);
        assert_eq!(rejected.requested_action, TrafficAction::Reject);
        assert_eq!(rejected.fixed_exit, None);
        assert!(rejected.validate().is_ok());

        let mesh_only =
            linux_recovery_route_context(fuckipv6_common::RoutingMode::MeshOnly, None, None);
        assert_eq!(mesh_only.requested_action, TrafficAction::Direct);
        assert!(mesh_only.validate().is_ok());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn recovery_audit_is_rooted_beside_the_network_config() {
        assert_eq!(
            linux_recovery_audit_root("/run/user/1000/fuckipv6/office/config.toml"),
            PathBuf::from("/run/user/1000/fuckipv6/office")
        );
        assert_eq!(linux_recovery_audit_root("config.toml"), PathBuf::from("."));
    }

    #[tokio::test]
    async fn exit_recovery_is_requested_only_for_the_removed_configured_exit() {
        let (mut events, sender) = RuntimeRecoveryEvents::without_os_watcher_for_test();
        let peer = PeerInfo::new_with_ips(
            "exit-peer".to_string(),
            VirtualIps::dual_stack("10.8.0.1".parse().unwrap(), "fd00::1001".parse().unwrap()),
            vec![0; 32],
            Vec::new(),
            NatType::Open,
        );

        request_exit_recovery_on_offline(&sender, None, Some("exit-peer"));
        request_exit_recovery_on_offline(&sender, Some(&peer), None);
        request_exit_recovery_on_offline(&sender, Some(&peer), Some("another-peer"));
        assert!(
            tokio::time::timeout(Duration::from_millis(20), events.next_trigger())
                .await
                .is_err(),
            "missing, stale, or unrelated peer removals must not request recovery"
        );

        request_exit_recovery_on_offline(&sender, Some(&peer), Some("fd00::1001"));
        assert_eq!(
            events.next_trigger().await,
            Some(RecoveryTrigger::ExitDisconnected)
        );
    }

    #[tokio::test]
    async fn runtime_recovery_events_deliver_programmatic_trigger() {
        let (mut events, sender) = RuntimeRecoveryEvents::without_os_watcher_for_test();

        assert!(sender.try_trigger(RecoveryTrigger::RelayDisconnected));
        assert_eq!(
            events.next_trigger().await,
            Some(RecoveryTrigger::RelayDisconnected)
        );
    }

    #[tokio::test]
    async fn stale_relay_generation_does_not_request_recovery() {
        let (mut events, sender) = RuntimeRecoveryEvents::without_os_watcher_for_test();
        let relay_state = RelayState::with_recovery_trigger_sender(sender);

        assert!(
            relay_state.clear_relay_with_recovery(17).await.is_none(),
            "an absent or stale generation must not be cleared"
        );
        assert!(
            tokio::time::timeout(Duration::from_millis(20), events.next_trigger())
                .await
                .is_err(),
            "failing to clear the current generation must not enqueue recovery"
        );
    }

    #[tokio::test]
    async fn runtime_recovery_events_use_a_bounded_queue() {
        let (_events, sender) = RuntimeRecoveryEvents::without_os_watcher_for_test();

        for _ in 0..RECOVERY_TRIGGER_QUEUE_CAPACITY {
            assert!(sender.try_trigger(RecoveryTrigger::NetworkChanged));
        }
        assert!(
            !sender.try_trigger(RecoveryTrigger::NetworkChanged),
            "a blocked recovery consumer must not permit unbounded queue growth"
        );
    }

    #[test]
    fn exit_contribution_context_requires_matching_active_membership_and_terms() {
        use crate::node::NodeNetworkMembershipSnapshot;
        use fuckipv6_common::domain::{
            ExitContributionObligation, ExitContributionScope, MembershipRole, MembershipState,
            MembershipTerms,
        };

        let network_id = NetworkId::from_public_key(b"exit-context-network");
        let node_id = NodeId::from_public_key(b"exit-context-node");
        let terms = MembershipTerms {
            network_policy_revision: 7,
            privacy_policy_revision: 3,
            exit_contribution: ExitContributionObligation::Required,
            exit_contribution_scope: ExitContributionScope::AllMembers,
            exit_contribution_mode: fuckipv6_common::domain::ExitContributionMode::RequiredOnJoin,
            member_limit: Some(8),
        };
        let expected_hash = terms.content_hash().expect("valid terms");
        let membership = |network: &NetworkId, node: &NodeId, state, current_terms| {
            NodeNetworkMembershipSnapshot {
                network_name: "home".into(),
                network_id: network.to_string(),
                node_id: node.to_string(),
                state,
                role: MembershipRole::Member,
                local_policy_revision: 1,
                network_policy_revision: Some(7),
                privacy_policy_revision: Some(3),
                member_limit: Some(8),
                exit_contribution: Some(ExitContributionObligation::Required),
                exit_contribution_scope: Some(ExitContributionScope::AllMembers),
                exit_contribution_mode: Some(
                    fuckipv6_common::domain::ExitContributionMode::RequiredOnJoin,
                ),
                terms_hash: Some(expected_hash.as_str().to_owned()),
                current_terms,
                current_terms_hash: Some(expected_hash.as_str().to_owned()),
                policy_confirmation_required: true,
                acknowledged_at_unix_ms: Some(1),
                created_at_unix_ms: 1,
                updated_at_unix_ms: 2,
                suspension_reason: None,
            }
        };

        let snapshot = NodeSnapshot {
            memberships: vec![membership(
                &network_id,
                &node_id,
                MembershipState::Active,
                Some(terms.clone()),
            )],
            ..NodeSnapshot::default()
        };
        let context = exit_contribution_context_from_snapshot(&snapshot, &network_id, &node_id)
            .expect("active matching membership with valid terms");
        assert_eq!(
            context.mode,
            fuckipv6_common::domain::ExitContributionMode::RequiredOnJoin
        );
        assert_eq!(context.policy_revision, 7);
        assert_eq!(context.terms_hash, expected_hash);
        assert!(context.policy_confirmation_required);

        let inactive = NodeSnapshot {
            memberships: vec![membership(
                &network_id,
                &node_id,
                MembershipState::Suspended,
                Some(terms.clone()),
            )],
            ..NodeSnapshot::default()
        };
        assert!(
            exit_contribution_context_from_snapshot(&inactive, &network_id, &node_id).is_none()
        );
        for state in [
            MembershipState::Discovered,
            MembershipState::Applying,
            MembershipState::PendingApproval,
            MembershipState::Accepted,
            MembershipState::KeyReady,
            MembershipState::Connecting,
            MembershipState::Active,
            MembershipState::Degraded,
        ] {
            let snapshot = NodeSnapshot {
                memberships: vec![membership(
                    &network_id,
                    &node_id,
                    state,
                    Some(terms.clone()),
                )],
                ..NodeSnapshot::default()
            };
            assert!(
                !local_membership_relationship_is_revoked(&snapshot, &network_id, &node_id),
                "{state:?} is a normal lifecycle state, not an authorization revocation"
            );
        }
        for state in [
            MembershipState::Leaving,
            MembershipState::Left,
            MembershipState::Rejected,
            MembershipState::Suspended,
            MembershipState::Cancelled,
        ] {
            let snapshot = NodeSnapshot {
                memberships: vec![membership(
                    &network_id,
                    &node_id,
                    state,
                    Some(terms.clone()),
                )],
                ..NodeSnapshot::default()
            };
            assert!(
                local_membership_relationship_is_revoked(&snapshot, &network_id, &node_id),
                "{state:?} must revoke local network authority"
            );
        }

        let without_terms = NodeSnapshot {
            memberships: vec![membership(
                &network_id,
                &node_id,
                MembershipState::Active,
                None,
            )],
            ..NodeSnapshot::default()
        };
        assert!(
            exit_contribution_context_from_snapshot(&without_terms, &network_id, &node_id)
                .is_none()
        );

        let other_network = NetworkId::from_public_key(b"other-network");
        assert!(
            exit_contribution_context_from_snapshot(&snapshot, &other_network, &node_id).is_none()
        );
        assert!(!local_membership_relationship_is_revoked(
            &snapshot,
            &other_network,
            &node_id
        ));
        assert!(!local_membership_relationship_is_revoked(
            &NodeSnapshot::default(),
            &network_id,
            &node_id
        ));
    }

    #[test]
    fn exit_directory_loss_only_fails_closed_for_active_proxy_sessions() {
        use fuckipv6_common::RoutingMode;

        assert!(!exit_selection_directory_loss_requires_fail_closed(
            RoutingMode::MeshOnly,
            true
        ));
        assert!(!exit_selection_directory_loss_requires_fail_closed(
            RoutingMode::GlobalProxy,
            false
        ));
        assert!(exit_selection_directory_loss_requires_fail_closed(
            RoutingMode::GlobalProxy,
            true
        ));
        assert!(exit_selection_directory_loss_requires_fail_closed(
            RoutingMode::BypassMainland,
            true
        ));
    }

    #[test]
    fn probe_network_prefers_the_matching_peer_network() {
        use crate::node::{NodeNetworkRole, NodePeerSnapshot, NodeTrafficSnapshot};

        let network = |name: &str, status| NodeNetworkSnapshot {
            name: name.to_string(),
            tun_name: format!("fuckipv6-{name}"),
            status,
            assigned_ipv6: None,
            assigned_ipv4: None,
            p2p_listen_addr: None,
            uptime_secs: 0,
            roles: vec![NodeNetworkRole::Member],
            peer_count: 0,
            traffic: NodeTrafficSnapshot::default(),
        };
        let snapshot = NodeSnapshot {
            networks: vec![
                network("office", NodeNetworkStatus::Online),
                network("lab", NodeNetworkStatus::Degraded),
            ],
            peers: vec![NodePeerSnapshot {
                peer_id: "peer-lab".to_string(),
                network_name: "lab".to_string(),
                virtual_ipv6: "fd00::2".parse().unwrap(),
                virtual_ipv4: Some("10.8.0.2".parse().unwrap()),
                endpoints: Vec::new(),
                online: true,
                p2p_active: true,
                p2p_healthy: true,
                rtt_us: Some(1_000),
                loss_rate: 0.0,
                traffic: NodeTrafficSnapshot::default(),
            }],
            ..NodeSnapshot::default()
        };

        assert_eq!(
            select_probe_network(&snapshot, "peer-lab")
                .expect("matching peer network")
                .name,
            "lab"
        );
        assert_eq!(
            select_probe_network(&snapshot, "203.0.113.1")
                .expect("fallback network")
                .name,
            "office"
        );
        assert!(select_probe_network(&NodeSnapshot::default(), "203.0.113.1").is_err());
    }

    #[test]
    fn local_exit_availability_requires_the_enabled_healthy_data_plane() {
        let enabled = LocalExitCapabilitySnapshot {
            owner_exit_enabled: true,
            stage: LocalExitCapabilityStage::Enabled,
            healthy: true,
            network_allowed: true,
            ..LocalExitCapabilitySnapshot::default()
        };
        assert_eq!(local_exit_availability_report(&enabled), (true, None));

        let owner_disabled = LocalExitCapabilitySnapshot {
            owner_exit_enabled: false,
            ..enabled.clone()
        };
        assert_eq!(
            local_exit_availability_report(&owner_disabled),
            (false, Some("exit.availability.owner_disabled"))
        );

        let network_disallowed = LocalExitCapabilitySnapshot {
            network_allowed: false,
            ..enabled.clone()
        };
        assert_eq!(
            local_exit_availability_report(&network_disallowed),
            (false, Some("exit.availability.network_not_allowed"))
        );

        let unhealthy = LocalExitCapabilitySnapshot {
            healthy: false,
            ..enabled.clone()
        };
        assert_eq!(
            local_exit_availability_report(&unhealthy),
            (false, Some("exit.availability.forwarder_unhealthy"))
        );

        let health_checking = LocalExitCapabilitySnapshot {
            stage: LocalExitCapabilityStage::HealthChecking,
            healthy: false,
            ..enabled.clone()
        };
        assert_eq!(
            local_exit_availability_report(&health_checking),
            (false, Some("exit.availability.health_checking"))
        );

        let draining = LocalExitCapabilitySnapshot {
            stage: LocalExitCapabilityStage::Draining,
            healthy: false,
            ..enabled
        };
        assert_eq!(
            local_exit_availability_report(&draining),
            (false, Some("exit.availability.draining"))
        );
    }
}
