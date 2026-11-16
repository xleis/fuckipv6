use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use fuckipv6_common::domain::{
    AdminRecoveryStatusView, CoordinatorCandidateConfig, CoordinatorCandidateDeclaration,
    CoordinatorCandidateStatus, CoordinatorEmergencyLeaderStatus, CoordinatorRecoveryReport,
    CoordinatorRecoveryStatus, CoordinatorTerm, CoordinatorWriteCredential, ExitHealthEvidence,
    ExitHealthEvidenceState, ExitHealthLayer, ExitHealthProtocol, ExitHealthRetest,
    ExitHealthRetestPhase, ExitHealthSnapshot, ExitHealthSource, ExitHealthTarget, InvitationCode,
    InvitationCreateRequest, InvitationId, InvitationPreview, InvitationRedeemRequest,
    InvitationRedemption, InvitationStatusView, IssuedInvitation, JoinRequest, JoinRequestId,
    LeakDetectionEvidence, LeakDetectionEvidenceState, LeakDetectionIsolationAction,
    LeakDetectionProtocol, LeakDetectionSnapshot, LeakDetectionSource, LeakDetectionTarget,
    MemberLifecycleChange, MemberLifecycleChangeRequest, MemberRemovalPreview,
    MemberRemovalPreviewRequest, MembershipRole, NetworkId, NetworkMemberDirectory, NodeId,
    RelayAuthorizationAuditSnapshot, RelayGovernanceAuditSnapshot, RelayHealthSnapshot,
    TrafficProtectionState,
};
use fuckipv6_common::{
    ClientConfig, ExitNodeCapabilities, ExitNodeUnsupportedReason, JoinRequestBatchReviewItem,
    JoinRequestReviewResult, NatType, NetworkTicket,
};
use futures::future::join_all;
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::JoinHandle;
use tokio::time::{sleep, sleep_until, timeout, Instant};

use super::command::{
    NodeAdminRecoveryListCommand, NodeAdminRecoveryStatusCommand, NodeExecuteAdminRecoveryCommand,
    NodePublishCoordinatorCandidateDeclarationCommand, NodeRevokeAdminRecoveryCommand,
    NodeSubmitAdminRecoveryCommand, NodeUpdateCoordinatorCandidatesCommand,
};
use super::coordinator_takeover::{
    active_local_emergency, decide_coordinator_takeover, CoordinatorTakeoverDecision,
    CoordinatorTakeoverPollBackoff,
};
use super::state::effective_membership_state;
use super::{
    LocalExitCapabilitySnapshot, LocalExitCapabilityStage, NodeCleanupFailureReason,
    NodeCleanupFailureSnapshot, NodeCleanupStepSnapshot, NodeCleanupStepStatus,
    NodeCreateNetworkCommand, NodeDiagnosticCheck, NodeDiagnosticLayer,
    NodeDiagnosticRecoveryAction, NodeDiagnosticState, NodeDiagnosticsSnapshot,
    NodeEmbeddedServiceSnapshot, NodeEmbeddedServiceStatus, NodeEventBus, NodeEventKind,
    NodeIdentitySnapshot, NodeInterfaceKind, NodeInterfaceOwnership, NodeInterfaceStatus,
    NodeJoinNetworkCommand, NodeLifecycle, NodeNetworkConfigSnapshot, NodeNetworkCreationNextStep,
    NodeNetworkMembershipSnapshot, NodeNetworkRole, NodeNetworkSelection, NodeNetworkSnapshot,
    NodeNetworkStatus, NodePhase, NodeProblem, NodeProblemCode, NodeResidualCleanupResult,
    NodeRoutingDefaultAction, NodeRoutingMode, NodeRoutingSummarySnapshot,
    NodeRoutingSummaryStatus, NodeRoutingTargetSnapshot, NodeRuntimePlan,
    NodeRuntimeResidueSnapshot, NodeSetNetworkRoleCommand, NodeSnapshot, NodeStateStore,
    NodeTrafficSnapshot, NodeUpdateRoutingCommand,
};
use crate::cli::commands::{self, CleanupStatus, RuntimeCleanupOutcome, RuntimeOptions};
use crate::config_manager::{
    ConfigManager, JoinedNetworkMembership, NetworkConfig, NetworkCreationError,
};
use crate::ipc::{self, MultiNetworkRegistry, NetworkSessionHandle};
use crate::network_setup::{self, NetworkSetupStage};
use crate::node::runtime_metadata;
use crate::owner_security::audit::OwnerSecurityAudit;
use crate::routing::leak_detection::{collect_leak_detection, LeakDetectionContext};
use crate::signaling::client::{
    AdminRecoveryControlError, CoordinatorCandidateControlError, InvitationControlError,
    JoinRequestControlError, JoinRequestStatusSnapshot, SignalingClient,
};
use crate::symmetric::{
    EmbeddedServiceError, EmbeddedServiceManager, EmbeddedServiceSnapshot,
    EmbeddedServiceStartRequest,
};

pub(crate) const STOP_TIMEOUT: Duration = Duration::from_secs(15);
pub(crate) const START_TIMEOUT: Duration = Duration::from_secs(30);
pub(crate) const RESTART_TIMEOUT: Duration = Duration::from_secs(60);
const ROUTING_UPDATE_TIMEOUT: Duration = Duration::from_secs(20);
const LOCAL_EXIT_UPDATE_TIMEOUT: Duration = Duration::from_secs(20);
const LOCAL_EXIT_DRAIN_OBSERVATION_TIMEOUT: Duration = Duration::from_secs(12);
const LOCAL_EXIT_DRAIN_POLL_INTERVAL: Duration = Duration::from_millis(100);
const LOCAL_EXIT_SNAPSHOT_REFRESH_INTERVAL: Duration = Duration::from_secs(1);
const LOCAL_EXIT_STATUS_REFRESH_TIMEOUT: Duration = Duration::from_secs(1);
const DIAGNOSTICS_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RuntimeTimeouts {
    stop: Duration,
    start: Duration,
    restart: Duration,
    routing_update: Duration,
    local_exit_update: Duration,
    local_exit_refresh: Duration,
    diagnostics: Duration,
}

impl Default for RuntimeTimeouts {
    fn default() -> Self {
        Self {
            stop: STOP_TIMEOUT,
            start: START_TIMEOUT,
            restart: RESTART_TIMEOUT,
            routing_update: ROUTING_UPDATE_TIMEOUT,
            local_exit_update: LOCAL_EXIT_UPDATE_TIMEOUT,
            local_exit_refresh: LOCAL_EXIT_SNAPSHOT_REFRESH_INTERVAL,
            diagnostics: DIAGNOSTICS_TIMEOUT,
        }
    }
}

type RuntimeFuture<T> = Pin<Box<dyn Future<Output = Result<T, RuntimeControlError>> + Send>>;

#[derive(Debug, Clone, PartialEq, Eq)]
enum RuntimeRoutingRequestError {
    ControlClosed,
    TimedOut(Duration),
    ResponseClosed,
    Rejected(String),
    LocalExitRejected(RuntimeControlError),
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub(crate) enum RuntimeControlError {
    #[error("节点运行时管理器已关闭")]
    ActorClosed,
    #[error("{0}")]
    Invalid(String),
    #[error("{0}")]
    Conflict(String),
    #[error("{message}")]
    CreationConflict {
        reason: &'static str,
        message: String,
    },
    #[error("{0}")]
    PermissionDenied(String),
    #[error("{0}")]
    Cancelled(String),
    #[error("{0}")]
    Unsupported(String),
    #[error("{0}")]
    NotFound(String),
    #[error("{0}")]
    Unavailable(String),
    #[error("[{reason_code}] {message}")]
    RelayLifecycleFailed {
        reason_code: &'static str,
        message: String,
    },
    #[error("{0}")]
    ResourceExhausted(String),
    #[error("{0}")]
    Persistence(String),
    #[error("{0}")]
    Storage(String),
    #[error("{0}")]
    Launch(String),
    #[error("{message}")]
    NetworkSetup {
        stage: NetworkSetupStage,
        message: String,
    },
    #[error("{0}")]
    Timeout(String),
    #[error("{message}")]
    JoinRequest {
        code: NodeProblemCode,
        message_key: &'static str,
        message: String,
    },
    #[error("{message}")]
    Invitation {
        code: NodeProblemCode,
        message_key: &'static str,
        message: String,
    },
    #[error("{message}")]
    CoordinatorCandidate {
        code: NodeProblemCode,
        message_key: &'static str,
        message: String,
    },
    #[error("{message}")]
    AdminRecovery {
        code: NodeProblemCode,
        message_key: &'static str,
        message: String,
    },
}

impl RuntimeControlError {
    pub(crate) fn problem_code(&self) -> NodeProblemCode {
        match self {
            Self::Invalid(_) => NodeProblemCode::InvalidRequest,
            Self::Conflict(_) | Self::CreationConflict { .. } => NodeProblemCode::Conflict,
            Self::PermissionDenied(_) => NodeProblemCode::PermissionDenied,
            Self::Cancelled(_) => NodeProblemCode::Cancelled,
            Self::Unsupported(_) => NodeProblemCode::Unsupported,
            Self::NotFound(_) => NodeProblemCode::NotFound,
            Self::NetworkSetup { stage, .. } => network_setup_problem_code(*stage),
            Self::RelayLifecycleFailed { reason_code, .. } => match *reason_code {
                "relay.start.platform_unsupported" => NodeProblemCode::Unsupported,
                "relay.start.permission_denied" => NodeProblemCode::PermissionDenied,
                _ => NodeProblemCode::Unavailable,
            },
            Self::ActorClosed
            | Self::Unavailable(_)
            | Self::Persistence(_)
            | Self::Storage(_)
            | Self::Launch(_) => NodeProblemCode::Unavailable,
            Self::ResourceExhausted(_) => NodeProblemCode::ResourceExhausted,
            Self::Timeout(_) => NodeProblemCode::TimedOut,
            Self::JoinRequest { code, .. } => *code,
            Self::Invitation { code, .. } => *code,
            Self::CoordinatorCandidate { code, .. } => *code,
            Self::AdminRecovery { code, .. } => *code,
        }
    }

    pub(crate) fn message_key(&self) -> &'static str {
        match self {
            Self::NetworkSetup { stage, .. } => stage.message_key(),
            Self::CreationConflict { .. } => "error.conflict",
            Self::PermissionDenied(_) => "error.permission_denied",
            Self::JoinRequest { message_key, .. } => message_key,
            Self::Invitation { message_key, .. } => message_key,
            Self::CoordinatorCandidate { message_key, .. } => message_key,
            Self::AdminRecovery { message_key, .. } => message_key,
            _ => self.problem_code().message_key(),
        }
    }

    pub(crate) fn retryable(&self) -> bool {
        if let Self::RelayLifecycleFailed { reason_code, .. } = self {
            return !matches!(
                *reason_code,
                "relay.start.platform_unsupported" | "relay.start.permission_denied"
            );
        }
        !matches!(
            self,
            Self::Invalid(_)
                | Self::Conflict(_)
                | Self::CreationConflict { .. }
                | Self::PermissionDenied(_)
                | Self::Cancelled(_)
                | Self::Unsupported(_)
                | Self::NotFound(_)
                | Self::NetworkSetup {
                    stage: NetworkSetupStage::PermissionDenied
                        | NetworkSetupStage::PlatformUnsupported,
                    ..
                }
                | Self::JoinRequest {
                    code: NodeProblemCode::AuthenticationRequired
                        | NodeProblemCode::InvalidRequest
                        | NodeProblemCode::NotFound
                        | NodeProblemCode::Conflict
                        | NodeProblemCode::PermissionDenied
                        | NodeProblemCode::Cancelled
                        | NodeProblemCode::Unsupported
                        | NodeProblemCode::Internal,
                    ..
                }
                | Self::Invitation {
                    code: NodeProblemCode::AuthenticationRequired
                        | NodeProblemCode::InvalidRequest
                        | NodeProblemCode::NotFound
                        | NodeProblemCode::Conflict
                        | NodeProblemCode::PermissionDenied
                        | NodeProblemCode::Cancelled
                        | NodeProblemCode::Unsupported
                        | NodeProblemCode::Internal,
                    ..
                }
                | Self::CoordinatorCandidate {
                    code: NodeProblemCode::AuthenticationRequired
                        | NodeProblemCode::InvalidRequest
                        | NodeProblemCode::NotFound
                        | NodeProblemCode::Conflict
                        | NodeProblemCode::PermissionDenied
                        | NodeProblemCode::Cancelled
                        | NodeProblemCode::Unsupported
                        | NodeProblemCode::Internal,
                    ..
                }
                | Self::AdminRecovery {
                    code: NodeProblemCode::AuthenticationRequired
                        | NodeProblemCode::InvalidRequest
                        | NodeProblemCode::NotFound
                        | NodeProblemCode::Conflict
                        | NodeProblemCode::PermissionDenied
                        | NodeProblemCode::Cancelled
                        | NodeProblemCode::Unsupported
                        | NodeProblemCode::Internal,
                    ..
                }
        )
    }

    pub(crate) fn network_setup_stage(&self) -> Option<NetworkSetupStage> {
        match self {
            Self::NetworkSetup { stage, .. } => Some(*stage),
            _ => None,
        }
    }

    pub(crate) fn conflict_reason(&self) -> Option<&'static str> {
        match self {
            Self::CreationConflict { reason, .. } => Some(*reason),
            Self::RelayLifecycleFailed { reason_code, .. } => Some(*reason_code),
            _ => None,
        }
    }

    pub(crate) fn is_storage_error(&self) -> bool {
        matches!(self, Self::Storage(_))
    }
}

pub(crate) fn network_setup_problem_code(stage: NetworkSetupStage) -> NodeProblemCode {
    match stage {
        NetworkSetupStage::PermissionDenied => NodeProblemCode::PermissionDenied,
        NetworkSetupStage::PlatformUnsupported => NodeProblemCode::Unsupported,
        _ => NodeProblemCode::Unavailable,
    }
}

fn map_join_request_error(error: JoinRequestControlError) -> RuntimeControlError {
    let code = match error {
        JoinRequestControlError::Disconnected
        | JoinRequestControlError::Unavailable
        | JoinRequestControlError::Rejected(429)
        | JoinRequestControlError::Rejected(503) => NodeProblemCode::Unavailable,
        JoinRequestControlError::TimedOut => NodeProblemCode::TimedOut,
        JoinRequestControlError::Rejected(401) => NodeProblemCode::AuthenticationRequired,
        JoinRequestControlError::Rejected(403) => NodeProblemCode::PermissionDenied,
        JoinRequestControlError::Rejected(404) => NodeProblemCode::NotFound,
        JoinRequestControlError::Rejected(_) | JoinRequestControlError::InvalidResponse => {
            NodeProblemCode::InvalidRequest
        }
    };
    RuntimeControlError::JoinRequest {
        code,
        message_key: error.message_key(),
        message: error.to_string(),
    }
}

fn join_request_unavailable(message: impl Into<String>) -> RuntimeControlError {
    RuntimeControlError::JoinRequest {
        code: NodeProblemCode::Unavailable,
        message_key: "join_request.error.unavailable",
        message: message.into(),
    }
}

fn map_invitation_error(error: InvitationControlError) -> RuntimeControlError {
    let message = error.to_string();
    let code = match error {
        InvitationControlError::Disconnected
        | InvitationControlError::Unavailable
        | InvitationControlError::Rejected(429 | 503) => NodeProblemCode::Unavailable,
        InvitationControlError::TimedOut => NodeProblemCode::TimedOut,
        InvitationControlError::Rejected(401) => NodeProblemCode::AuthenticationRequired,
        InvitationControlError::Rejected(403) => NodeProblemCode::PermissionDenied,
        InvitationControlError::Rejected(404) => NodeProblemCode::NotFound,
        InvitationControlError::Rejected(409 | 412) => NodeProblemCode::Conflict,
        InvitationControlError::Rejected(_) | InvitationControlError::InvalidResponse => {
            NodeProblemCode::InvalidRequest
        }
    };
    RuntimeControlError::Invitation {
        code,
        message_key: error.message_key(),
        message,
    }
}

fn invitation_unavailable(message: impl Into<String>) -> RuntimeControlError {
    RuntimeControlError::Invitation {
        code: NodeProblemCode::Unavailable,
        message_key: "invitation.error.unavailable",
        message: message.into(),
    }
}

fn map_coordinator_candidate_error(error: CoordinatorCandidateControlError) -> RuntimeControlError {
    let message = error.to_string();
    let code = match error {
        CoordinatorCandidateControlError::Disconnected
        | CoordinatorCandidateControlError::Unavailable
        | CoordinatorCandidateControlError::Rejected(429 | 503) => NodeProblemCode::Unavailable,
        CoordinatorCandidateControlError::TimedOut => NodeProblemCode::TimedOut,
        CoordinatorCandidateControlError::Rejected(401) => NodeProblemCode::AuthenticationRequired,
        CoordinatorCandidateControlError::Rejected(403) => NodeProblemCode::PermissionDenied,
        CoordinatorCandidateControlError::Rejected(404) => NodeProblemCode::NotFound,
        CoordinatorCandidateControlError::Rejected(409 | 412) => NodeProblemCode::Conflict,
        CoordinatorCandidateControlError::Rejected(_) => NodeProblemCode::InvalidRequest,
    };
    RuntimeControlError::CoordinatorCandidate {
        code,
        message_key: error.message_key(),
        message,
    }
}

fn coordinator_candidate_unavailable(message: impl Into<String>) -> RuntimeControlError {
    RuntimeControlError::CoordinatorCandidate {
        code: NodeProblemCode::Unavailable,
        message_key: "coordinator_candidate.error.unavailable",
        message: message.into(),
    }
}

fn coordinator_candidate_invalid(message: impl Into<String>) -> RuntimeControlError {
    RuntimeControlError::CoordinatorCandidate {
        code: NodeProblemCode::InvalidRequest,
        message_key: "coordinator_candidate.error.invalid",
        message: message.into(),
    }
}

fn map_admin_recovery_error(error: AdminRecoveryControlError) -> RuntimeControlError {
    let message = error.to_string();
    let message_key = error.message_key();
    let code = match error {
        AdminRecoveryControlError::Disconnected
        | AdminRecoveryControlError::Unavailable
        | AdminRecoveryControlError::Rejected {
            code: 429 | 503, ..
        } => NodeProblemCode::Unavailable,
        AdminRecoveryControlError::TimedOut => NodeProblemCode::TimedOut,
        AdminRecoveryControlError::Rejected { code: 401, .. } => {
            NodeProblemCode::AuthenticationRequired
        }
        AdminRecoveryControlError::Rejected { code: 403, .. } => NodeProblemCode::PermissionDenied,
        AdminRecoveryControlError::Rejected { code: 404, .. } => NodeProblemCode::NotFound,
        AdminRecoveryControlError::Rejected {
            code: 409 | 412, ..
        } => NodeProblemCode::Conflict,
        AdminRecoveryControlError::Rejected { .. } | AdminRecoveryControlError::InvalidResponse => {
            NodeProblemCode::InvalidRequest
        }
    };
    RuntimeControlError::AdminRecovery {
        code,
        message_key,
        message,
    }
}

fn admin_recovery_unavailable(message: impl Into<String>) -> RuntimeControlError {
    RuntimeControlError::AdminRecovery {
        code: NodeProblemCode::Unavailable,
        message_key: "admin_recovery.error.unavailable",
        message: message.into(),
    }
}

fn admin_recovery_invalid(message: impl Into<String>) -> RuntimeControlError {
    RuntimeControlError::AdminRecovery {
        code: NodeProblemCode::InvalidRequest,
        message_key: "admin_recovery.error.invalid_request",
        message: message.into(),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StartFailure {
    pub network_name: String,
    pub message: String,
    pub stage: Option<NetworkSetupStage>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct StartSummary {
    pub requested: usize,
    pub started: Vec<String>,
    pub already_running: Vec<String>,
    pub failed: Vec<StartFailure>,
}

impl StartSummary {
    pub fn message(&self, action: &str) -> String {
        let mut parts = vec![format!("{action}: 已启动 {} 个网络", self.started.len())];
        if !self.already_running.is_empty() {
            parts.push(format!(
                "{} 个网络已在运行 ({})",
                self.already_running.len(),
                self.already_running.join(", ")
            ));
        }
        if !self.failed.is_empty() {
            let failures = self
                .failed
                .iter()
                .map(|failure| format!("{}: {}", failure.network_name, failure.message))
                .collect::<Vec<_>>()
                .join("; ");
            parts.push(format!("{} 个网络启动失败 ({failures})", self.failed.len()));
        }
        parts.join("；")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StopFailure {
    pub network_name: String,
    pub message: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct StopSummary {
    pub stopped: Vec<String>,
    pub failed: Vec<StopFailure>,
    pub cleanup: RuntimeCleanupReport,
}

impl StopSummary {
    pub fn is_success(&self) -> bool {
        self.failed.is_empty() && self.cleanup.is_clean_for_stop()
    }

    pub fn message(&self) -> String {
        let mut parts = vec![format!("已停止网络: {}", self.stopped.len())];
        if !self.stopped.is_empty() {
            parts.push(self.stopped.join(", "));
        }
        if !self.failed.is_empty() {
            let failures = self
                .failed
                .iter()
                .map(|failure| format!("{}: {}", failure.network_name, failure.message))
                .collect::<Vec<_>>()
                .join("; ");
            parts.push(format!("{} 个网络停止失败 ({failures})", self.failed.len()));
        }
        let cleanup_issues = self.cleanup.stop_issues();
        if !cleanup_issues.is_empty() {
            parts.push(format!("资源清理未完成 ({})", cleanup_issues.join("; ")));
        }
        parts.join("；")
    }

    pub(crate) fn to_public_cleanup_result(&self) -> NodeResidualCleanupResult {
        NodeResidualCleanupResult {
            cleaned_networks: self.stopped.clone(),
            failed_networks: self
                .failed
                .iter()
                .map(|failure| NodeCleanupFailureSnapshot {
                    network_name: failure.network_name.clone(),
                    reason: cleanup_failure_reason(&failure.message),
                })
                .collect(),
            residue: self.cleanup.to_public(),
        }
    }
}

fn cleanup_failure_reason(message: &str) -> NodeCleanupFailureReason {
    if message.contains("运行会话尚未退出") {
        NodeCleanupFailureReason::StillRunning
    } else if message.contains("运行时资源所有者已丢失") {
        NodeCleanupFailureReason::OwnerUnavailable
    } else if message.contains("cleanup/timeout") {
        NodeCleanupFailureReason::TimedOut
    } else if message.contains("资源清理重试未完成") {
        NodeCleanupFailureReason::Incomplete
    } else {
        NodeCleanupFailureReason::Unknown
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RestartPhase {
    PersistLoad,
    Stop,
    Cleanup,
    Start,
}

impl RestartPhase {
    const fn as_str(self) -> &'static str {
        match self {
            Self::PersistLoad => "persist_load",
            Self::Stop => "stop",
            Self::Cleanup => "cleanup",
            Self::Start => "start",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RestartFailure {
    pub network_name: String,
    pub phase: RestartPhase,
    pub message: String,
    pub stage: Option<NetworkSetupStage>,
}

/// Registry/state/service checks performed after an explicit stop. These counts
/// do not claim that kernel TUN, route, policy-route or DNS state was audited.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct RuntimeCleanupReport {
    pub sessions_remaining: usize,
    pub listeners_remaining: usize,
    pub peers_remaining: usize,
    pub embedded_services_remaining: usize,
    pub configuration_retained: bool,
    /// Per-network runtime cleanup results returned by the owner of the TUN,
    /// routes, DNS, relay and P2P resources.
    pub runtime_outcomes: BTreeMap<String, RuntimeCleanupOutcome>,
    /// Networks with a session, listener or peer residue. Embedded services
    /// are intentionally retained across a network restart and never appear
    /// here.
    pub residual_networks: Vec<String>,
}

impl RuntimeCleanupReport {
    fn is_clean_for_stop(&self) -> bool {
        self.sessions_remaining == 0
            && self.listeners_remaining == 0
            && self.peers_remaining == 0
            && self.embedded_services_remaining == 0
            && self.residual_networks.is_empty()
            && self.resource_failures().is_empty()
    }

    fn stop_issues(&self) -> Vec<String> {
        let mut issues = Vec::new();
        if !self.residual_networks.is_empty() {
            issues.push(format!("残留网络: {}", self.residual_networks.join(", ")));
        }
        if self.sessions_remaining > 0 {
            issues.push(format!("运行时任务残留 {} 个", self.sessions_remaining));
        }
        if self.listeners_remaining > 0 {
            issues.push(format!("监听器残留 {} 个", self.listeners_remaining));
        }
        if self.peers_remaining > 0 {
            issues.push(format!("Peer 记录残留 {} 个", self.peers_remaining));
        }
        if self.embedded_services_remaining > 0 {
            issues.push(format!(
                "内嵌服务残留 {} 个",
                self.embedded_services_remaining
            ));
        }
        issues.extend(self.resource_failures());
        issues
    }

    fn blocks_restart(&self, network_name: &str) -> bool {
        self.residual_networks
            .iter()
            .any(|name| name == network_name)
            || self
                .runtime_outcomes
                .get(network_name)
                .is_some_and(|outcome| !outcome.is_clean())
    }

    fn resource_failures(&self) -> Vec<String> {
        self.runtime_outcomes
            .iter()
            .filter_map(|(network, outcome)| {
                let issues = outcome.issues();
                (!issues.is_empty()).then(|| format!("{network}: {}", issues.join("; ")))
            })
            .collect()
    }

    fn to_public(&self) -> NodeRuntimeResidueSnapshot {
        let cleanup_steps = self
            .runtime_outcomes
            .iter()
            .flat_map(|(network_name, outcome)| {
                if outcome.steps.is_empty() {
                    return vec![NodeCleanupStepSnapshot {
                        network_name: network_name.clone(),
                        resource: "运行时清理".into(),
                        status: NodeCleanupStepStatus::Unknown,
                        detail: Some("运行时未返回清理结果".into()),
                    }];
                }
                outcome
                    .steps
                    .iter()
                    .map(|step| {
                        let (status, detail) = match &step.status {
                            CleanupStatus::Removed => (NodeCleanupStepStatus::Removed, None),
                            CleanupStatus::NotOwned => (NodeCleanupStepStatus::NotOwned, None),
                            CleanupStatus::Deferred(_) => {
                                (NodeCleanupStepStatus::Deferred, Some("清理已延迟".into()))
                            }
                            CleanupStatus::Failed(_) => {
                                (NodeCleanupStepStatus::Failed, Some("资源清理失败".into()))
                            }
                            CleanupStatus::Unknown(_) => {
                                (NodeCleanupStepStatus::Unknown, Some("清理状态未知".into()))
                            }
                        };
                        NodeCleanupStepSnapshot {
                            network_name: network_name.clone(),
                            resource: step.resource.into(),
                            status,
                            detail,
                        }
                    })
                    .collect()
            })
            .collect::<Vec<_>>();

        let mut issues = Vec::new();
        if !self.residual_networks.is_empty() {
            issues.push(format!("残留网络: {}", self.residual_networks.join(", ")));
        }
        if self.sessions_remaining > 0 {
            issues.push(format!("运行时任务残留 {} 个", self.sessions_remaining));
        }
        if self.listeners_remaining > 0 {
            issues.push(format!("监听器残留 {} 个", self.listeners_remaining));
        }
        if self.peers_remaining > 0 {
            issues.push(format!("Peer 记录残留 {} 个", self.peers_remaining));
        }
        if self.embedded_services_remaining > 0 {
            issues.push(format!(
                "内嵌服务残留 {} 个",
                self.embedded_services_remaining
            ));
        }
        for step in &cleanup_steps {
            let issue = match step.status {
                NodeCleanupStepStatus::Removed | NodeCleanupStepStatus::NotOwned => continue,
                NodeCleanupStepStatus::Deferred => "清理已延迟",
                NodeCleanupStepStatus::Failed => "资源清理失败",
                NodeCleanupStepStatus::Unknown => "清理状态未知",
            };
            issues.push(format!("{}: {} {issue}", step.network_name, step.resource));
        }
        issues.sort();
        issues.dedup();

        NodeRuntimeResidueSnapshot {
            sessions_remaining: self.sessions_remaining,
            listeners_remaining: self.listeners_remaining,
            peers_remaining: self.peers_remaining,
            embedded_services_remaining: self.embedded_services_remaining,
            configuration_retained: self.configuration_retained,
            residual_networks: self.residual_networks.clone(),
            cleanup_steps,
            issues,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct RestartSummary {
    pub requested: usize,
    pub restarted: Vec<String>,
    pub failed: Vec<RestartFailure>,
    pub cleanup: RuntimeCleanupReport,
}

impl RestartSummary {
    pub fn message(&self) -> String {
        let mut parts = vec![format!(
            "重启网络: 已重启 {} / {} 个网络",
            self.restarted.len(),
            self.requested
        )];
        if !self.failed.is_empty() {
            let failures = self
                .failed
                .iter()
                .map(|failure| {
                    format!(
                        "{} [{}]: {}",
                        failure.network_name,
                        failure.phase.as_str(),
                        failure.message
                    )
                })
                .collect::<Vec<_>>()
                .join("; ");
            parts.push(format!("{} 个网络重启失败 ({failures})", self.failed.len()));
        }
        let residue = if self.cleanup.residual_networks.is_empty()
            && self.cleanup.resource_failures().is_empty()
        {
            "none".to_string()
        } else {
            let mut residue = self.cleanup.residual_networks.clone();
            residue.extend(self.cleanup.resource_failures());
            residue.join(", ")
        };
        parts.push(format!(
            "清理检查: sessions={}, listeners={}, peers={}, embedded_services={}, residual_networks=[{}]; 持久化配置{}",
            self.cleanup.sessions_remaining,
            self.cleanup.listeners_remaining,
            self.cleanup.peers_remaining,
            self.cleanup.embedded_services_remaining,
            residue,
            if self.cleanup.configuration_retained {
                "已保留"
            } else {
                "未完整保留"
            }
        ));
        parts.join("；")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CreateResult {
    pub network_name: String,
    pub network_id: String,
    pub public_key: String,
    pub owner_role: MembershipRole,
    pub next_steps: Vec<NodeNetworkCreationNextStep>,
    pub config_saved: bool,
    pub started: bool,
    pub ticket: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct JoinResult {
    pub network_name: String,
    pub coordinator_url: String,
    pub config_saved: bool,
    pub started: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LeaveResult {
    pub network_name: String,
    pub config_removed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RoleUpdateResult {
    pub network_name: String,
    pub role: NodeNetworkRole,
    pub enabled: bool,
    pub roles: Vec<NodeNetworkRole>,
}

#[derive(Debug, Clone)]
struct NetworkLaunchRequest {
    network_name: String,
    network_id: Option<NetworkId>,
    config_path: String,
    config: ClientConfig,
    local_exit_enabled: bool,
}

struct StartedNetworkRuntime {
    network_id: Option<String>,
    signaling: Option<Arc<SignalingClient>>,
    tun_name: String,
    assigned_ipv6: std::net::Ipv6Addr,
    assigned_ipv4: Option<std::net::Ipv4Addr>,
    p2p_listen_addr: std::net::SocketAddr,
    identity: NodeIdentitySnapshot,
    routing_control: mpsc::UnboundedSender<ipc::RuntimeCommand>,
    shutdown: watch::Sender<bool>,
    handle: JoinHandle<NetworkRuntimeExit>,
}

/// Result returned by a network task after its event loop has exited.
///
/// A failed cleanup must not destroy the only object capable of retrying the
/// rollback. The task transfers that owner back to the actor, which keeps it
/// alive until a later cleanup attempt succeeds.
struct NetworkRuntimeExit {
    cleanup: RuntimeCleanupOutcome,
    retained_owner: Option<Box<dyn ResidualRuntimeOwner>>,
}

trait ResidualRuntimeOwner: Send + Sync {
    fn shutdown(&mut self) -> Pin<Box<dyn Future<Output = RuntimeCleanupOutcome> + Send + '_>>;
}

struct LiveResidualRuntimeOwner {
    runtime: commands::ClientRuntime,
    registry: MultiNetworkRegistry,
    network_name: String,
}

impl ResidualRuntimeOwner for LiveResidualRuntimeOwner {
    fn shutdown(&mut self) -> Pin<Box<dyn Future<Output = RuntimeCleanupOutcome> + Send + '_>> {
        Box::pin(async move {
            let outcome = self.runtime.shutdown().await;
            if outcome.is_clean() {
                self.registry.unregister(&self.network_name).await;
            }
            outcome
        })
    }
}

trait NetworkRuntimeLauncher: Send + Sync + 'static {
    fn launch(&self, request: NetworkLaunchRequest) -> RuntimeFuture<StartedNetworkRuntime>;
}

struct LiveNetworkRuntimeLauncher {
    state: NodeStateStore,
    events: NodeEventBus,
    registry: MultiNetworkRegistry,
    failover_audit: Option<Arc<OwnerSecurityAudit>>,
    coordinator_takeover_wakeup: mpsc::UnboundedSender<()>,
}

impl NetworkRuntimeLauncher for LiveNetworkRuntimeLauncher {
    fn launch(&self, request: NetworkLaunchRequest) -> RuntimeFuture<StartedNetworkRuntime> {
        let state = self.state.clone();
        let events = self.events.clone();
        let registry = self.registry.clone();
        let failover_audit = self.failover_audit.clone();
        let coordinator_takeover_wakeup = self.coordinator_takeover_wakeup.clone();

        Box::pin(async move {
            let mut runtime = commands::prepare_client_runtime_ext(
                &request.config_path,
                request.config,
                request.network_id.clone(),
                Some(&request.network_name),
                request.local_exit_enabled,
                false,
                commands::RuntimeObservability {
                    state: Some(state.clone()),
                    failover_audit,
                },
            )
            .await
            .map_err(|error| {
                if let Some(stage) = network_setup::stage_from_error(&error) {
                    tracing::warn!(
                        network = %request.network_name,
                        ?stage,
                        error = %format!("{error:#}"),
                        "网络初始化因主机网络配置失败"
                    );
                    RuntimeControlError::NetworkSetup {
                        stage,
                        message: stage.fallback_message().into(),
                    }
                } else {
                    RuntimeControlError::Launch(format!(
                        "网络 [{}] 初始化失败: {error:#}",
                        request.network_name
                    ))
                }
            })?;
            runtime.set_coordinator_takeover_wakeup(coordinator_takeover_wakeup);

            let tun_name = runtime.tun_name().to_string();
            let assigned_ipv6 = runtime.assigned_ip;
            let assigned_ipv4 = runtime.assigned_ips.ipv4;
            let p2p_listen_addr = runtime.p2p.local_addr();
            let identity = runtime_metadata::identity_snapshot(&runtime.identity, None, None);
            let signaling = Some(runtime.signaling.clone());
            let p2p = runtime.p2p.clone();
            let stats = runtime.stats.clone();

            // Multi-network nodes disable each runtime's private IPC listener.
            // Replace its unused control receiver with the actor-owned channel so
            // routing changes are applied by the existing live event loop.
            let (routing_control, routing_control_rx) = mpsc::unbounded_channel();
            runtime.control_rx = routing_control_rx;

            registry
                .register(NetworkSessionHandle {
                    network_name: request.network_name.clone(),
                    tun_name: tun_name.clone(),
                    assigned_ip: assigned_ipv6,
                    assigned_ipv4,
                    p2p,
                    stats,
                })
                .await;
            runtime.attach_node_state(state.clone()).await;

            let (shutdown, mut shutdown_rx) = watch::channel(false);
            let network_name = request.network_name.clone();
            let handle = tokio::spawn(async move {
                tracing::info!("网络 [{network_name}] 正在启动事件循环");
                let (disconnect_reply, event_error) =
                    match runtime.run_with_shutdown(&mut shutdown_rx).await {
                        Ok(reply) => (reply, None),
                        Err(error) => {
                            tracing::error!("网络 [{network_name}] 事件循环异常退出: {error:#}");
                            (None, Some(format!("{error:#}")))
                        }
                    };
                tracing::info!("网络 [{network_name}] 正在清理资源");
                let cleanup = runtime.shutdown().await;
                let cleanup_clean = cleanup.is_clean();
                let retained_owner = if cleanup_clean {
                    drop(runtime);
                    None
                } else {
                    Some(Box::new(LiveResidualRuntimeOwner {
                        runtime,
                        registry: registry.clone(),
                        network_name: network_name.clone(),
                    }) as Box<dyn ResidualRuntimeOwner>)
                };
                let cleanup_issues = cleanup.issues();
                let status = if cleanup_clean && event_error.is_none() {
                    NodeNetworkStatus::Offline
                } else {
                    NodeNetworkStatus::Failed
                };
                let failure_message = event_error
                    .iter()
                    .chain(cleanup_issues.iter())
                    .cloned()
                    .collect::<Vec<_>>()
                    .join("; ");

                if cleanup_clean {
                    registry.unregister(&network_name).await;
                    state
                        .update(|snapshot| {
                            if let Some(network) = snapshot
                                .networks
                                .iter_mut()
                                .find(|network| network.name == network_name)
                            {
                                network.status = status;
                                network.assigned_ipv6 = None;
                                network.assigned_ipv4 = None;
                                network.p2p_listen_addr = None;
                                network.uptime_secs = 0;
                                network.peer_count = 0;
                            }
                            snapshot
                                .peers
                                .retain(|peer| peer.network_name != network_name);
                            if !failure_message.is_empty() {
                                snapshot.last_error = Some(
                                    NodeProblem::new(
                                        NodeProblemCode::Unavailable,
                                        failure_message.clone(),
                                    )
                                    .for_network(&network_name)
                                    .retryable(true),
                                );
                            }
                        })
                        .await;
                } else {
                    state
                        .update(|snapshot| {
                            if let Some(network) = snapshot
                                .networks
                                .iter_mut()
                                .find(|network| network.name == network_name)
                            {
                                network.status = NodeNetworkStatus::Failed;
                            }
                            snapshot.last_error = Some(
                                NodeProblem::new(
                                    NodeProblemCode::Unavailable,
                                    format!(
                                        "网络 [{network_name}] 资源清理未完成: {}",
                                        cleanup.summary()
                                    ),
                                )
                                .for_network(&network_name)
                                .retryable(true),
                            );
                        })
                        .await;
                }
                events.publish(NodeEventKind::NetworkChanged {
                    network_name: network_name.clone(),
                    status,
                });
                if let Some(reply) = disconnect_reply {
                    let result = if cleanup_clean {
                        Ok(())
                    } else {
                        Err(cleanup.summary())
                    };
                    let _ = reply.send(result);
                }
                if cleanup_clean {
                    tracing::info!("网络 [{network_name}] 已安全下线");
                } else {
                    tracing::error!(
                        "网络 [{network_name}] 清理未完成，保留残留状态: {}",
                        cleanup.summary()
                    );
                }
                NetworkRuntimeExit {
                    cleanup,
                    retained_owner,
                }
            });

            Ok(StartedNetworkRuntime {
                network_id: request.network_id.map(|network_id| network_id.to_string()),
                signaling,
                tun_name,
                assigned_ipv6,
                assigned_ipv4,
                p2p_listen_addr,
                identity,
                routing_control,
                shutdown,
                handle,
            })
        })
    }
}

#[derive(Debug, Clone, Default)]
struct CoordinatorContinuityMetrics {
    unavailable_since: Option<Instant>,
    control_plane_interruptions: u64,
    consecutive_failures: u64,
    recoveries: u64,
    last_recovery_duration: Option<Duration>,
    last_failure_reason: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CoordinatorTakeoverRefreshOutcome {
    Idle,
    Healthy,
    Failed,
}

impl CoordinatorContinuityMetrics {
    fn record_failure(&mut self, now: Instant, reason: String) -> (Duration, u64, u64) {
        if self.unavailable_since.is_none() {
            self.unavailable_since = Some(now);
            self.control_plane_interruptions += 1;
        }
        self.consecutive_failures += 1;
        self.last_failure_reason = Some(reason);
        (
            now.saturating_duration_since(
                self.unavailable_since
                    .expect("an interruption start is recorded before measuring it"),
            ),
            self.consecutive_failures,
            self.control_plane_interruptions,
        )
    }

    fn record_success(&mut self, now: Instant) -> Option<(Duration, u64, u64)> {
        let started_at = self.unavailable_since.take()?;
        let duration = now.saturating_duration_since(started_at);
        self.consecutive_failures = 0;
        self.recoveries += 1;
        self.last_recovery_duration = Some(duration);
        self.last_failure_reason = None;
        Some((duration, self.recoveries, self.control_plane_interruptions))
    }
}

struct NetworkRuntimeSession {
    network_id: Option<String>,
    signaling: Option<Arc<SignalingClient>>,
    coordinator_recovery: Option<CoordinatorRecoveryStatus>,
    coordinator_emergency: Option<CoordinatorEmergencyLeaderStatus>,
    coordinator_continuity: CoordinatorContinuityMetrics,
    tun_name: String,
    routing_control: mpsc::UnboundedSender<ipc::RuntimeCommand>,
    shutdown: watch::Sender<bool>,
    handle: JoinHandle<NetworkRuntimeExit>,
}

async fn wait_for_cancellation(mut cancel: watch::Receiver<bool>) {
    if *cancel.borrow() {
        return;
    }
    loop {
        if cancel.changed().await.is_err() {
            std::future::pending::<()>().await;
        }
        if *cancel.borrow() {
            return;
        }
    }
}

async fn run_diagnostic_collection<T>(
    budget: Duration,
    cancel: watch::Receiver<bool>,
    collection: impl Future<Output = Result<T, RuntimeControlError>>,
) -> Result<T, RuntimeControlError> {
    let collection = timeout(budget, collection);
    tokio::pin!(collection);
    tokio::select! {
        result = &mut collection => match result {
            Ok(result) => result,
            Err(_) => Err(RuntimeControlError::Timeout(
                "分层诊断超过硬超时，已停止采集并释放临时状态".into(),
            )),
        },
        _ = wait_for_cancellation(cancel) => Err(RuntimeControlError::Cancelled(
            "诊断已取消，临时状态已释放".into(),
        )),
    }
}

fn unix_time_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

fn next_diagnostic_id(started_at_unix_ms: u64) -> String {
    static NEXT_ID: AtomicU64 = AtomicU64::new(1);
    let sequence = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    format!("diag-{started_at_unix_ms}-{sequence:08x}")
}

#[derive(Debug, Clone, Copy)]
struct DiagnosticResult {
    state: NodeDiagnosticState,
    error_code: Option<&'static str>,
    message_key: &'static str,
    suggestion_key: Option<&'static str>,
}

impl DiagnosticResult {
    const fn new(state: NodeDiagnosticState, message_key: &'static str) -> Self {
        Self {
            state,
            error_code: None,
            message_key,
            suggestion_key: None,
        }
    }

    const fn failed(error_code: &'static str, suggestion_key: &'static str) -> Self {
        Self {
            state: NodeDiagnosticState::Failed,
            error_code: Some(error_code),
            message_key: "diagnostic.state.failed",
            suggestion_key: Some(suggestion_key),
        }
    }

    const fn unsupported(error_code: &'static str, suggestion_key: &'static str) -> Self {
        Self {
            state: NodeDiagnosticState::Unsupported,
            error_code: Some(error_code),
            message_key: "diagnostic.state.unsupported",
            suggestion_key: Some(suggestion_key),
        }
    }

    const fn no_permission(error_code: &'static str, suggestion_key: &'static str) -> Self {
        Self {
            state: NodeDiagnosticState::NoPermission,
            error_code: Some(error_code),
            message_key: "diagnostic.state.no_permission",
            suggestion_key: Some(suggestion_key),
        }
    }
}

fn build_diagnostic_checks(
    snapshot: &NodeSnapshot,
    residue: &NodeRuntimeResidueSnapshot,
    report_started_at_unix_ms: u64,
) -> Vec<NodeDiagnosticCheck> {
    NodeDiagnosticLayer::ALL
        .into_iter()
        .map(|layer| {
            let started_at_unix_ms = unix_time_ms().max(report_started_at_unix_ms);
            let timer = Instant::now();
            let result = diagnostic_result(layer, snapshot, residue);
            NodeDiagnosticCheck {
                layer,
                state: result.state,
                started_at_unix_ms,
                duration_ms: timer.elapsed().as_millis().try_into().unwrap_or(u64::MAX),
                error_code: result.error_code.map(str::to_owned),
                message_key: result.message_key.to_owned(),
                suggestion_key: result.suggestion_key.map(str::to_owned),
            }
        })
        .collect()
}

fn diagnostic_result(
    layer: NodeDiagnosticLayer,
    snapshot: &NodeSnapshot,
    residue: &NodeRuntimeResidueSnapshot,
) -> DiagnosticResult {
    match layer {
        NodeDiagnosticLayer::Network => network_diagnostic(snapshot),
        NodeDiagnosticLayer::Dns => {
            DiagnosticResult::new(NodeDiagnosticState::NotTested, "diagnostic.dns.not_tested")
        }
        NodeDiagnosticLayer::Tun => tun_diagnostic(snapshot, residue),
        NodeDiagnosticLayer::Coordinator => service_diagnostic(snapshot, layer),
        NodeDiagnosticLayer::Relay => service_diagnostic(snapshot, layer),
        NodeDiagnosticLayer::Exit => exit_diagnostic(snapshot),
    }
}

fn network_diagnostic(snapshot: &NodeSnapshot) -> DiagnosticResult {
    if snapshot.lifecycle.phase == NodePhase::Failed
        || !snapshot.lifecycle.failed_networks.is_empty()
    {
        return DiagnosticResult::failed(
            "diagnostic.network.lifecycle_failed",
            "diagnostic.suggestion.restart_node",
        );
    }
    if snapshot.lifecycle.phase == NodePhase::Running && snapshot.lifecycle.active_networks > 0 {
        return DiagnosticResult::new(NodeDiagnosticState::Passed, "diagnostic.network.passed");
    }
    DiagnosticResult::new(
        NodeDiagnosticState::NotTested,
        "diagnostic.network.not_running",
    )
}

fn tun_diagnostic(
    snapshot: &NodeSnapshot,
    residue: &NodeRuntimeResidueSnapshot,
) -> DiagnosticResult {
    use crate::tun::TunCapabilityReason;

    if !snapshot.tun_capabilities.supported {
        return DiagnosticResult::unsupported(
            "diagnostic.tun.platform_unsupported",
            "diagnostic.suggestion.use_supported_platform",
        );
    }
    if snapshot
        .tun_capabilities
        .reasons
        .contains(&TunCapabilityReason::PermissionDenied)
    {
        return DiagnosticResult::no_permission(
            "diagnostic.tun.permission_denied",
            "diagnostic.suggestion.grant_network_privileges",
        );
    }

    let online_networks = snapshot
        .networks
        .iter()
        .filter(|network| {
            matches!(
                network.status,
                NodeNetworkStatus::Online | NodeNetworkStatus::Degraded
            )
        })
        .collect::<Vec<_>>();
    if online_networks.is_empty() {
        return DiagnosticResult::new(NodeDiagnosticState::NotTested, "diagnostic.tun.not_running");
    }

    let runtime_is_healthy = residue.sessions_remaining > 0 || residue.configuration_retained;
    let tun_mapping_is_healthy = online_networks.iter().all(|network| {
        network.status == NodeNetworkStatus::Online
            && snapshot.interfaces.iter().any(|interface| {
                interface.name == network.tun_name
                    && interface.kind == NodeInterfaceKind::Tun
                    && interface.ownership == NodeInterfaceOwnership::Product
                    && interface.status == NodeInterfaceStatus::Up
            })
    });
    if tun_mapping_is_healthy && runtime_is_healthy {
        DiagnosticResult::new(NodeDiagnosticState::Passed, "diagnostic.tun.passed")
    } else {
        DiagnosticResult::failed(
            "diagnostic.tun.runtime_mapping_invalid",
            "diagnostic.suggestion.rebuild_tun",
        )
    }
}

fn service_diagnostic(snapshot: &NodeSnapshot, layer: NodeDiagnosticLayer) -> DiagnosticResult {
    let services = snapshot.embedded_services.iter().collect::<Vec<_>>();
    if services.is_empty() {
        return DiagnosticResult::new(
            NodeDiagnosticState::NotTested,
            if layer == NodeDiagnosticLayer::Coordinator {
                "diagnostic.coordinator.not_running"
            } else {
                "diagnostic.relay.not_running"
            },
        );
    }
    if services
        .iter()
        .any(|service| service.status == NodeEmbeddedServiceStatus::Failed)
    {
        return DiagnosticResult::failed(
            if layer == NodeDiagnosticLayer::Coordinator {
                "diagnostic.coordinator.service_failed"
            } else {
                "diagnostic.relay.service_failed"
            },
            "diagnostic.suggestion.restart_node",
        );
    }
    let running_address_present = services.iter().any(|service| {
        service.status == NodeEmbeddedServiceStatus::Running
            && match layer {
                NodeDiagnosticLayer::Coordinator => service.signaling_addr.is_some(),
                NodeDiagnosticLayer::Relay => service.relay_addr.is_some(),
                _ => false,
            }
    });
    if running_address_present {
        return DiagnosticResult::new(
            NodeDiagnosticState::Passed,
            if layer == NodeDiagnosticLayer::Coordinator {
                "diagnostic.coordinator.passed"
            } else {
                "diagnostic.relay.passed"
            },
        );
    }
    if services.iter().any(|service| {
        matches!(
            service.status,
            NodeEmbeddedServiceStatus::Starting | NodeEmbeddedServiceStatus::Stopping
        )
    }) {
        return DiagnosticResult::new(
            NodeDiagnosticState::NotTested,
            if layer == NodeDiagnosticLayer::Coordinator {
                "diagnostic.coordinator.transitioning"
            } else {
                "diagnostic.relay.transitioning"
            },
        );
    }
    DiagnosticResult::failed(
        if layer == NodeDiagnosticLayer::Coordinator {
            "diagnostic.coordinator.address_missing"
        } else {
            "diagnostic.relay.address_missing"
        },
        "diagnostic.suggestion.restart_node",
    )
}

fn exit_diagnostic(snapshot: &NodeSnapshot) -> DiagnosticResult {
    use fuckipv6_common::ExitNodeUnsupportedReason;

    let local_exit_networks = snapshot
        .networks
        .iter()
        .filter(|network| network.roles.contains(&NodeNetworkRole::ExitNode))
        .collect::<Vec<_>>();
    if !local_exit_networks.is_empty() {
        if !snapshot.exit_node_capabilities.supported {
            if snapshot
                .exit_node_capabilities
                .unsupported_reasons
                .contains(&ExitNodeUnsupportedReason::PrivilegesRequired)
            {
                return DiagnosticResult::no_permission(
                    "diagnostic.exit.permission_denied",
                    "diagnostic.suggestion.grant_network_privileges",
                );
            }
            return DiagnosticResult::unsupported(
                "diagnostic.exit.platform_unsupported",
                "diagnostic.suggestion.use_supported_platform",
            );
        }
        if local_exit_networks
            .iter()
            .any(|network| network.status == NodeNetworkStatus::Failed)
        {
            return DiagnosticResult::failed(
                "diagnostic.exit.runtime_failed",
                "diagnostic.suggestion.restart_node",
            );
        }
        if local_exit_networks.iter().any(|network| {
            matches!(
                network.status,
                NodeNetworkStatus::Online | NodeNetworkStatus::Degraded
            )
        }) {
            return DiagnosticResult::new(NodeDiagnosticState::Passed, "diagnostic.exit.passed");
        }
    }

    let exit_routes = snapshot
        .routing_summaries
        .iter()
        .filter(|summary| matches!(summary.default_action, NodeRoutingDefaultAction::Exit(_)))
        .collect::<Vec<_>>();
    if exit_routes.is_empty() {
        return DiagnosticResult::new(
            NodeDiagnosticState::NotTested,
            "diagnostic.exit.not_configured",
        );
    }
    if exit_routes.iter().any(|summary| {
        summary.status == NodeRoutingSummaryStatus::Invalid
            || summary.status == NodeRoutingSummaryStatus::ApplyFailed
            || (summary.status == NodeRoutingSummaryStatus::Current
                && !matches!(
                    summary.effective_target.as_ref(),
                    Some(NodeRoutingTargetSnapshot::Exit { .. })
                ))
    }) {
        return DiagnosticResult::failed(
            "diagnostic.exit.route_apply_failed",
            "diagnostic.suggestion.review_exit_route",
        );
    }
    if exit_routes.iter().any(|summary| {
        summary.status == NodeRoutingSummaryStatus::Current
            && matches!(
                summary.effective_target.as_ref(),
                Some(NodeRoutingTargetSnapshot::Exit { .. })
            )
    }) {
        return DiagnosticResult::new(NodeDiagnosticState::Passed, "diagnostic.exit.passed");
    }
    DiagnosticResult::new(NodeDiagnosticState::NotTested, "diagnostic.exit.stale")
}

fn diagnostic_recovery_actions(
    checks: &[NodeDiagnosticCheck],
    residue: &NodeRuntimeResidueSnapshot,
) -> Vec<NodeDiagnosticRecoveryAction> {
    let mut actions = Vec::new();
    let needs_restart = checks.iter().any(|check| {
        check.state == NodeDiagnosticState::Failed
            && matches!(
                check.layer,
                NodeDiagnosticLayer::Network
                    | NodeDiagnosticLayer::Coordinator
                    | NodeDiagnosticLayer::Relay
                    | NodeDiagnosticLayer::Exit
            )
    });
    if needs_restart {
        actions.push(NodeDiagnosticRecoveryAction {
            action_id: "restart_node".into(),
            impact_key: "diagnostic.recovery.restart_node.impact".into(),
            confirmation_required: true,
        });
    }
    let needs_tun_rebuild = checks.iter().any(|check| {
        check.layer == NodeDiagnosticLayer::Tun
            && matches!(
                check.state,
                NodeDiagnosticState::Failed | NodeDiagnosticState::NoPermission
            )
    });
    if needs_tun_rebuild {
        actions.push(NodeDiagnosticRecoveryAction {
            action_id: "rebuild_tun".into(),
            impact_key: "diagnostic.recovery.rebuild_tun.impact".into(),
            confirmation_required: true,
        });
    }
    if !residue.is_clean() {
        actions.push(NodeDiagnosticRecoveryAction {
            action_id: "cleanup_residuals".into(),
            impact_key: "diagnostic.recovery.cleanup_residuals.impact".into(),
            confirmation_required: true,
        });
    }
    actions
}

fn build_exit_health_snapshot(
    snapshot: &NodeSnapshot,
    leak_detection: &LeakDetectionSnapshot,
    started_at_unix_ms: u64,
    completed_at_unix_ms: u64,
) -> ExitHealthSnapshot {
    let local_exit_networks = snapshot
        .networks
        .iter()
        .filter(|network| network.roles.contains(&NodeNetworkRole::ExitNode))
        .collect::<Vec<_>>();

    let mut evidence = Vec::new();
    if !local_exit_networks.is_empty() {
        let process_state = if snapshot.local_exit.problem.is_some()
            || snapshot.local_exit.stage == LocalExitCapabilityStage::Failed
        {
            ExitHealthEvidenceState::Failed
        } else if matches!(
            snapshot.local_exit.stage,
            LocalExitCapabilityStage::Listening
                | LocalExitCapabilityStage::HealthChecking
                | LocalExitCapabilityStage::Enabled
                | LocalExitCapabilityStage::Draining
                | LocalExitCapabilityStage::Cleaning
        ) {
            ExitHealthEvidenceState::Passed
        } else {
            ExitHealthEvidenceState::NotTested
        };
        evidence.push(ExitHealthEvidence {
            layer: ExitHealthLayer::Process,
            state: process_state,
            source: ExitHealthSource::LocalProcess,
            observed_at_unix_ms: completed_at_unix_ms.max(1),
            duration_ms: 0,
            target: None,
            protocol: None,
            error_code: snapshot
                .local_exit
                .problem
                .as_ref()
                .map(|_| "exit.health.process_unhealthy".into()),
        });

        let listening_state = if matches!(
            snapshot.local_exit.stage,
            LocalExitCapabilityStage::Listening | LocalExitCapabilityStage::Enabled
        ) {
            ExitHealthEvidenceState::Passed
        } else if snapshot.local_exit.stage == LocalExitCapabilityStage::Failed {
            ExitHealthEvidenceState::Failed
        } else {
            ExitHealthEvidenceState::NotTested
        };
        evidence.push(ExitHealthEvidence {
            layer: ExitHealthLayer::Listening,
            state: listening_state,
            source: ExitHealthSource::LocalSocket,
            observed_at_unix_ms: completed_at_unix_ms.max(1),
            duration_ms: 0,
            target: None,
            protocol: None,
            error_code: None,
        });

        let network_state = if local_exit_networks
            .iter()
            .any(|network| network.status == NodeNetworkStatus::Offline)
        {
            ExitHealthEvidenceState::Failed
        } else if local_exit_networks.iter().any(|network| {
            matches!(
                network.status,
                NodeNetworkStatus::Online | NodeNetworkStatus::Degraded
            )
        }) {
            ExitHealthEvidenceState::Passed
        } else {
            ExitHealthEvidenceState::NotTested
        };
        evidence.push(ExitHealthEvidence {
            layer: ExitHealthLayer::Network,
            state: network_state,
            source: ExitHealthSource::LocalNetwork,
            observed_at_unix_ms: completed_at_unix_ms.max(1),
            duration_ms: 0,
            target: None,
            protocol: None,
            error_code: None,
        });
    }

    // Exit DNS and reachability are projected from leak evidence rather than
    // inferred from a healthy process/listener. Keep one deterministic evidence
    // item per protocol: the worst state wins, then the newest observation, and
    // finally the stable target key. This keeps a passing host route from hiding
    // an escaping route, an unexecuted probe or an isolated unavailable family.
    evidence.extend(
        aggregate_leak_evidence_for_exit_health(leak_detection)
            .into_iter()
            .map(leak_evidence_to_exit_health),
    );

    let retest = ExitHealthRetest {
        phase: ExitHealthRetestPhase::Completed,
        requested_at_unix_ms: started_at_unix_ms.max(1),
        started_at_unix_ms: Some(started_at_unix_ms.max(1)),
        finished_at_unix_ms: Some(completed_at_unix_ms.max(1)),
    };
    ExitHealthSnapshot {
        evidence,
        retest,
        ..ExitHealthSnapshot::default()
    }
}

fn aggregate_leak_evidence_for_exit_health(
    leak_detection: &LeakDetectionSnapshot,
) -> Vec<&LeakDetectionEvidence> {
    let mut by_protocol = BTreeMap::<LeakDetectionProtocol, &LeakDetectionEvidence>::new();
    for candidate in &leak_detection.evidence {
        let protocol = candidate.target.protocol();
        match by_protocol.get(&protocol) {
            Some(current) if !leak_evidence_is_worse(candidate, current) => {}
            _ => {
                by_protocol.insert(protocol, candidate);
            }
        }
    }
    by_protocol.into_values().collect()
}

fn leak_evidence_is_worse(
    candidate: &LeakDetectionEvidence,
    current: &LeakDetectionEvidence,
) -> bool {
    leak_evidence_severity(candidate.state)
        .cmp(&leak_evidence_severity(current.state))
        .then_with(|| {
            candidate
                .observed_at_unix_ms
                .cmp(&current.observed_at_unix_ms)
        })
        .then_with(|| current.target.key().cmp(candidate.target.key()))
        .is_gt()
}

fn leak_evidence_severity(state: LeakDetectionEvidenceState) -> u8 {
    match state {
        LeakDetectionEvidenceState::Passed => 0,
        LeakDetectionEvidenceState::Unsupported => 1,
        LeakDetectionEvidenceState::NotTested => 2,
        LeakDetectionEvidenceState::Failed => 3,
    }
}

fn leak_evidence_to_exit_health(evidence: &LeakDetectionEvidence) -> ExitHealthEvidence {
    ExitHealthEvidence {
        layer: match evidence.target.protocol() {
            LeakDetectionProtocol::Dns => ExitHealthLayer::Dns,
            LeakDetectionProtocol::Ipv4 | LeakDetectionProtocol::Ipv6 => {
                ExitHealthLayer::InternetReachability
            }
        },
        state: match evidence.state {
            LeakDetectionEvidenceState::Passed => ExitHealthEvidenceState::Passed,
            LeakDetectionEvidenceState::Failed => ExitHealthEvidenceState::Failed,
            LeakDetectionEvidenceState::Unsupported => ExitHealthEvidenceState::Unsupported,
            LeakDetectionEvidenceState::NotTested => ExitHealthEvidenceState::NotTested,
        },
        source: match evidence.source {
            LeakDetectionSource::SystemResolver | LeakDetectionSource::ManagedDoh => {
                ExitHealthSource::LocalDns
            }
            LeakDetectionSource::LocalRoutingTable | LeakDetectionSource::LinkCapability => {
                ExitHealthSource::LocalNetwork
            }
            LeakDetectionSource::ControlledProbe
            | LeakDetectionSource::ExternalReachabilityProbe
            | LeakDetectionSource::ApplicationBehaviorProbe => ExitHealthSource::ControlledProbe,
        },
        observed_at_unix_ms: evidence.observed_at_unix_ms,
        duration_ms: evidence.duration_ms,
        target: Some(leak_target_to_exit_target(evidence.target)),
        protocol: Some(match evidence.target.protocol() {
            LeakDetectionProtocol::Dns => ExitHealthProtocol::Dns,
            LeakDetectionProtocol::Ipv4 => ExitHealthProtocol::Ipv4,
            LeakDetectionProtocol::Ipv6 => ExitHealthProtocol::Ipv6,
        }),
        error_code: evidence.error_code.clone(),
    }
}

fn leak_target_to_exit_target(target: LeakDetectionTarget) -> ExitHealthTarget {
    match target {
        LeakDetectionTarget::DnsResolver => ExitHealthTarget::DnsResolver,
        LeakDetectionTarget::Ipv4ExternalTarget => ExitHealthTarget::Ipv4Internet,
        LeakDetectionTarget::Ipv6ExternalTarget => ExitHealthTarget::Ipv6Internet,
        LeakDetectionTarget::BrowserApplicationDns
        | LeakDetectionTarget::Ipv4Carrier
        | LeakDetectionTarget::Ipv4SourceAddress
        | LeakDetectionTarget::Ipv4DefaultRoute
        | LeakDetectionTarget::Ipv4ControlledLeakService
        | LeakDetectionTarget::Ipv6Carrier
        | LeakDetectionTarget::Ipv6SourceAddress
        | LeakDetectionTarget::Ipv6DefaultRoute
        | LeakDetectionTarget::Ipv6ControlledLeakService => {
            ExitHealthTarget::Controlled(target.key().into())
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn build_sanitized_diagnostic_report(
    diagnostic_id: &str,
    started_at_unix_ms: u64,
    completed_at_unix_ms: u64,
    timed_out: bool,
    checks: &[NodeDiagnosticCheck],
    exit_health: &ExitHealthSnapshot,
    leak_detection: &LeakDetectionSnapshot,
    recovery_actions: &[NodeDiagnosticRecoveryAction],
    residue: &NodeRuntimeResidueSnapshot,
) -> String {
    let mut report = format!(
        "FuckIPv6 diagnostics\nid: {diagnostic_id}\nstarted_at_unix_ms: {started_at_unix_ms}\ncompleted_at_unix_ms: {completed_at_unix_ms}\ntimed_out: {timed_out}\nchecks:\n"
    );
    for check in checks {
        report.push_str(&format!(
            "- {}: {} duration_ms={} error_code={} message_key={} suggestion_key={}\n",
            check.layer.key(),
            check.state.key(),
            check.duration_ms,
            check.error_code.as_deref().unwrap_or("none"),
            check.message_key,
            check.suggestion_key.as_deref().unwrap_or("none"),
        ));
    }
    let exit_health_assessment = exit_health.assess(completed_at_unix_ms);
    report.push_str(&format!(
        "exit_health: state={} default_candidate={} reason={} affected_layers={} affected_targets={} affected_protocols={}\n",
        match exit_health_assessment.state {
            fuckipv6_common::domain::ExitHealthState::Unknown => "unknown",
            fuckipv6_common::domain::ExitHealthState::Available => "available",
            fuckipv6_common::domain::ExitHealthState::Degraded => "degraded",
            fuckipv6_common::domain::ExitHealthState::Unavailable => "unavailable",
        },
        exit_health_assessment.default_candidate,
        exit_health_assessment.reason_code,
        exit_health_assessment
            .affected_layers
            .iter()
            .map(|layer| layer.key())
            .collect::<Vec<_>>()
            .join(","),
        exit_health_assessment
            .affected_targets
            .iter()
            .map(ExitHealthTarget::key)
            .collect::<Vec<_>>()
            .join(","),
        exit_health_assessment
            .affected_protocols
            .iter()
            .map(|protocol| match protocol {
                ExitHealthProtocol::Dns => "dns",
                ExitHealthProtocol::Tcp => "tcp",
                ExitHealthProtocol::Udp => "udp",
                ExitHealthProtocol::Quic => "quic",
                ExitHealthProtocol::Ipv4 => "ipv4",
                ExitHealthProtocol::Ipv6 => "ipv6",
            })
            .collect::<Vec<_>>()
            .join(","),
    ));
    let leak_assessment = leak_detection.assess(completed_at_unix_ms);
    report.push_str(&format!(
        "leak_detection: state={} reason={} fail_closed_required={} affected_protocols={} affected_targets={}\n",
        leak_assessment.state.key(),
        leak_assessment.reason_code,
        leak_assessment.fail_closed_required,
        leak_assessment
            .affected_protocols
            .iter()
            .map(|protocol| protocol.key())
            .collect::<Vec<_>>()
            .join(","),
        leak_assessment
            .affected_targets
            .iter()
            .map(|target| target.key())
            .collect::<Vec<_>>()
            .join(","),
    ));
    report.push_str("leak_detection_evidence:\n");
    for evidence in &leak_detection.evidence {
        report.push_str(&format!(
            "- {}: state={} source={} observed_at_unix_ms={} duration_ms={} error_code={} isolation_action={}\n",
            evidence.target.key(),
            leak_evidence_state_key(evidence.state),
            leak_source_key(evidence.source),
            evidence.observed_at_unix_ms,
            evidence.duration_ms,
            stable_report_token(evidence.error_code.as_deref()),
            evidence
                .isolation_action
                .map(leak_isolation_action_key)
                .unwrap_or("none"),
        ));
    }
    report.push_str(&format!(
        "residue: sessions={} listeners={} peers={} services={} residual_networks={}\n",
        residue.sessions_remaining,
        residue.listeners_remaining,
        residue.peers_remaining,
        residue.embedded_services_remaining,
        residue.residual_networks.len(),
    ));
    report.push_str("recovery_actions:\n");
    for action in recovery_actions {
        report.push_str(&format!(
            "- {}: impact_key={} confirmation_required={}\n",
            action.action_id, action.impact_key, action.confirmation_required,
        ));
    }
    report
}

fn leak_evidence_state_key(state: LeakDetectionEvidenceState) -> &'static str {
    match state {
        LeakDetectionEvidenceState::Passed => "passed",
        LeakDetectionEvidenceState::Failed => "failed",
        LeakDetectionEvidenceState::Unsupported => "unsupported",
        LeakDetectionEvidenceState::NotTested => "not_tested",
    }
}

fn leak_source_key(source: LeakDetectionSource) -> &'static str {
    match source {
        LeakDetectionSource::SystemResolver => "system_resolver",
        LeakDetectionSource::ManagedDoh => "managed_doh",
        LeakDetectionSource::LocalRoutingTable => "local_routing_table",
        LeakDetectionSource::LinkCapability => "link_capability",
        LeakDetectionSource::ControlledProbe => "controlled_probe",
        LeakDetectionSource::ExternalReachabilityProbe => "external_reachability_probe",
        LeakDetectionSource::ApplicationBehaviorProbe => "application_behavior_probe",
    }
}

fn leak_isolation_action_key(action: LeakDetectionIsolationAction) -> &'static str {
    match action {
        LeakDetectionIsolationAction::BlockedUnavailableIpv4 => "blocked_unavailable_ipv4",
        LeakDetectionIsolationAction::BlockedUnavailableIpv6 => "blocked_unavailable_ipv6",
        LeakDetectionIsolationAction::FailClosedAffectedTraffic => "fail_closed_affected_traffic",
        LeakDetectionIsolationAction::RetainedProtectedRoute => "retained_protected_route",
    }
}

fn stable_report_token(value: Option<&str>) -> &str {
    value
        .filter(|value| {
            !value.is_empty()
                && value.len() <= 128
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
        })
        .unwrap_or("none")
}

/// A manager-wide single-flight gate for lifecycle operations. The permit is
/// moved into the actor command, so cancellation of the caller cannot release
/// it while the actor is still changing runtime state.
struct OperationGate {
    busy: AtomicBool,
}

struct OperationGuard {
    gate: Arc<OperationGate>,
}

impl OperationGate {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            busy: AtomicBool::new(false),
        })
    }

    fn try_acquire(self: &Arc<Self>) -> Result<OperationGuard, RuntimeControlError> {
        self.busy
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map(|_| OperationGuard { gate: self.clone() })
            .map_err(|_| {
                RuntimeControlError::Conflict(
                    "已有网络启动、创建、加入、停止或重启操作正在进行".into(),
                )
            })
    }
}

impl Drop for OperationGuard {
    fn drop(&mut self) {
        self.gate.busy.store(false, Ordering::Release);
    }
}

enum RuntimeControlCommand {
    Diagnostics {
        cancel: watch::Receiver<bool>,
        reply: oneshot::Sender<Result<NodeDiagnosticsSnapshot, RuntimeControlError>>,
    },
    LiveTunNames {
        reply: oneshot::Sender<Result<BTreeMap<String, String>, RuntimeControlError>>,
    },
    StartPlan {
        plan: NodeRuntimePlan,
        _operation: OperationGuard,
        reply: oneshot::Sender<Result<StartSummary, RuntimeControlError>>,
    },
    StartSelection {
        selection: NodeNetworkSelection,
        _operation: OperationGuard,
        reply: oneshot::Sender<Result<StartSummary, RuntimeControlError>>,
    },
    Create {
        command: NodeCreateNetworkCommand,
        _operation: OperationGuard,
        reply: oneshot::Sender<Result<CreateResult, RuntimeControlError>>,
    },
    Join {
        command: Box<NodeJoinNetworkCommand>,
        _operation: OperationGuard,
        reply: oneshot::Sender<Result<JoinResult, RuntimeControlError>>,
    },
    Leave {
        network_name: String,
        _operation: OperationGuard,
        reply: oneshot::Sender<Result<LeaveResult, RuntimeControlError>>,
    },
    UpdateRouting {
        command: NodeUpdateRoutingCommand,
        reply: oneshot::Sender<Result<ipc::RoutingUpdateResult, RuntimeControlError>>,
    },
    SetNetworkRole {
        command: NodeSetNetworkRoleCommand,
        reply: oneshot::Sender<Result<RoleUpdateResult, RuntimeControlError>>,
    },
    SetLocalExitCapability {
        enabled: bool,
        _operation: OperationGuard,
        reply: oneshot::Sender<Result<LocalExitCapabilitySnapshot, RuntimeControlError>>,
    },
    JoinRequest {
        command: JoinRequestRuntimeCommand,
        reply: oneshot::Sender<Result<JoinRequestExecutionResult, RuntimeControlError>>,
    },
    Invitation {
        command: InvitationRuntimeCommand,
        reply: oneshot::Sender<Result<InvitationExecutionResult, RuntimeControlError>>,
    },
    CoordinatorCandidates {
        command: CoordinatorCandidateRuntimeCommand,
        reply: oneshot::Sender<Result<CoordinatorCandidateStatus, RuntimeControlError>>,
    },
    AdminRecovery {
        command: AdminRecoveryRuntimeCommand,
        reply: oneshot::Sender<Result<AdminRecoveryExecutionResult, RuntimeControlError>>,
    },
    Stop {
        networks: Vec<String>,
        all: bool,
        _operation: OperationGuard,
        reply: oneshot::Sender<Result<StopSummary, RuntimeControlError>>,
    },
    Restart {
        networks: Vec<String>,
        all: bool,
        _operation: OperationGuard,
        reply: oneshot::Sender<Result<RestartSummary, RuntimeControlError>>,
    },
    RebuildTun {
        network_name: String,
        _operation: OperationGuard,
        reply: oneshot::Sender<Result<RestartSummary, RuntimeControlError>>,
    },
    StartNode {
        _operation: OperationGuard,
        reply: oneshot::Sender<Result<StartSummary, RuntimeControlError>>,
    },
    StopNode {
        _operation: OperationGuard,
        reply: oneshot::Sender<Result<StopSummary, RuntimeControlError>>,
    },
    RestartNode {
        _operation: OperationGuard,
        reply: oneshot::Sender<Result<RestartSummary, RuntimeControlError>>,
    },
    CleanupResiduals {
        networks: Vec<String>,
        all: bool,
        _operation: OperationGuard,
        reply: oneshot::Sender<Result<StopSummary, RuntimeControlError>>,
    },
    Shutdown {
        _operation: OperationGuard,
        reply: oneshot::Sender<Result<StopSummary, RuntimeControlError>>,
    },
}

enum JoinRequestRuntimeCommand {
    Submit {
        request: Box<JoinRequest>,
        coordinator_url: Option<String>,
    },
    List {
        network_id: NetworkId,
        coordinator_url: Option<String>,
    },
    ListMembers {
        network_id: NetworkId,
        coordinator_url: Option<String>,
    },
    PreviewRemoval {
        request: MemberRemovalPreviewRequest,
        coordinator_url: Option<String>,
    },
    LifecycleChange {
        request: MemberLifecycleChangeRequest,
        coordinator_url: Option<String>,
    },
    Review {
        network_id: NetworkId,
        reviews: Vec<JoinRequestBatchReviewItem>,
    },
    Withdraw {
        network_id: NetworkId,
        request_id: JoinRequestId,
        coordinator_url: Option<String>,
    },
    Resubmit {
        network_id: NetworkId,
        request_id: JoinRequestId,
        expected_revision: u64,
        reason: String,
        coordinator_url: Option<String>,
    },
}

enum JoinRequestExecutionResult {
    Status(JoinRequestStatusSnapshot),
    List(Vec<JoinRequest>),
    Members(NetworkMemberDirectory),
    RemovalPreview(MemberRemovalPreview),
    LifecycleChanged(MemberLifecycleChange),
    Review(Vec<JoinRequestReviewResult>),
}

enum InvitationRuntimeCommand {
    Create(InvitationCreateRequest),
    List {
        network_id: NetworkId,
    },
    Status {
        network_id: NetworkId,
        invitation_id: InvitationId,
    },
    Revoke {
        network_id: NetworkId,
        invitation_id: InvitationId,
        expected_revision: u64,
    },
    Preview {
        network_id: NetworkId,
        coordinator_url: String,
        code: InvitationCode,
    },
    Redeem {
        coordinator_url: String,
        request: InvitationRedeemRequest,
    },
}

enum InvitationExecutionResult {
    Created(IssuedInvitation),
    List(Vec<InvitationStatusView>),
    Status(InvitationStatusView),
    Revoked(InvitationStatusView),
    Preview(InvitationPreview),
    Redeemed(InvitationRedemption),
}

enum CoordinatorCandidateRuntimeCommand {
    Status {
        network_name: String,
    },
    UpdateConfig(NodeUpdateCoordinatorCandidatesCommand),
    PublishDeclaration(NodePublishCoordinatorCandidateDeclarationCommand),
    // W14-03 wires these actor-owned election entry points once a verified
    // majority proof exists. Keeping them here now makes the authorization
    // boundary testable without exposing an unsafe user command.
    #[allow(dead_code)]
    InstallTerm {
        network_name: String,
        term: CoordinatorTerm,
    },
    #[allow(dead_code)]
    RenewTermLease {
        network_name: String,
        credential: CoordinatorWriteCredential,
        new_expires_at_unix_ms: u64,
    },
}

enum AdminRecoveryRuntimeCommand {
    Submit(NodeSubmitAdminRecoveryCommand),
    Status(NodeAdminRecoveryStatusCommand),
    List(NodeAdminRecoveryListCommand),
    Revoke(NodeRevokeAdminRecoveryCommand),
    Execute(NodeExecuteAdminRecoveryCommand),
}

enum AdminRecoveryExecutionResult {
    Status(AdminRecoveryStatusView),
    List(Vec<AdminRecoveryStatusView>),
}

fn routing_summary_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| {
            u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
        })
}

fn network_membership_snapshot(
    joined: &JoinedNetworkMembership,
    runtime: Option<NodeNetworkStatus>,
) -> NodeNetworkMembershipSnapshot {
    let membership = &joined.membership;
    NodeNetworkMembershipSnapshot {
        network_name: joined.network_name.clone(),
        network_id: membership.network_id.to_string(),
        node_id: membership.node_id.to_string(),
        state: effective_membership_state(membership.state, runtime),
        role: membership.role,
        local_policy_revision: membership.local_policy_revision,
        network_policy_revision: membership
            .terms
            .as_ref()
            .map(|terms| terms.network_policy_revision),
        privacy_policy_revision: membership
            .terms
            .as_ref()
            .map(|terms| terms.privacy_policy_revision),
        member_limit: membership
            .terms
            .as_ref()
            .and_then(|terms| terms.member_limit),
        exit_contribution: membership
            .terms
            .as_ref()
            .map(|terms| terms.exit_contribution),
        exit_contribution_scope: membership
            .terms
            .as_ref()
            .map(|terms| terms.exit_contribution_scope),
        exit_contribution_mode: membership
            .terms
            .as_ref()
            .map(|terms| terms.exit_contribution_mode),
        terms_hash: membership
            .terms_ack
            .as_ref()
            .map(|ack| ack.terms_hash.as_str().to_owned()),
        current_terms: membership.current_terms.clone(),
        current_terms_hash: membership
            .current_terms
            .as_ref()
            .and_then(|terms| terms.content_hash().ok())
            .map(|hash| hash.as_str().to_owned()),
        policy_confirmation_required: membership.policy_confirmation_required(),
        acknowledged_at_unix_ms: membership
            .terms_ack
            .as_ref()
            .map(|ack| ack.acknowledged_at_unix_ms),
        created_at_unix_ms: membership.created_at_unix_ms,
        updated_at_unix_ms: membership.updated_at_unix_ms,
        suspension_reason: membership.suspension_reason.clone(),
    }
}

fn routing_problem(
    network_name: &str,
    product_key: &'static str,
    code: NodeProblemCode,
    message: impl Into<String>,
) -> NodeProblem {
    NodeProblem::new(code, message)
        .for_network(network_name)
        .with_message_key(product_key)
}

fn routing_default_action(
    config: &NodeNetworkConfigSnapshot,
) -> (NodeRoutingDefaultAction, Option<NodeRoutingTargetSnapshot>) {
    match config.routing_mode {
        NodeRoutingMode::MeshOnly => (NodeRoutingDefaultAction::Direct, None),
        NodeRoutingMode::BypassMainland | NodeRoutingMode::GlobalProxy => {
            match config
                .exit_node
                .as_deref()
                .filter(|node| !node.trim().is_empty())
            {
                Some(node_id) => (
                    NodeRoutingDefaultAction::Exit(node_id.to_string()),
                    Some(NodeRoutingTargetSnapshot::Exit {
                        node_id: node_id.to_string(),
                    }),
                ),
                None => (
                    NodeRoutingDefaultAction::Network,
                    Some(NodeRoutingTargetSnapshot::Network {
                        network_name: config.network_name.clone(),
                    }),
                ),
            }
        }
    }
}

fn routing_rule_count(config: &NodeNetworkConfigSnapshot) -> usize {
    config
        .direct_cidrs
        .len()
        .saturating_add(config.proxy_cidrs.len())
}

fn routing_summary_with_status(
    snapshot: &NodeSnapshot,
    config: &NodeNetworkConfigSnapshot,
    status: NodeRoutingSummaryStatus,
    problem: Option<NodeProblem>,
) -> NodeRoutingSummarySnapshot {
    let previous = snapshot
        .routing_summaries
        .iter()
        .find(|summary| summary.network_name == config.network_name);
    if status != NodeRoutingSummaryStatus::Current {
        if let Some(previous) = previous {
            let mut summary = (*previous).clone();
            summary.status = status;
            summary.problem = problem;
            return summary;
        }
    }
    let (last_applied_revision, last_applied_at_unix_ms) =
        if status == NodeRoutingSummaryStatus::Current {
            (
                Some(
                    previous
                        .and_then(|summary| summary.last_applied_revision)
                        .unwrap_or(0)
                        .saturating_add(1),
                ),
                Some(routing_summary_timestamp()),
            )
        } else {
            (None, None)
        };
    let (default_action, effective_target) = routing_default_action(config);
    NodeRoutingSummarySnapshot {
        network_name: config.network_name.clone(),
        default_action,
        effective_target,
        enabled_rule_count: routing_rule_count(config),
        last_applied_revision,
        last_applied_at_unix_ms,
        status,
        problem,
    }
}

fn replace_routing_summary(snapshot: &mut NodeSnapshot, summary: NodeRoutingSummarySnapshot) {
    if let Some(existing) = snapshot
        .routing_summaries
        .iter_mut()
        .find(|existing| existing.network_name == summary.network_name)
    {
        *existing = summary;
    } else {
        snapshot.routing_summaries.push(summary);
    }
    snapshot
        .routing_summaries
        .sort_by(|left, right| left.network_name.cmp(&right.network_name));
}

/// Cloneable control handle for the actor that owns live network runtimes.
#[derive(Clone)]
pub(crate) struct NetworkRuntimeManager {
    sender: mpsc::Sender<RuntimeControlCommand>,
    operation_gate: Arc<OperationGate>,
}

impl NetworkRuntimeManager {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn spawn_with_failover_audit(
        configs: ConfigManager,
        options: RuntimeOptions,
        state: NodeStateStore,
        events: NodeEventBus,
        lifecycle: NodeLifecycle,
        registry: MultiNetworkRegistry,
        services: EmbeddedServiceManager,
        failover_audit: Option<Arc<OwnerSecurityAudit>>,
    ) -> Self {
        let (coordinator_takeover_wakeup, coordinator_takeover_wakeups) = mpsc::unbounded_channel();
        let launcher = Arc::new(LiveNetworkRuntimeLauncher {
            state: state.clone(),
            events: events.clone(),
            registry: registry.clone(),
            failover_audit,
            coordinator_takeover_wakeup,
        });
        Self::spawn_with_launcher_and_timeouts_inner(
            configs,
            options,
            state,
            events,
            lifecycle,
            registry,
            services,
            launcher,
            RuntimeTimeouts::default(),
            Some(coordinator_takeover_wakeups),
        )
    }

    #[allow(clippy::too_many_arguments)]
    #[cfg(test)]
    fn spawn_with_launcher_and_timeouts(
        configs: ConfigManager,
        options: RuntimeOptions,
        state: NodeStateStore,
        events: NodeEventBus,
        lifecycle: NodeLifecycle,
        registry: MultiNetworkRegistry,
        services: EmbeddedServiceManager,
        launcher: Arc<dyn NetworkRuntimeLauncher>,
        timeouts: RuntimeTimeouts,
    ) -> Self {
        Self::spawn_with_launcher_and_timeouts_inner(
            configs, options, state, events, lifecycle, registry, services, launcher, timeouts,
            None,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn spawn_with_launcher_and_timeouts_inner(
        configs: ConfigManager,
        options: RuntimeOptions,
        state: NodeStateStore,
        events: NodeEventBus,
        lifecycle: NodeLifecycle,
        registry: MultiNetworkRegistry,
        services: EmbeddedServiceManager,
        launcher: Arc<dyn NetworkRuntimeLauncher>,
        timeouts: RuntimeTimeouts,
        coordinator_takeover_wakeups: Option<mpsc::UnboundedReceiver<()>>,
    ) -> Self {
        let (sender, receiver) = mpsc::channel(16);
        tokio::spawn(run_actor(
            receiver,
            NetworkRuntimeActor {
                configs,
                options,
                state,
                events,
                lifecycle,
                registry,
                services,
                launcher,
                sessions: HashMap::new(),
                retained_owners: HashMap::new(),
                cleanup_failures: BTreeMap::new(),
                completed_cleanup: BTreeMap::new(),
                timeouts,
                coordinator_takeover_wakeups,
            },
        ));
        Self {
            sender,
            operation_gate: OperationGate::new(),
        }
    }

    pub async fn start_plan(
        &self,
        plan: NodeRuntimePlan,
    ) -> Result<StartSummary, RuntimeControlError> {
        let operation = self.operation_gate.try_acquire()?;
        let (reply, response) = oneshot::channel();
        self.send(RuntimeControlCommand::StartPlan {
            plan,
            _operation: operation,
            reply,
        })
        .await?;
        response
            .await
            .map_err(|_| RuntimeControlError::ActorClosed)?
    }

    pub async fn start_selection(
        &self,
        selection: NodeNetworkSelection,
    ) -> Result<StartSummary, RuntimeControlError> {
        let operation = self.operation_gate.try_acquire()?;
        let (reply, response) = oneshot::channel();
        self.send(RuntimeControlCommand::StartSelection {
            selection,
            _operation: operation,
            reply,
        })
        .await?;
        response
            .await
            .map_err(|_| RuntimeControlError::ActorClosed)?
    }

    pub async fn create(
        &self,
        command: NodeCreateNetworkCommand,
    ) -> Result<CreateResult, RuntimeControlError> {
        let operation = self.operation_gate.try_acquire()?;
        let (reply, response) = oneshot::channel();
        self.send(RuntimeControlCommand::Create {
            command,
            _operation: operation,
            reply,
        })
        .await?;
        response
            .await
            .map_err(|_| RuntimeControlError::ActorClosed)?
    }

    pub async fn join(
        &self,
        command: NodeJoinNetworkCommand,
    ) -> Result<JoinResult, RuntimeControlError> {
        let operation = self.operation_gate.try_acquire()?;
        let (reply, response) = oneshot::channel();
        self.send(RuntimeControlCommand::Join {
            command: Box::new(command),
            _operation: operation,
            reply,
        })
        .await?;
        response
            .await
            .map_err(|_| RuntimeControlError::ActorClosed)?
    }

    pub async fn leave(&self, network_name: String) -> Result<LeaveResult, RuntimeControlError> {
        let operation = self.operation_gate.try_acquire()?;
        let (reply, response) = oneshot::channel();
        self.send(RuntimeControlCommand::Leave {
            network_name,
            _operation: operation,
            reply,
        })
        .await?;
        response
            .await
            .map_err(|_| RuntimeControlError::ActorClosed)?
    }

    pub async fn update_routing(
        &self,
        command: NodeUpdateRoutingCommand,
    ) -> Result<ipc::RoutingUpdateResult, RuntimeControlError> {
        let (reply, response) = oneshot::channel();
        self.send(RuntimeControlCommand::UpdateRouting { command, reply })
            .await?;
        response
            .await
            .map_err(|_| RuntimeControlError::ActorClosed)?
    }

    pub async fn set_network_role(
        &self,
        command: NodeSetNetworkRoleCommand,
    ) -> Result<RoleUpdateResult, RuntimeControlError> {
        let (reply, response) = oneshot::channel();
        self.send(RuntimeControlCommand::SetNetworkRole { command, reply })
            .await?;
        response
            .await
            .map_err(|_| RuntimeControlError::ActorClosed)?
    }

    pub(crate) async fn set_local_exit_capability(
        &self,
        enabled: bool,
    ) -> Result<LocalExitCapabilitySnapshot, RuntimeControlError> {
        let operation = self.operation_gate.try_acquire()?;
        let (reply, response) = oneshot::channel();
        self.send(RuntimeControlCommand::SetLocalExitCapability {
            enabled,
            _operation: operation,
            reply,
        })
        .await?;
        response
            .await
            .map_err(|_| RuntimeControlError::ActorClosed)?
    }

    pub async fn submit_join_request(
        &self,
        request: JoinRequest,
        coordinator_url: Option<String>,
    ) -> Result<JoinRequestStatusSnapshot, RuntimeControlError> {
        match self
            .execute_join_request(JoinRequestRuntimeCommand::Submit {
                request: Box::new(request),
                coordinator_url,
            })
            .await?
        {
            JoinRequestExecutionResult::Status(status) => Ok(status),
            _ => Err(RuntimeControlError::JoinRequest {
                code: NodeProblemCode::Internal,
                message_key: "join_request.error.unavailable",
                message: "审批控制返回了不匹配的提交响应".into(),
            }),
        }
    }

    pub async fn list_join_requests(
        &self,
        network_id: NetworkId,
        coordinator_url: Option<String>,
    ) -> Result<Vec<JoinRequest>, RuntimeControlError> {
        match self
            .execute_join_request(JoinRequestRuntimeCommand::List {
                network_id,
                coordinator_url,
            })
            .await?
        {
            JoinRequestExecutionResult::List(requests) => Ok(requests),
            _ => Err(RuntimeControlError::JoinRequest {
                code: NodeProblemCode::Internal,
                message_key: "join_request.error.unavailable",
                message: "审批控制返回了不匹配的列表响应".into(),
            }),
        }
    }

    pub async fn list_network_members(
        &self,
        network_id: NetworkId,
        coordinator_url: Option<String>,
    ) -> Result<NetworkMemberDirectory, RuntimeControlError> {
        match self
            .execute_join_request(JoinRequestRuntimeCommand::ListMembers {
                network_id,
                coordinator_url,
            })
            .await?
        {
            JoinRequestExecutionResult::Members(directory) => Ok(directory),
            _ => Err(RuntimeControlError::JoinRequest {
                code: NodeProblemCode::Internal,
                message_key: "join_request.error.unavailable",
                message: "审批控制返回了不匹配的成员目录响应".into(),
            }),
        }
    }

    pub async fn preview_member_removal(
        &self,
        request: MemberRemovalPreviewRequest,
        coordinator_url: Option<String>,
    ) -> Result<MemberRemovalPreview, RuntimeControlError> {
        match self
            .execute_join_request(JoinRequestRuntimeCommand::PreviewRemoval {
                request,
                coordinator_url,
            })
            .await?
        {
            JoinRequestExecutionResult::RemovalPreview(preview) => Ok(preview),
            _ => Err(RuntimeControlError::JoinRequest {
                code: NodeProblemCode::Internal,
                message_key: "join_request.error.unavailable",
                message: "审批控制返回了不匹配的移除预览响应".into(),
            }),
        }
    }

    pub async fn change_member_lifecycle(
        &self,
        request: MemberLifecycleChangeRequest,
        coordinator_url: Option<String>,
    ) -> Result<MemberLifecycleChange, RuntimeControlError> {
        match self
            .execute_join_request(JoinRequestRuntimeCommand::LifecycleChange {
                request,
                coordinator_url,
            })
            .await?
        {
            JoinRequestExecutionResult::LifecycleChanged(change) => Ok(change),
            _ => Err(RuntimeControlError::JoinRequest {
                code: NodeProblemCode::Internal,
                message_key: "join_request.error.unavailable",
                message: "审批控制返回了不匹配的生命周期变更响应".into(),
            }),
        }
    }

    pub async fn review_join_requests(
        &self,
        network_id: NetworkId,
        reviews: Vec<JoinRequestBatchReviewItem>,
    ) -> Result<Vec<JoinRequestReviewResult>, RuntimeControlError> {
        match self
            .execute_join_request(JoinRequestRuntimeCommand::Review {
                network_id,
                reviews,
            })
            .await?
        {
            JoinRequestExecutionResult::Review(reviews) => Ok(reviews),
            _ => Err(RuntimeControlError::JoinRequest {
                code: NodeProblemCode::Internal,
                message_key: "join_request.error.unavailable",
                message: "审批控制返回了不匹配的审批响应".into(),
            }),
        }
    }

    pub async fn withdraw_join_request(
        &self,
        network_id: NetworkId,
        request_id: JoinRequestId,
        coordinator_url: Option<String>,
    ) -> Result<JoinRequestStatusSnapshot, RuntimeControlError> {
        match self
            .execute_join_request(JoinRequestRuntimeCommand::Withdraw {
                network_id,
                request_id,
                coordinator_url,
            })
            .await?
        {
            JoinRequestExecutionResult::Status(status) => Ok(status),
            _ => Err(RuntimeControlError::JoinRequest {
                code: NodeProblemCode::Internal,
                message_key: "join_request.error.unavailable",
                message: "审批控制返回了不匹配的撤回响应".into(),
            }),
        }
    }

    pub async fn resubmit_join_request(
        &self,
        network_id: NetworkId,
        request_id: JoinRequestId,
        expected_revision: u64,
        reason: String,
        coordinator_url: Option<String>,
    ) -> Result<JoinRequestStatusSnapshot, RuntimeControlError> {
        match self
            .execute_join_request(JoinRequestRuntimeCommand::Resubmit {
                network_id,
                request_id,
                expected_revision,
                reason,
                coordinator_url,
            })
            .await?
        {
            JoinRequestExecutionResult::Status(status) => Ok(status),
            _ => Err(RuntimeControlError::JoinRequest {
                code: NodeProblemCode::Internal,
                message_key: "join_request.error.unavailable",
                message: "审批控制返回了不匹配的重申响应".into(),
            }),
        }
    }

    async fn execute_join_request(
        &self,
        command: JoinRequestRuntimeCommand,
    ) -> Result<JoinRequestExecutionResult, RuntimeControlError> {
        let (reply, response) = oneshot::channel();
        self.send(RuntimeControlCommand::JoinRequest { command, reply })
            .await?;
        response
            .await
            .map_err(|_| RuntimeControlError::ActorClosed)?
    }

    pub async fn create_invitation(
        &self,
        request: InvitationCreateRequest,
    ) -> Result<IssuedInvitation, RuntimeControlError> {
        match self
            .execute_invitation(InvitationRuntimeCommand::Create(request))
            .await?
        {
            InvitationExecutionResult::Created(issued) => Ok(issued),
            _ => Err(invitation_unavailable("邀请控制返回了不匹配的创建响应")),
        }
    }

    pub async fn list_invitations(
        &self,
        network_id: NetworkId,
    ) -> Result<Vec<InvitationStatusView>, RuntimeControlError> {
        match self
            .execute_invitation(InvitationRuntimeCommand::List { network_id })
            .await?
        {
            InvitationExecutionResult::List(invitations) => Ok(invitations),
            _ => Err(invitation_unavailable("邀请控制返回了不匹配的列表响应")),
        }
    }

    pub async fn get_invitation_status(
        &self,
        network_id: NetworkId,
        invitation_id: InvitationId,
    ) -> Result<InvitationStatusView, RuntimeControlError> {
        match self
            .execute_invitation(InvitationRuntimeCommand::Status {
                network_id,
                invitation_id,
            })
            .await?
        {
            InvitationExecutionResult::Status(invitation) => Ok(invitation),
            _ => Err(invitation_unavailable("邀请控制返回了不匹配的状态响应")),
        }
    }

    pub async fn revoke_invitation(
        &self,
        network_id: NetworkId,
        invitation_id: InvitationId,
        expected_revision: u64,
    ) -> Result<InvitationStatusView, RuntimeControlError> {
        match self
            .execute_invitation(InvitationRuntimeCommand::Revoke {
                network_id,
                invitation_id,
                expected_revision,
            })
            .await?
        {
            InvitationExecutionResult::Revoked(invitation) => Ok(invitation),
            _ => Err(invitation_unavailable("邀请控制返回了不匹配的撤销响应")),
        }
    }

    pub async fn preview_invitation(
        &self,
        network_id: NetworkId,
        coordinator_url: String,
        code: InvitationCode,
    ) -> Result<InvitationPreview, RuntimeControlError> {
        match self
            .execute_invitation(InvitationRuntimeCommand::Preview {
                network_id,
                coordinator_url,
                code,
            })
            .await?
        {
            InvitationExecutionResult::Preview(preview) => Ok(preview),
            _ => Err(invitation_unavailable("邀请控制返回了不匹配的预览响应")),
        }
    }

    pub async fn redeem_invitation(
        &self,
        coordinator_url: String,
        request: InvitationRedeemRequest,
    ) -> Result<InvitationRedemption, RuntimeControlError> {
        match self
            .execute_invitation(InvitationRuntimeCommand::Redeem {
                coordinator_url,
                request,
            })
            .await?
        {
            InvitationExecutionResult::Redeemed(redemption) => Ok(redemption),
            _ => Err(invitation_unavailable("邀请控制返回了不匹配的兑换响应")),
        }
    }

    async fn execute_invitation(
        &self,
        command: InvitationRuntimeCommand,
    ) -> Result<InvitationExecutionResult, RuntimeControlError> {
        let (reply, response) = oneshot::channel();
        self.send(RuntimeControlCommand::Invitation { command, reply })
            .await?;
        response
            .await
            .map_err(|_| RuntimeControlError::ActorClosed)?
    }

    pub async fn get_coordinator_candidates(
        &self,
        network_name: String,
    ) -> Result<CoordinatorCandidateStatus, RuntimeControlError> {
        self.execute_coordinator_candidates(CoordinatorCandidateRuntimeCommand::Status {
            network_name,
        })
        .await
    }

    pub async fn update_coordinator_candidates(
        &self,
        command: NodeUpdateCoordinatorCandidatesCommand,
    ) -> Result<CoordinatorCandidateStatus, RuntimeControlError> {
        self.execute_coordinator_candidates(CoordinatorCandidateRuntimeCommand::UpdateConfig(
            command,
        ))
        .await
    }

    pub async fn publish_coordinator_candidate_declaration(
        &self,
        command: NodePublishCoordinatorCandidateDeclarationCommand,
    ) -> Result<CoordinatorCandidateStatus, RuntimeControlError> {
        self.execute_coordinator_candidates(CoordinatorCandidateRuntimeCommand::PublishDeclaration(
            command,
        ))
        .await
    }

    /// Internal election/handoff entry point. This is intentionally not a
    /// user-facing NodeCommand: W14-03 must supply an authenticated majority
    /// proof before invoking it.
    #[allow(dead_code)]
    pub async fn install_coordinator_term(
        &self,
        network_name: String,
        term: CoordinatorTerm,
    ) -> Result<CoordinatorCandidateStatus, RuntimeControlError> {
        self.execute_coordinator_candidates(CoordinatorCandidateRuntimeCommand::InstallTerm {
            network_name,
            term,
        })
        .await
    }

    /// Renew the current Leader lease without changing term or fencing token.
    #[allow(dead_code)]
    pub async fn renew_coordinator_term_lease(
        &self,
        network_name: String,
        credential: CoordinatorWriteCredential,
        new_expires_at_unix_ms: u64,
    ) -> Result<CoordinatorCandidateStatus, RuntimeControlError> {
        self.execute_coordinator_candidates(CoordinatorCandidateRuntimeCommand::RenewTermLease {
            network_name,
            credential,
            new_expires_at_unix_ms,
        })
        .await
    }

    async fn execute_coordinator_candidates(
        &self,
        command: CoordinatorCandidateRuntimeCommand,
    ) -> Result<CoordinatorCandidateStatus, RuntimeControlError> {
        let (reply, response) = oneshot::channel();
        self.send(RuntimeControlCommand::CoordinatorCandidates { command, reply })
            .await?;
        response
            .await
            .map_err(|_| RuntimeControlError::ActorClosed)?
    }

    pub async fn submit_admin_recovery(
        &self,
        command: NodeSubmitAdminRecoveryCommand,
    ) -> Result<AdminRecoveryStatusView, RuntimeControlError> {
        match self
            .send_admin_recovery(AdminRecoveryRuntimeCommand::Submit(command))
            .await?
        {
            AdminRecoveryExecutionResult::Status(status) => Ok(status),
            AdminRecoveryExecutionResult::List(_) => Err(admin_recovery_unavailable(
                "管理员恢复提交返回了不匹配的列表响应",
            )),
        }
    }

    pub async fn get_admin_recovery_status(
        &self,
        command: NodeAdminRecoveryStatusCommand,
    ) -> Result<AdminRecoveryStatusView, RuntimeControlError> {
        match self
            .send_admin_recovery(AdminRecoveryRuntimeCommand::Status(command))
            .await?
        {
            AdminRecoveryExecutionResult::Status(status) => Ok(status),
            AdminRecoveryExecutionResult::List(_) => Err(admin_recovery_unavailable(
                "管理员恢复状态查询返回了不匹配的列表响应",
            )),
        }
    }

    pub async fn list_admin_recoveries(
        &self,
        command: NodeAdminRecoveryListCommand,
    ) -> Result<Vec<AdminRecoveryStatusView>, RuntimeControlError> {
        match self
            .send_admin_recovery(AdminRecoveryRuntimeCommand::List(command))
            .await?
        {
            AdminRecoveryExecutionResult::List(statuses) => Ok(statuses),
            AdminRecoveryExecutionResult::Status(_) => Err(admin_recovery_unavailable(
                "管理员恢复列表返回了不匹配的单项响应",
            )),
        }
    }

    pub async fn revoke_admin_recovery(
        &self,
        command: NodeRevokeAdminRecoveryCommand,
    ) -> Result<AdminRecoveryStatusView, RuntimeControlError> {
        match self
            .send_admin_recovery(AdminRecoveryRuntimeCommand::Revoke(command))
            .await?
        {
            AdminRecoveryExecutionResult::Status(status) => Ok(status),
            AdminRecoveryExecutionResult::List(_) => Err(admin_recovery_unavailable(
                "管理员恢复撤销返回了不匹配的列表响应",
            )),
        }
    }

    pub async fn execute_admin_recovery(
        &self,
        command: NodeExecuteAdminRecoveryCommand,
    ) -> Result<AdminRecoveryStatusView, RuntimeControlError> {
        match self
            .send_admin_recovery(AdminRecoveryRuntimeCommand::Execute(command))
            .await?
        {
            AdminRecoveryExecutionResult::Status(status) => Ok(status),
            AdminRecoveryExecutionResult::List(_) => Err(admin_recovery_unavailable(
                "管理员恢复执行返回了不匹配的列表响应",
            )),
        }
    }

    async fn send_admin_recovery(
        &self,
        command: AdminRecoveryRuntimeCommand,
    ) -> Result<AdminRecoveryExecutionResult, RuntimeControlError> {
        let (reply, response) = oneshot::channel();
        self.send(RuntimeControlCommand::AdminRecovery { command, reply })
            .await?;
        response
            .await
            .map_err(|_| RuntimeControlError::ActorClosed)?
    }

    pub async fn stop(
        &self,
        networks: Vec<String>,
        all: bool,
    ) -> Result<StopSummary, RuntimeControlError> {
        let operation = self.operation_gate.try_acquire()?;
        let (reply, response) = oneshot::channel();
        self.send(RuntimeControlCommand::Stop {
            networks,
            all,
            _operation: operation,
            reply,
        })
        .await?;
        response
            .await
            .map_err(|_| RuntimeControlError::ActorClosed)?
    }

    pub async fn restart(
        &self,
        networks: Vec<String>,
        all: bool,
    ) -> Result<RestartSummary, RuntimeControlError> {
        let operation = self.operation_gate.try_acquire()?;
        let (reply, response) = oneshot::channel();
        self.send(RuntimeControlCommand::Restart {
            networks,
            all,
            _operation: operation,
            reply,
        })
        .await?;
        response
            .await
            .map_err(|_| RuntimeControlError::ActorClosed)?
    }

    pub async fn rebuild_tun(
        &self,
        network_name: String,
    ) -> Result<RestartSummary, RuntimeControlError> {
        let operation = self.operation_gate.try_acquire()?;
        let (reply, response) = oneshot::channel();
        self.send(RuntimeControlCommand::RebuildTun {
            network_name,
            _operation: operation,
            reply,
        })
        .await?;
        response
            .await
            .map_err(|_| RuntimeControlError::ActorClosed)?
    }

    /// Restore every persisted network while retaining the node IPC process.
    pub async fn start_node(&self) -> Result<StartSummary, RuntimeControlError> {
        let operation = self.operation_gate.try_acquire()?;
        let (reply, response) = oneshot::channel();
        self.send(RuntimeControlCommand::StartNode {
            _operation: operation,
            reply,
        })
        .await?;
        response
            .await
            .map_err(|_| RuntimeControlError::ActorClosed)?
    }

    /// Stop every data-plane session and owned embedded service while retaining
    /// the node IPC process so a later start can restore persisted networks.
    pub async fn stop_node(&self) -> Result<StopSummary, RuntimeControlError> {
        let operation = self.operation_gate.try_acquire()?;
        let (reply, response) = oneshot::channel();
        self.send(RuntimeControlCommand::StopNode {
            _operation: operation,
            reply,
        })
        .await?;
        response
            .await
            .map_err(|_| RuntimeControlError::ActorClosed)?
    }

    /// Replace the complete data plane from persisted configuration under one
    /// operation permit and one restart deadline.
    pub async fn restart_node(&self) -> Result<RestartSummary, RuntimeControlError> {
        let operation = self.operation_gate.try_acquire()?;
        let (reply, response) = oneshot::channel();
        self.send(RuntimeControlCommand::RestartNode {
            _operation: operation,
            reply,
        })
        .await?;
        response
            .await
            .map_err(|_| RuntimeControlError::ActorClosed)?
    }

    pub async fn cleanup_residuals(
        &self,
        networks: Vec<String>,
        all: bool,
    ) -> Result<StopSummary, RuntimeControlError> {
        let operation = self.operation_gate.try_acquire()?;
        let (reply, response) = oneshot::channel();
        self.send(RuntimeControlCommand::CleanupResiduals {
            networks,
            all,
            _operation: operation,
            reply,
        })
        .await?;
        response
            .await
            .map_err(|_| RuntimeControlError::ActorClosed)?
    }

    /// Return the exact network-to-TUN mapping owned by live runtime sessions.
    /// This is authoritative for classifying product interfaces.
    pub async fn live_tun_names(&self) -> Result<BTreeMap<String, String>, RuntimeControlError> {
        let (reply, response) = oneshot::channel();
        self.send(RuntimeControlCommand::LiveTunNames { reply })
            .await?;
        response
            .await
            .map_err(|_| RuntimeControlError::ActorClosed)?
    }

    pub async fn diagnostics(&self) -> Result<NodeDiagnosticsSnapshot, RuntimeControlError> {
        let (_cancel_sender, cancel) = watch::channel(false);
        self.diagnostics_cancellable(cancel).await
    }

    pub async fn diagnostics_cancellable(
        &self,
        cancel: watch::Receiver<bool>,
    ) -> Result<NodeDiagnosticsSnapshot, RuntimeControlError> {
        let (reply, response) = oneshot::channel();
        self.send(RuntimeControlCommand::Diagnostics { cancel, reply })
            .await?;
        response
            .await
            .map_err(|_| RuntimeControlError::ActorClosed)?
    }

    pub async fn shutdown(&self) -> Result<StopSummary, RuntimeControlError> {
        let operation = self.operation_gate.try_acquire()?;
        let (reply, response) = oneshot::channel();
        self.send(RuntimeControlCommand::Shutdown {
            _operation: operation,
            reply,
        })
        .await?;
        response
            .await
            .map_err(|_| RuntimeControlError::ActorClosed)?
    }

    async fn send(&self, command: RuntimeControlCommand) -> Result<(), RuntimeControlError> {
        self.sender
            .send(command)
            .await
            .map_err(|_| RuntimeControlError::ActorClosed)
    }
}

async fn run_actor(
    mut receiver: mpsc::Receiver<RuntimeControlCommand>,
    mut actor: NetworkRuntimeActor,
) {
    actor.refresh_local_exit_capability_state().await;
    let mut refresh = tokio::time::interval(actor.timeouts.local_exit_refresh);
    refresh.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    refresh.tick().await;

    let coordinator_takeover_seed = actor
        .configs
        .load_node_identity()
        .map(|identity| {
            let digest = blake3::hash(&identity.public_key());
            u64::from_le_bytes(
                digest.as_bytes()[..8]
                    .try_into()
                    .expect("blake3 digest contains at least eight bytes"),
            )
        })
        .unwrap_or(0x5741_3443_4F4F_5244);
    let mut takeover_backoff = CoordinatorTakeoverPollBackoff::new(coordinator_takeover_seed);
    let mut takeover_deadline = Instant::now() + takeover_backoff.delay();
    let mut coordinator_takeover_wakeups = actor.coordinator_takeover_wakeups.take();
    loop {
        tokio::select! {
            command = receiver.recv() => {
                let Some(command) = command else {
                    break;
                };
                match command {
                    RuntimeControlCommand::Diagnostics { cancel, reply } => {
                        let result = actor.diagnostics_cancellable(cancel).await;
                        let _ = reply.send(result);
                    }
                    RuntimeControlCommand::LiveTunNames { reply } => {
                        let result = actor.live_tun_names().await;
                        let _ = reply.send(result);
                    }
                    RuntimeControlCommand::StartPlan {
                        plan,
                        _operation,
                        reply,
                    } => {
                        let result = actor.start_plan(plan).await;
                        let _ = reply.send(result);
                    }
                    RuntimeControlCommand::StartSelection {
                        selection,
                        _operation,
                        reply,
                    } => {
                        let result = actor.start_selection(selection).await;
                        let _ = reply.send(result);
                    }
                    RuntimeControlCommand::Create {
                        command,
                        _operation,
                        reply,
                    } => {
                        let result = actor.create(command).await;
                        let _ = reply.send(result);
                    }
                    RuntimeControlCommand::Join {
                        command,
                        _operation,
                        reply,
                    } => {
                        let result = actor.join(command).await;
                        let _ = reply.send(result);
                    }
                    RuntimeControlCommand::Leave {
                        network_name,
                        _operation,
                        reply,
                    } => {
                        let result = actor.leave(network_name).await;
                        let _ = reply.send(result);
                    }
                    RuntimeControlCommand::UpdateRouting { command, reply } => {
                        let result = actor.update_routing(command).await;
                        let _ = reply.send(result);
                    }
                    RuntimeControlCommand::SetNetworkRole { command, reply } => {
                        let result = actor.set_network_role(command).await;
                        let _ = reply.send(result);
                    }
                    RuntimeControlCommand::SetLocalExitCapability {
                        enabled,
                        _operation,
                        reply,
                    } => {
                        let result = actor.set_local_exit_capability(enabled).await;
                        let _ = reply.send(result);
                    }
                    RuntimeControlCommand::JoinRequest { command, reply } => {
                        let result = actor.execute_join_request_command(command).await;
                        let _ = reply.send(result);
                    }
                    RuntimeControlCommand::Invitation { command, reply } => {
                        let result = actor.execute_invitation_command(command).await;
                        let _ = reply.send(result);
                    }
                    RuntimeControlCommand::Stop {
                        networks,
                        all,
                        _operation,
                        reply,
                    } => {
                        let result = actor.stop(networks, all).await;
                        let _ = reply.send(result);
                    }
                    RuntimeControlCommand::Restart {
                        networks,
                        all,
                        _operation,
                        reply,
                    } => {
                        let result = actor.restart(networks, all).await;
                        let _ = reply.send(result);
                    }
                    RuntimeControlCommand::RebuildTun {
                        network_name,
                        _operation,
                        reply,
                    } => {
                        let result = actor.rebuild_tun(network_name).await;
                        let _ = reply.send(result);
                    }
                    RuntimeControlCommand::StartNode { _operation, reply } => {
                        let result = actor.start_node().await;
                        let _ = reply.send(result);
                    }
                    RuntimeControlCommand::StopNode { _operation, reply } => {
                        let result = actor.stop_node().await;
                        let _ = reply.send(result);
                    }
                    RuntimeControlCommand::RestartNode { _operation, reply } => {
                        let result = actor.restart_node().await;
                        let _ = reply.send(result);
                    }
                    RuntimeControlCommand::CleanupResiduals {
                        networks,
                        all,
                        _operation,
                        reply,
                    } => {
                        let result = actor.cleanup_residuals(networks, all).await;
                        let _ = reply.send(result);
                    }
                    RuntimeControlCommand::CoordinatorCandidates { command, reply } => {
                        let result = actor.execute_coordinator_candidate_command(command).await;
                        let _ = reply.send(result);
                    }
                    RuntimeControlCommand::AdminRecovery { command, reply } => {
                        let result = actor.execute_admin_recovery_command(command).await;
                        let _ = reply.send(result);
                    }
                    RuntimeControlCommand::Shutdown { _operation, reply } => {
                        let result = actor.shutdown().await;
                        let _ = reply.send(result);
                        break;
                    }
                }
            }
            _ = refresh.tick() => {
                actor.refresh_local_exit_client_counts().await;
            }
            _ = sleep_until(takeover_deadline) => {
                if matches!(
                    actor.refresh_coordinator_takeover().await,
                    CoordinatorTakeoverRefreshOutcome::Failed
                ) {
                    takeover_backoff.record_failure();
                } else {
                    takeover_backoff.record_success();
                }
                takeover_deadline = Instant::now() + takeover_backoff.delay();
            }
            received = async {
                match coordinator_takeover_wakeups.as_mut() {
                    Some(wakeups) => wakeups.recv().await.is_some(),
                    None => std::future::pending::<bool>().await,
                }
            }, if coordinator_takeover_wakeups.is_some() => {
                if received {
                    takeover_backoff.record_success();
                    if matches!(
                        actor.refresh_coordinator_takeover().await,
                        CoordinatorTakeoverRefreshOutcome::Failed
                    ) {
                        takeover_backoff.record_failure();
                    }
                    takeover_deadline = Instant::now() + takeover_backoff.delay();
                } else {
                    coordinator_takeover_wakeups = None;
                }
            }
        }
    }
}

struct NetworkRuntimeActor {
    configs: ConfigManager,
    options: RuntimeOptions,
    state: NodeStateStore,
    events: NodeEventBus,
    lifecycle: NodeLifecycle,
    registry: MultiNetworkRegistry,
    services: EmbeddedServiceManager,
    launcher: Arc<dyn NetworkRuntimeLauncher>,
    sessions: HashMap<String, NetworkRuntimeSession>,
    retained_owners: HashMap<String, Box<dyn ResidualRuntimeOwner>>,
    cleanup_failures: BTreeMap<String, RuntimeCleanupOutcome>,
    completed_cleanup: BTreeMap<String, RuntimeCleanupOutcome>,
    timeouts: RuntimeTimeouts,
    coordinator_takeover_wakeups: Option<mpsc::UnboundedReceiver<()>>,
}

impl NetworkRuntimeActor {
    async fn local_exit_owner_intent(&self) -> Result<bool, RuntimeControlError> {
        self.configs.node_local_exit_enabled().map_err(|error| {
            RuntimeControlError::Persistence(format!("读取本机 Exit owner intent 失败: {error:#}"))
        })
    }

    async fn publish_local_exit_capability(
        &self,
        capability: LocalExitCapabilitySnapshot,
    ) -> LocalExitCapabilitySnapshot {
        let capability = LocalExitCapabilitySnapshot {
            last_transition_at_unix_ms: unix_time_ms(),
            ..capability
        };
        self.state
            .update(|snapshot| snapshot.local_exit = capability.clone())
            .await;
        capability
    }

    async fn set_local_exit_transition(&self, stage: LocalExitCapabilityStage) {
        self.state
            .update(|snapshot| {
                snapshot.local_exit.stage = stage;
                snapshot.local_exit.healthy = false;
                snapshot.local_exit.problem = None;
                snapshot.local_exit.last_transition_at_unix_ms = unix_time_ms();
            })
            .await;
    }

    async fn set_local_exit_post_request_transition(
        &self,
        stage: LocalExitCapabilityStage,
        owner_exit_enabled: bool,
        statuses: &[ipc::LocalExitRuntimeStatus],
    ) {
        let network_allowed = statuses.iter().any(|status| status.network_allowed);
        let active_clients = statuses.iter().fold(0u32, |total, status| {
            total.saturating_add(status.active_clients)
        });
        let total_clients = statuses.iter().fold(0u64, |total, status| {
            total.saturating_add(status.total_clients)
        });
        self.publish_local_exit_capability(LocalExitCapabilitySnapshot {
            owner_exit_enabled,
            stage,
            healthy: false,
            network_allowed,
            active_clients,
            total_clients,
            problem: None,
            last_transition_at_unix_ms: 0,
        })
        .await;
    }

    async fn request_all_local_exit_capabilities(
        &self,
        enabled: bool,
    ) -> Vec<(
        String,
        Result<ipc::LocalExitRuntimeStatus, RuntimeRoutingRequestError>,
    )> {
        self.request_all_local_exit_capabilities_with_wait(enabled, self.timeouts.local_exit_update)
            .await
    }

    async fn request_all_local_exit_capabilities_with_wait(
        &self,
        enabled: bool,
        wait: Duration,
    ) -> Vec<(
        String,
        Result<ipc::LocalExitRuntimeStatus, RuntimeRoutingRequestError>,
    )> {
        let requests = self
            .sessions
            .iter()
            .map(|(network_name, session)| {
                let network_name = network_name.clone();
                let routing_control = session.routing_control.clone();
                async move {
                    let result = request_runtime_local_exit(&routing_control, enabled, wait).await;
                    (network_name, result)
                }
            })
            .collect::<Vec<_>>();
        join_all(requests).await
    }

    async fn request_all_local_exit_statuses(
        &self,
    ) -> Vec<(
        String,
        Result<ipc::LocalExitRuntimeStatus, RuntimeRoutingRequestError>,
    )> {
        self.request_all_local_exit_statuses_with_wait(self.timeouts.local_exit_update)
            .await
    }

    async fn request_all_local_exit_statuses_with_wait(
        &self,
        wait: Duration,
    ) -> Vec<(
        String,
        Result<ipc::LocalExitRuntimeStatus, RuntimeRoutingRequestError>,
    )> {
        let requests = self
            .sessions
            .iter()
            .map(|(network_name, session)| {
                let network_name = network_name.clone();
                let routing_control = session.routing_control.clone();
                async move {
                    let result = request_runtime_local_exit_status(&routing_control, wait).await;
                    (network_name, result)
                }
            })
            .collect::<Vec<_>>();
        join_all(requests).await
    }

    /// Client authorization happens inside a network runtime after its first
    /// Exit packet. Refresh only the observed counters here; lifecycle and
    /// owner intent remain owned by the command path and must not be inferred
    /// from a periodic observation.
    async fn refresh_local_exit_client_counts(&self) {
        if self.sessions.is_empty() {
            return;
        }

        let results = self
            .request_all_local_exit_statuses_with_wait(LOCAL_EXIT_STATUS_REFRESH_TIMEOUT)
            .await;
        if let Some((network_name, error)) = results.iter().find_map(|(network_name, result)| {
            result
                .as_ref()
                .err()
                .map(|error| (network_name, describe_routing_request_failure(error)))
        }) {
            tracing::warn!(
                network_name,
                error,
                "刷新本机 Exit 客户端计数失败，保留上一次已确认快照"
            );
            return;
        }

        let active_clients = results.iter().fold(0u32, |total, (_, result)| {
            result
                .as_ref()
                .map_or(total, |status| total.saturating_add(status.active_clients))
        });
        let total_clients = results.iter().fold(0u64, |total, (_, result)| {
            result
                .as_ref()
                .map_or(total, |status| total.saturating_add(status.total_clients))
        });
        self.state
            .update_if_changed(|snapshot| {
                if snapshot.local_exit.owner_exit_enabled {
                    snapshot.local_exit.active_clients = active_clients;
                    snapshot.local_exit.total_clients = total_clients;
                }
            })
            .await;
    }

    async fn wait_for_local_exit_cleanup(
        &self,
        mut results: Vec<(
            String,
            Result<ipc::LocalExitRuntimeStatus, RuntimeRoutingRequestError>,
        )>,
    ) -> Vec<(
        String,
        Result<ipc::LocalExitRuntimeStatus, RuntimeRoutingRequestError>,
    )> {
        let deadline = Instant::now() + LOCAL_EXIT_DRAIN_OBSERVATION_TIMEOUT;
        loop {
            if !local_exit_results_are_draining(&results) || Instant::now() >= deadline {
                return results;
            }
            sleep(LOCAL_EXIT_DRAIN_POLL_INTERVAL).await;
            results = self.request_all_local_exit_statuses().await;
        }
    }

    async fn refresh_local_exit_capability_state(&self) {
        self.refresh_local_exit_capability_state_with_wait(self.timeouts.local_exit_update)
            .await;
    }

    async fn refresh_local_exit_capability_state_with_deadline(&self, deadline: Instant) {
        let wait = deadline.saturating_duration_since(Instant::now());
        if wait.is_zero() {
            return;
        }
        self.refresh_local_exit_capability_state_with_wait(wait)
            .await;
    }

    async fn refresh_local_exit_capability_state_with_wait(&self, wait: Duration) {
        if wait.is_zero() {
            return;
        }
        let owner_exit_enabled = match self.local_exit_owner_intent().await {
            Ok(enabled) => enabled,
            Err(error) => {
                let problem = runtime_control_problem(&error);
                let current = self.state.snapshot().await.local_exit;
                self.publish_local_exit_capability(LocalExitCapabilitySnapshot {
                    owner_exit_enabled: current.owner_exit_enabled,
                    stage: LocalExitCapabilityStage::Failed,
                    problem: Some(problem),
                    ..LocalExitCapabilitySnapshot::default()
                })
                .await;
                return;
            }
        };

        let capabilities = self.state.snapshot().await.exit_node_capabilities;
        let unsupported_problem = owner_exit_enabled
            .then(|| local_exit_unsupported_error(&capabilities))
            .flatten()
            .map(|error| runtime_control_problem(&error));
        let results = self
            .request_all_local_exit_capabilities_with_wait(owner_exit_enabled, wait)
            .await;
        let (statuses, request_problem) = local_exit_request_results(results);
        let problem = unsupported_problem.or(request_problem);
        self.publish_local_exit_capability(local_exit_capability_from_statuses(
            owner_exit_enabled,
            &statuses,
            problem,
        ))
        .await;
    }

    async fn set_local_exit_capability(
        &mut self,
        enabled: bool,
    ) -> Result<LocalExitCapabilitySnapshot, RuntimeControlError> {
        self.reconcile_finished_sessions().await;

        let previous = self.local_exit_owner_intent().await?;
        if enabled {
            let capabilities = self.state.snapshot().await.exit_node_capabilities;
            if let Some(error) = local_exit_unsupported_error(&capabilities) {
                let problem = runtime_control_problem(&error);
                self.publish_local_exit_capability(local_exit_capability_from_statuses(
                    previous,
                    &[],
                    Some(problem),
                ))
                .await;
                return Err(error);
            }
        }

        self.set_local_exit_transition(if enabled {
            LocalExitCapabilityStage::Listening
        } else {
            LocalExitCapabilityStage::Draining
        })
        .await;

        let results = self.request_all_local_exit_capabilities(enabled).await;
        let failed = results.iter().find_map(|(network_name, result)| {
            result
                .as_ref()
                .err()
                .map(|error| local_exit_request_error(network_name, error))
        });

        if let Some(error) = failed {
            let rollback_results = self.request_all_local_exit_capabilities(previous).await;
            let (statuses, rollback_problem) = local_exit_request_results(rollback_results);
            let mut problem = runtime_control_problem(&error);
            if let Some(rollback_problem) = rollback_problem {
                problem.message.push_str(&format!(
                    "；部分网络的 owner intent 回滚未能确认: {}",
                    rollback_problem.message
                ));
                problem.retryable = true;
            }
            self.publish_local_exit_capability(local_exit_capability_from_statuses(
                previous,
                &statuses,
                Some(problem),
            ))
            .await;
            return Err(error);
        }

        let requested_statuses = results
            .iter()
            .filter_map(|(_, result)| result.as_ref().ok().cloned())
            .collect::<Vec<_>>();
        if enabled
            && requested_statuses
                .iter()
                .any(|status| status.network_allowed)
        {
            self.set_local_exit_post_request_transition(
                LocalExitCapabilityStage::HealthChecking,
                enabled,
                &requested_statuses,
            )
            .await;
        } else if !enabled {
            self.set_local_exit_post_request_transition(
                LocalExitCapabilityStage::Cleaning,
                enabled,
                &requested_statuses,
            )
            .await;
        }

        if let Err(error) = self.configs.set_node_local_exit_enabled(enabled) {
            let rollback_results = self.request_all_local_exit_capabilities(previous).await;
            let (statuses, rollback_problem) = local_exit_request_results(rollback_results);
            let persistence_error = RuntimeControlError::Persistence(format!(
                "保存本机 Exit owner intent 失败: {error:#}"
            ));
            let mut problem = runtime_control_problem(&persistence_error);
            if let Some(rollback_problem) = rollback_problem {
                problem.message.push_str(&format!(
                    "；运行时 owner intent 回滚未能确认: {}",
                    rollback_problem.message
                ));
            }
            self.publish_local_exit_capability(local_exit_capability_from_statuses(
                previous,
                &statuses,
                Some(problem),
            ))
            .await;
            return Err(persistence_error);
        }

        let results = if !enabled && local_exit_results_are_draining(&results) {
            self.wait_for_local_exit_cleanup(results).await
        } else {
            results
        };
        let (statuses, cleanup_problem) = local_exit_request_results(results);
        Ok(self
            .publish_local_exit_capability(local_exit_capability_from_statuses(
                enabled,
                &statuses,
                cleanup_problem,
            ))
            .await)
    }

    async fn live_tun_names(&mut self) -> Result<BTreeMap<String, String>, RuntimeControlError> {
        self.reconcile_finished_sessions().await;
        Ok(self
            .sessions
            .iter()
            .map(|(network_name, session)| (network_name.clone(), session.tun_name.clone()))
            .collect())
    }

    async fn execute_join_request_command(
        &mut self,
        command: JoinRequestRuntimeCommand,
    ) -> Result<JoinRequestExecutionResult, RuntimeControlError> {
        match command {
            JoinRequestRuntimeCommand::Submit {
                request,
                coordinator_url,
            } => {
                let network_id = request.network_id.clone();
                let (signaling, ephemeral) = self
                    .join_request_signaling(&network_id, coordinator_url.as_deref(), true)
                    .await?;
                let result = signaling.submit_join_request(*request).await;
                self.close_ephemeral_join_request_signaling(&signaling, ephemeral)
                    .await;
                result
                    .map(JoinRequestExecutionResult::Status)
                    .map_err(map_join_request_error)
            }
            JoinRequestRuntimeCommand::List {
                network_id,
                coordinator_url,
            } => {
                let (signaling, ephemeral) = self
                    .join_request_signaling(&network_id, coordinator_url.as_deref(), true)
                    .await?;
                let result = signaling.list_join_requests(network_id).await;
                self.close_ephemeral_join_request_signaling(&signaling, ephemeral)
                    .await;
                result
                    .map(JoinRequestExecutionResult::List)
                    .map_err(map_join_request_error)
            }
            JoinRequestRuntimeCommand::ListMembers {
                network_id,
                coordinator_url,
            } => {
                let (signaling, ephemeral) = self
                    .join_request_signaling(&network_id, coordinator_url.as_deref(), true)
                    .await?;
                let result = signaling.list_network_members(network_id).await;
                self.close_ephemeral_join_request_signaling(&signaling, ephemeral)
                    .await;
                result
                    .map(JoinRequestExecutionResult::Members)
                    .map_err(map_join_request_error)
            }
            JoinRequestRuntimeCommand::PreviewRemoval {
                request,
                coordinator_url,
            } => {
                let network_id = request.network_id.clone();
                let (signaling, ephemeral) = self
                    .join_request_signaling(&network_id, coordinator_url.as_deref(), true)
                    .await?;
                let result = signaling.preview_member_removal(request).await;
                self.close_ephemeral_join_request_signaling(&signaling, ephemeral)
                    .await;
                result
                    .map(JoinRequestExecutionResult::RemovalPreview)
                    .map_err(map_join_request_error)
            }
            JoinRequestRuntimeCommand::LifecycleChange {
                request,
                coordinator_url,
            } => {
                let network_id = request.network_id.clone();
                let (signaling, ephemeral) = self
                    .join_request_signaling(&network_id, coordinator_url.as_deref(), true)
                    .await?;
                let result = signaling.change_member_lifecycle(request).await;
                self.close_ephemeral_join_request_signaling(&signaling, ephemeral)
                    .await;
                result
                    .map(JoinRequestExecutionResult::LifecycleChanged)
                    .map_err(map_join_request_error)
            }
            JoinRequestRuntimeCommand::Review {
                network_id,
                reviews,
            } => {
                let (signaling, ephemeral) = self
                    .join_request_signaling(&network_id, None, false)
                    .await?;
                debug_assert!(!ephemeral);
                signaling
                    .review_join_requests(network_id, reviews)
                    .await
                    .map(JoinRequestExecutionResult::Review)
                    .map_err(map_join_request_error)
            }
            JoinRequestRuntimeCommand::Withdraw {
                network_id,
                request_id,
                coordinator_url,
            } => {
                let (signaling, ephemeral) = self
                    .join_request_signaling(&network_id, coordinator_url.as_deref(), true)
                    .await?;
                let result = signaling
                    .withdraw_join_request(network_id, request_id)
                    .await;
                self.close_ephemeral_join_request_signaling(&signaling, ephemeral)
                    .await;
                result
                    .map(JoinRequestExecutionResult::Status)
                    .map_err(map_join_request_error)
            }
            JoinRequestRuntimeCommand::Resubmit {
                network_id,
                request_id,
                expected_revision,
                reason,
                coordinator_url,
            } => {
                let (signaling, ephemeral) = self
                    .join_request_signaling(&network_id, coordinator_url.as_deref(), true)
                    .await?;
                let result = signaling
                    .resubmit_join_request(network_id, request_id, expected_revision, reason)
                    .await;
                self.close_ephemeral_join_request_signaling(&signaling, ephemeral)
                    .await;
                result
                    .map(JoinRequestExecutionResult::Status)
                    .map_err(map_join_request_error)
            }
        }
    }

    async fn execute_invitation_command(
        &mut self,
        command: InvitationRuntimeCommand,
    ) -> Result<InvitationExecutionResult, RuntimeControlError> {
        match command {
            InvitationRuntimeCommand::Create(request) => {
                let network_id = request.network_id.clone();
                let (signaling, ephemeral) = self.invitation_signaling(&network_id, None).await?;
                let result = signaling.create_invitation(request).await;
                self.close_ephemeral_invitation_signaling(&signaling, ephemeral)
                    .await;
                result
                    .map(InvitationExecutionResult::Created)
                    .map_err(map_invitation_error)
            }
            InvitationRuntimeCommand::List { network_id } => {
                let (signaling, ephemeral) = self.invitation_signaling(&network_id, None).await?;
                let result = signaling.list_invitations(network_id).await;
                self.close_ephemeral_invitation_signaling(&signaling, ephemeral)
                    .await;
                result
                    .map(InvitationExecutionResult::List)
                    .map_err(map_invitation_error)
            }
            InvitationRuntimeCommand::Status {
                network_id,
                invitation_id,
            } => {
                let (signaling, ephemeral) = self.invitation_signaling(&network_id, None).await?;
                let result = signaling
                    .get_invitation_status(network_id, invitation_id)
                    .await;
                self.close_ephemeral_invitation_signaling(&signaling, ephemeral)
                    .await;
                result
                    .map(InvitationExecutionResult::Status)
                    .map_err(map_invitation_error)
            }
            InvitationRuntimeCommand::Revoke {
                network_id,
                invitation_id,
                expected_revision,
            } => {
                let (signaling, ephemeral) = self.invitation_signaling(&network_id, None).await?;
                let result = signaling
                    .revoke_invitation(network_id, invitation_id, expected_revision)
                    .await;
                self.close_ephemeral_invitation_signaling(&signaling, ephemeral)
                    .await;
                result
                    .map(InvitationExecutionResult::Revoked)
                    .map_err(map_invitation_error)
            }
            InvitationRuntimeCommand::Preview {
                network_id,
                coordinator_url,
                code,
            } => {
                let (signaling, ephemeral) = self
                    .invitation_signaling(&network_id, Some(&coordinator_url))
                    .await?;
                let result = signaling.preview_invitation(network_id, code).await;
                self.close_ephemeral_invitation_signaling(&signaling, ephemeral)
                    .await;
                result
                    .map(InvitationExecutionResult::Preview)
                    .map_err(map_invitation_error)
            }
            InvitationRuntimeCommand::Redeem {
                coordinator_url,
                request,
            } => {
                let network_id = request.network_id.clone();
                let (signaling, ephemeral) = self
                    .invitation_signaling(&network_id, Some(&coordinator_url))
                    .await?;
                let result = signaling.redeem_invitation(request).await;
                self.close_ephemeral_invitation_signaling(&signaling, ephemeral)
                    .await;
                result
                    .map(InvitationExecutionResult::Redeemed)
                    .map_err(map_invitation_error)
            }
        }
    }

    async fn execute_coordinator_candidate_command(
        &mut self,
        command: CoordinatorCandidateRuntimeCommand,
    ) -> Result<CoordinatorCandidateStatus, RuntimeControlError> {
        match command {
            CoordinatorCandidateRuntimeCommand::Status { network_name } => {
                let (network_id, signaling, ephemeral) =
                    self.coordinator_candidate_signaling(&network_name).await?;
                let result = signaling
                    .coordinator_candidate_status(network_id)
                    .await
                    .map_err(map_coordinator_candidate_error);
                self.close_ephemeral_coordinator_candidate_signaling(&signaling, ephemeral)
                    .await;
                result
            }
            CoordinatorCandidateRuntimeCommand::UpdateConfig(command) => {
                let NodeUpdateCoordinatorCandidatesCommand {
                    network_name,
                    expected_revision,
                    candidates,
                    effective_from_unix_ms,
                    expires_at_unix_ms,
                    credential,
                } = command;
                let (network_id, signaling, ephemeral) =
                    self.coordinator_candidate_signaling(&network_name).await?;
                let identity = self.configs.load_node_identity().map_err(|error| {
                    coordinator_candidate_unavailable(format!(
                        "加载 Coordinator 候选配置签名身份失败: {error:#}"
                    ))
                });
                let result = match identity {
                    Ok(identity) => {
                        let revision = expected_revision.checked_add(1).ok_or_else(|| {
                            coordinator_candidate_invalid(
                                "Coordinator 候选配置 revision 溢出".to_owned(),
                            )
                        });
                        match revision {
                            Ok(revision) => {
                                let effective_from_unix_ms =
                                    effective_from_unix_ms.unwrap_or_else(unix_time_ms);
                                match CoordinatorCandidateConfig::signed(
                                    network_id.clone(),
                                    revision,
                                    candidates,
                                    effective_from_unix_ms,
                                    expires_at_unix_ms,
                                    &identity,
                                ) {
                                    Ok(config) => match credential {
                                        Some(credential) if credential.network_id != network_id => {
                                            Err(coordinator_candidate_invalid(
                                                "Coordinator 治理写入凭据属于其他网络".to_owned(),
                                            ))
                                        }
                                        Some(credential) => signaling
                                            .update_coordinator_candidates_authorized(
                                                config,
                                                expected_revision,
                                                credential,
                                            )
                                            .await
                                            .map_err(map_coordinator_candidate_error),
                                        None => signaling
                                            .update_coordinator_candidates(
                                                config,
                                                expected_revision,
                                            )
                                            .await
                                            .map_err(map_coordinator_candidate_error),
                                    },
                                    Err(error) => Err(coordinator_candidate_invalid(format!(
                                        "构造 Coordinator 候选配置失败: {error}"
                                    ))),
                                }
                            }
                            Err(error) => Err(error),
                        }
                    }
                    Err(error) => Err(error),
                };
                self.close_ephemeral_coordinator_candidate_signaling(&signaling, ephemeral)
                    .await;
                result
            }
            CoordinatorCandidateRuntimeCommand::InstallTerm { network_name, term } => {
                let (network_id, signaling, ephemeral) =
                    self.coordinator_candidate_signaling(&network_name).await?;
                let result = if term.network_id != network_id {
                    Err(coordinator_candidate_invalid(
                        "Coordinator 任期属于其他网络".to_owned(),
                    ))
                } else {
                    signaling
                        .install_coordinator_term(term)
                        .await
                        .map_err(map_coordinator_candidate_error)
                };
                self.close_ephemeral_coordinator_candidate_signaling(&signaling, ephemeral)
                    .await;
                result
            }
            CoordinatorCandidateRuntimeCommand::RenewTermLease {
                network_name,
                credential,
                new_expires_at_unix_ms,
            } => {
                let (network_id, signaling, ephemeral) =
                    self.coordinator_candidate_signaling(&network_name).await?;
                let result = if credential.network_id != network_id {
                    Err(coordinator_candidate_invalid(
                        "Coordinator 租约凭据属于其他网络".to_owned(),
                    ))
                } else {
                    signaling
                        .renew_coordinator_term_lease(credential, new_expires_at_unix_ms)
                        .await
                        .map_err(map_coordinator_candidate_error)
                };
                self.close_ephemeral_coordinator_candidate_signaling(&signaling, ephemeral)
                    .await;
                result
            }
            CoordinatorCandidateRuntimeCommand::PublishDeclaration(command) => {
                let network_name = command.network_name.clone();
                let (network_id, signaling, ephemeral) =
                    self.coordinator_candidate_signaling(&network_name).await?;
                let identity = self.configs.load_node_identity().map_err(|error| {
                    coordinator_candidate_unavailable(format!(
                        "加载 Coordinator 候选声明签名身份失败: {error:#}"
                    ))
                });
                let result =
                    match identity {
                        Ok(identity) => {
                            match signaling
                                .coordinator_candidate_status(network_id.clone())
                                .await
                                .map_err(map_coordinator_candidate_error)
                            {
                                Ok(status) => match status.config {
                                    None => Err(coordinator_candidate_unavailable(
                                        "Coordinator 候选配置尚未生效，无法发布声明",
                                    )),
                                    Some(config) => {
                                        let observed_at_unix_ms = unix_time_ms();
                                        let expires_at_unix_ms = observed_at_unix_ms
                                            .checked_add(command.validity_ms)
                                            .ok_or_else(|| {
                                                coordinator_candidate_invalid(
                                                    "Coordinator 候选声明有效期溢出".to_owned(),
                                                )
                                            });
                                        match expires_at_unix_ms {
                                    Ok(expires_at_unix_ms) => {
                                        match CoordinatorCandidateDeclaration::signed(
                                            network_id,
                                            config.revision,
                                            command.public_reachability,
                                            command.resources,
                                            command.willingness,
                                            observed_at_unix_ms,
                                            expires_at_unix_ms,
                                            &identity,
                                        ) {
                                            Ok(declaration) => signaling
                                                .publish_coordinator_candidate_declaration(
                                                    declaration,
                                                )
                                                .await
                                                .map_err(map_coordinator_candidate_error),
                                            Err(error) => Err(coordinator_candidate_invalid(
                                                format!("构造 Coordinator 候选声明失败: {error}"),
                                            )),
                                        }
                                    }
                                    Err(error) => Err(error),
                                }
                                    }
                                },
                                Err(error) => Err(error),
                            }
                        }
                        Err(error) => Err(error),
                    };
                self.close_ephemeral_coordinator_candidate_signaling(&signaling, ephemeral)
                    .await;
                result
            }
        }
    }

    async fn execute_admin_recovery_command(
        &mut self,
        command: AdminRecoveryRuntimeCommand,
    ) -> Result<AdminRecoveryExecutionResult, RuntimeControlError> {
        match command {
            AdminRecoveryRuntimeCommand::Submit(command) => {
                let NodeSubmitAdminRecoveryCommand {
                    network_name,
                    request,
                } = command;
                let (network_id, signaling) = self.admin_recovery_signaling(&network_name).await?;
                if request.proposal.network_id != network_id {
                    return Err(admin_recovery_invalid(
                        "管理员恢复请求属于其他网络，拒绝提交",
                    ));
                }
                signaling
                    .submit_admin_recovery(request)
                    .await
                    .map(AdminRecoveryExecutionResult::Status)
                    .map_err(map_admin_recovery_error)
            }
            AdminRecoveryRuntimeCommand::Status(command) => {
                let NodeAdminRecoveryStatusCommand {
                    network_name,
                    request_id,
                } = command;
                let (network_id, signaling) = self.admin_recovery_signaling(&network_name).await?;
                signaling
                    .get_admin_recovery_status(network_id, request_id)
                    .await
                    .map(AdminRecoveryExecutionResult::Status)
                    .map_err(map_admin_recovery_error)
            }
            AdminRecoveryRuntimeCommand::List(command) => {
                let (network_id, signaling) =
                    self.admin_recovery_signaling(&command.network_name).await?;
                signaling
                    .list_admin_recoveries(network_id)
                    .await
                    .map(AdminRecoveryExecutionResult::List)
                    .map_err(map_admin_recovery_error)
            }
            AdminRecoveryRuntimeCommand::Revoke(command) => {
                let NodeRevokeAdminRecoveryCommand {
                    network_name,
                    request_id,
                    reason,
                } = command;
                let (network_id, signaling) = self.admin_recovery_signaling(&network_name).await?;
                signaling
                    .revoke_admin_recovery(network_id, request_id, reason)
                    .await
                    .map(AdminRecoveryExecutionResult::Status)
                    .map_err(map_admin_recovery_error)
            }
            AdminRecoveryRuntimeCommand::Execute(command) => {
                let NodeExecuteAdminRecoveryCommand {
                    network_name,
                    request_id,
                } = command;
                let (network_id, signaling) = self.admin_recovery_signaling(&network_name).await?;
                signaling
                    .execute_admin_recovery(network_id, request_id)
                    .await
                    .map(AdminRecoveryExecutionResult::Status)
                    .map_err(map_admin_recovery_error)
            }
        }
    }

    /// Break-glass recovery is deliberately bound to an already authenticated
    /// live data-plane connection. Falling back to a fresh unqualified
    /// connection would weaken the transport identity and could make an Ack
    /// loss indistinguishable from an unauthenticated execution attempt.
    async fn admin_recovery_signaling(
        &mut self,
        network_name: &str,
    ) -> Result<(NetworkId, Arc<SignalingClient>), RuntimeControlError> {
        self.reconcile_finished_sessions().await;
        let network = self
            .configs
            .list_networks()
            .map_err(|error| {
                RuntimeControlError::Persistence(format!("加载管理员恢复网络配置失败: {error:#}"))
            })?
            .into_iter()
            .find(|network| network.name == network_name)
            .ok_or_else(|| {
                RuntimeControlError::NotFound(format!("网络 [{network_name}] 不存在"))
            })?;
        let network_id = network
            .network_id
            .as_deref()
            .ok_or_else(|| {
                admin_recovery_unavailable(format!(
                    "网络 [{network_name}] 缺少稳定网络 ID，拒绝管理员恢复"
                ))
            })
            .and_then(|network_id| {
                NetworkId::parse(network_id.to_owned()).map_err(|error| {
                    admin_recovery_invalid(format!(
                        "网络 [{network_name}] 的稳定网络 ID 无效: {error:#}"
                    ))
                })
            })?;
        let session = self.sessions.get(network_name).ok_or_else(|| {
            admin_recovery_unavailable(format!(
                "网络 [{network_name}] 没有活动的数据面运行会话，管理员恢复必须使用已认证连接"
            ))
        })?;
        if session.network_id.as_deref() != Some(network_id.as_str()) {
            return Err(admin_recovery_unavailable(format!(
                "网络 [{network_name}] 的活动运行会话尚未绑定权威网络 ID"
            )));
        }
        let signaling = session.signaling.clone().ok_or_else(|| {
            admin_recovery_unavailable(format!(
                "网络 [{network_name}] 的活动运行会话没有已认证的 Coordinator 信令连接"
            ))
        })?;
        Ok((network_id, signaling))
    }

    fn record_coordinator_control_failure(&mut self, network_name: &str, reason: String) {
        let Some(session) = self.sessions.get_mut(network_name) else {
            return;
        };
        session.coordinator_emergency = None;
        let (unavailable_for, consecutive_failures, interruptions) = session
            .coordinator_continuity
            .record_failure(Instant::now(), reason.clone());
        tracing::warn!(
            network = %network_name,
            reason = %reason,
            unavailable_ms = unavailable_for.as_millis() as u64,
            consecutive_failures,
            interruptions,
            "Coordinator 控制面暂不可用，数据面保持现有会话"
        );
    }

    fn record_coordinator_control_recovered(&mut self, network_name: &str) {
        let Some(session) = self.sessions.get_mut(network_name) else {
            return;
        };
        if let Some((recovery_time, recoveries, interruptions)) = session
            .coordinator_continuity
            .record_success(Instant::now())
        {
            tracing::info!(
                network = %network_name,
                recovery_ms = recovery_time.as_millis() as u64,
                recoveries,
                interruptions,
                "Coordinator 控制面已恢复，数据面会话未重建"
            );
        }
    }

    /// Poll only live data-plane registrations. Candidate events are hints;
    /// every round fetches authoritative status before signing a proposal or
    /// vote. A missed acknowledgement is therefore repaired by the next poll
    /// instead of being inferred from a replayed event.
    async fn refresh_coordinator_takeover(&mut self) -> CoordinatorTakeoverRefreshOutcome {
        if self
            .sessions
            .values()
            .any(|session| session.handle.is_finished())
        {
            self.reconcile_finished_sessions().await;
        }
        let sessions = self
            .sessions
            .iter()
            .filter_map(|(network_name, session)| {
                session.signaling.clone().map(|signaling| {
                    (
                        network_name.clone(),
                        signaling,
                        session.network_id.clone(),
                        session
                            .coordinator_recovery
                            .as_ref()
                            .map(|recovery| recovery.authoritative_audit_sequence)
                            .unwrap_or(0),
                    )
                })
            })
            .collect::<Vec<_>>();
        if sessions.is_empty() {
            return CoordinatorTakeoverRefreshOutcome::Idle;
        }
        let identity = match self.configs.load_node_identity() {
            Ok(identity) => identity,
            Err(error) => {
                tracing::debug!(%error, "Coordinator 接管轮次无法加载本机身份");
                return CoordinatorTakeoverRefreshOutcome::Failed;
            }
        };
        let local_node = NodeId::from_public_key(&identity.public_key());
        let mut saw_authority = false;
        let mut saw_failure = false;

        for (network_name, signaling, network_id, recovery_cursor) in sessions {
            let Some(network_id) = network_id.as_deref() else {
                tracing::warn!(network = %network_name, "Coordinator 接管轮次缺少网络 ID");
                saw_failure = true;
                continue;
            };
            let network_id = match NetworkId::parse(network_id.to_owned()) {
                Ok(network_id) => network_id,
                Err(error) => {
                    tracing::warn!(
                        network = %network_name,
                        %error,
                        "Coordinator 接管轮次跳过无效网络 ID"
                    );
                    saw_failure = true;
                    continue;
                }
            };
            let status = match signaling
                .coordinator_candidate_status(network_id.clone())
                .await
            {
                Ok(status) => status,
                Err(error) => {
                    self.record_coordinator_control_failure(
                        &network_name,
                        format!("无法刷新权威候选状态: {error}"),
                    );
                    saw_failure = true;
                    continue;
                }
            };
            let recovery = match signaling
                .recover_coordinator(CoordinatorRecoveryReport::from_status(
                    &status,
                    recovery_cursor,
                ))
                .await
            {
                Ok(recovery) => recovery,
                Err(error) => {
                    self.record_coordinator_control_failure(
                        &network_name,
                        format!("无法验证恢复快照，治理保持只读: {error}"),
                    );
                    saw_failure = true;
                    continue;
                }
            };
            if recovery.recovery.governance_writable
                || !recovery.recovery.health_checks_allowed
                || !recovery.recovery.data_plane_allowed
            {
                self.record_coordinator_control_failure(
                    &network_name,
                    format!(
                        "恢复响应包含不安全的能力声明: stage={:?}, governance_writable={}, health_checks_allowed={}, data_plane_allowed={}",
                        recovery.recovery.stage,
                        recovery.recovery.governance_writable,
                        recovery.recovery.health_checks_allowed,
                        recovery.recovery.data_plane_allowed,
                    ),
                );
                saw_failure = true;
                continue;
            }
            let status = recovery.status;
            saw_authority = true;
            let now_unix_ms = unix_time_ms();
            let emergency = match active_local_emergency(&status, &local_node, now_unix_ms) {
                Ok(emergency) => emergency,
                Err(error) => {
                    self.record_coordinator_control_failure(
                        &network_name,
                        format!("拒绝不可信的紧急租约: {error}"),
                    );
                    saw_failure = true;
                    continue;
                }
            };
            if let Some(session) = self.sessions.get_mut(&network_name) {
                session.coordinator_recovery = Some(recovery.recovery.clone());
                session.coordinator_emergency = emergency;
            }
            let decision = match decide_coordinator_takeover(&status, &identity, now_unix_ms) {
                Ok(decision) => decision,
                Err(error) => {
                    self.record_coordinator_control_failure(
                        &network_name,
                        format!("拒绝不可信的候选状态: {error}"),
                    );
                    saw_failure = true;
                    continue;
                }
            };
            self.record_coordinator_control_recovered(&network_name);
            let (action, result) = match decision {
                CoordinatorTakeoverDecision::Wait => continue,
                CoordinatorTakeoverDecision::Propose(proposal) => (
                    "propose",
                    signaling.propose_coordinator_election(proposal).await,
                ),
                CoordinatorTakeoverDecision::Vote { proposal_id, vote } => (
                    "vote",
                    signaling.vote_coordinator_election(proposal_id, vote).await,
                ),
                CoordinatorTakeoverDecision::Timeout {
                    network_id,
                    proposal_id,
                } => (
                    "timeout",
                    signaling
                        .timeout_coordinator_election(network_id, proposal_id)
                        .await,
                ),
                CoordinatorTakeoverDecision::IssueEmergency { network_id } => (
                    "issue_emergency",
                    signaling
                        .issue_emergency_leader(network_id)
                        .await
                        .map(|response| response.status),
                ),
                CoordinatorTakeoverDecision::RenewEmergency {
                    network_id,
                    credential,
                } => (
                    "renew_emergency",
                    signaling
                        .renew_emergency_leader(network_id, credential)
                        .await
                        .map(|response| response.status),
                ),
            };
            match result {
                Ok(status) => {
                    let emergency =
                        match active_local_emergency(&status, &local_node, unix_time_ms()) {
                            Ok(emergency) => emergency,
                            Err(error) => {
                                if let Some(session) = self.sessions.get_mut(&network_name) {
                                    session.coordinator_emergency = None;
                                }
                                tracing::debug!(
                                    network = %network_name,
                                    action,
                                    %error,
                                    "Coordinator 接管响应包含不可信的紧急租约"
                                );
                                saw_failure = true;
                                continue;
                            }
                        };
                    if let Some(session) = self.sessions.get_mut(&network_name) {
                        session.coordinator_emergency = emergency;
                    }
                    tracing::debug!(
                        network = %network_name,
                        action,
                        leader = ?status.current_leader,
                        term = ?status.latest_term.as_ref().map(|term| term.term),
                        election_stage = ?status.latest_election.as_ref().map(|election| election.stage),
                        "Coordinator 接管轮次已刷新权威状态"
                    );
                }
                Err(error) => {
                    if matches!(action, "issue_emergency" | "renew_emergency") {
                        if let Some(session) = self.sessions.get_mut(&network_name) {
                            session.coordinator_emergency = None;
                        }
                    }
                    tracing::debug!(
                        network = %network_name,
                        action,
                        %error,
                        "Coordinator 接管动作失败，将在下一轮重新读取权威状态"
                    );
                    saw_failure = true;
                }
            }
        }
        if saw_failure {
            CoordinatorTakeoverRefreshOutcome::Failed
        } else if saw_authority {
            CoordinatorTakeoverRefreshOutcome::Healthy
        } else {
            CoordinatorTakeoverRefreshOutcome::Failed
        }
    }

    /// Candidate administration always targets the authoritative Coordinator.
    /// Prefer the live data-plane registration, otherwise use the configured
    /// endpoint for one short-lived command and close it immediately.
    async fn coordinator_candidate_signaling(
        &mut self,
        network_name: &str,
    ) -> Result<(NetworkId, Arc<SignalingClient>, bool), RuntimeControlError> {
        self.reconcile_finished_sessions().await;
        let network = self
            .configs
            .list_networks()
            .map_err(|error| {
                RuntimeControlError::Persistence(format!(
                    "加载 Coordinator 候选网络配置失败: {error:#}"
                ))
            })?
            .into_iter()
            .find(|network| network.name == network_name)
            .ok_or_else(|| {
                RuntimeControlError::NotFound(format!("网络 [{network_name}] 不存在"))
            })?;
        let network_id_text = network.network_id.as_deref().ok_or_else(|| {
            coordinator_candidate_unavailable(format!(
                "网络 [{network_name}] 缺少稳定网络 ID，拒绝访问 Coordinator"
            ))
        })?;
        let network_id = NetworkId::parse(network_id_text.to_owned()).map_err(|error| {
            coordinator_candidate_invalid(format!(
                "网络 [{network_name}] 的稳定网络 ID 无效: {error:#}"
            ))
        })?;
        if let Some(signaling) = self
            .sessions
            .values()
            .find(|session| session.network_id.as_deref() == Some(network_id.as_str()))
            .and_then(|session| session.signaling.clone())
        {
            return Ok((network_id, signaling, false));
        }
        let coordinator_url = network.coordinator_url.trim();
        if coordinator_url.is_empty() {
            return Err(coordinator_candidate_unavailable(format!(
                "网络 [{network_name}] 没有可用的 Coordinator 信令端点"
            )));
        }
        let identity = self.configs.load_node_identity().map_err(|error| {
            coordinator_candidate_unavailable(format!(
                "加载 Coordinator 候选控制连接所需节点身份失败: {error:#}"
            ))
        })?;
        let signaling = SignalingClient::connect(coordinator_url, identity, 0, NatType::Unknown)
            .await
            .map_err(|error| {
                coordinator_candidate_unavailable(format!(
                    "建立 Coordinator 候选控制连接失败: {error:#}"
                ))
            })?;
        Ok((network_id, Arc::new(signaling), true))
    }

    async fn close_ephemeral_coordinator_candidate_signaling(
        &self,
        signaling: &Arc<SignalingClient>,
        ephemeral: bool,
    ) {
        if ephemeral {
            if let Err(error) = signaling.close().await {
                tracing::warn!(%error, "关闭一次性 Coordinator 候选控制连接失败");
            }
        }
    }

    /// Invitation management prefers the live authoritative signaling session.
    /// Applicants previewing/redeeming before joining use the endpoint carried
    /// by their command. Owner operations may fall back to the endpoint bound in
    /// local network configuration without changing that binding.
    async fn invitation_signaling(
        &mut self,
        network_id: &NetworkId,
        coordinator_url: Option<&str>,
    ) -> Result<(Arc<SignalingClient>, bool), RuntimeControlError> {
        self.reconcile_finished_sessions().await;
        if let Some(signaling) = self
            .sessions
            .values()
            .find(|session| session.network_id.as_deref() == Some(network_id.as_str()))
            .and_then(|session| session.signaling.clone())
        {
            return Ok((signaling, false));
        }

        let coordinator_url = coordinator_url
            .filter(|url| !url.trim().is_empty())
            .map(str::to_owned)
            .or_else(|| {
                self.configs
                    .list_networks()
                    .ok()
                    .and_then(|networks| {
                        networks.into_iter().find(|network| {
                            network.network_id.as_deref() == Some(network_id.as_str())
                        })
                    })
                    .map(|network| network.coordinator_url)
                    .filter(|url| !url.trim().is_empty())
            });
        let Some(coordinator_url) = coordinator_url else {
            return Err(invitation_unavailable(
                "目标网络当前未运行，且没有可用的 Websocket 邀请端点",
            ));
        };
        let identity = self.configs.load_node_identity().map_err(|error| {
            invitation_unavailable(format!("加载邀请控制连接所需节点身份失败: {error:#}"))
        })?;
        let signaling = SignalingClient::connect(&coordinator_url, identity, 0, NatType::Unknown)
            .await
            .map_err(|error| invitation_unavailable(format!("建立邀请控制连接失败: {error:#}")))?;
        Ok((Arc::new(signaling), true))
    }

    async fn close_ephemeral_invitation_signaling(
        &self,
        signaling: &Arc<SignalingClient>,
        ephemeral: bool,
    ) {
        if ephemeral {
            if let Err(error) = signaling.close().await {
                tracing::warn!(%error, "关闭一次性邀请控制连接失败");
            }
        }
    }

    /// Review operations must use the established data-plane registration.
    /// Applicant-only operations may open one short-lived signaling connection
    /// when the target network has not been joined yet. Reusing the active
    /// session is mandatory when present so the Coordinator never sees two
    /// registrations for the same node on the same network.
    async fn join_request_signaling(
        &mut self,
        network_id: &NetworkId,
        coordinator_url: Option<&str>,
        allow_ephemeral: bool,
    ) -> Result<(Arc<SignalingClient>, bool), RuntimeControlError> {
        self.reconcile_finished_sessions().await;
        if let Some(signaling) = self
            .sessions
            .values()
            .find(|session| session.network_id.as_deref() == Some(network_id.as_str()))
            .and_then(|session| session.signaling.clone())
        {
            return Ok((signaling, false));
        }
        if !allow_ephemeral {
            return Err(join_request_unavailable(
                "审批操作需要目标网络使用中的权威数据面连接",
            ));
        }
        let Some(coordinator_url) = coordinator_url.filter(|url| !url.trim().is_empty()) else {
            return Err(join_request_unavailable(
                "目标网络当前未运行，且没有可用的签名协调端点",
            ));
        };
        let identity = self.configs.load_node_identity().map_err(|error| {
            join_request_unavailable(format!("加载审批连接所需节点身份失败: {error:#}"))
        })?;
        let signaling = SignalingClient::connect(coordinator_url, identity, 0, NatType::Unknown)
            .await
            .map_err(|error| {
                join_request_unavailable(format!("建立审批控制连接失败: {error:#}"))
            })?;
        Ok((Arc::new(signaling), true))
    }

    async fn close_ephemeral_join_request_signaling(
        &self,
        signaling: &Arc<SignalingClient>,
        ephemeral: bool,
    ) {
        if ephemeral {
            if let Err(error) = signaling.close().await {
                tracing::warn!(%error, "关闭一次性审批控制连接失败");
            }
        }
    }

    async fn start_node(&mut self) -> Result<StartSummary, RuntimeControlError> {
        self.start_node_until(Instant::now() + self.timeouts.start)
            .await
    }

    async fn fail_lifecycle(&self, error: RuntimeControlError) -> RuntimeControlError {
        let lifecycle = self.lifecycle.failed().await;
        self.state.set_lifecycle(lifecycle).await;
        error
    }

    async fn start_node_until(
        &mut self,
        deadline: Instant,
    ) -> Result<StartSummary, RuntimeControlError> {
        let networks = self.configs.list_networks().map_err(|error| {
            RuntimeControlError::Persistence(format!("加载持久化网络列表失败: {error:#}"))
        })?;
        let configured = networks.len();
        self.state
            .update(|snapshot| {
                snapshot.permissions = runtime_metadata::trusted_control_plane_permissions();
                snapshot.configuration.networks.clear();
            })
            .await;
        let lifecycle = self.lifecycle.begin_start(configured, Vec::new()).await;
        self.state.set_lifecycle(lifecycle).await;

        // Starting the node is not the same operation as `up --all`: restore
        // only the default network here. Explicit `up <network...>` commands
        // add their selected networks afterwards. Loading every persisted
        // network at bootstrap can create unrelated TUNs with overlapping
        // virtual addresses before the requested plan is applied.
        let selection = if networks.is_empty() {
            NodeNetworkSelection::None
        } else {
            NodeNetworkSelection::Default
        };
        let plan = match NodeRuntimePlan::load(&self.configs, selection) {
            Ok(plan) => plan,
            Err(error) => {
                let lifecycle = self.lifecycle.failed().await;
                self.state.set_lifecycle(lifecycle).await;
                return Err(RuntimeControlError::Persistence(format!(
                    "加载节点启动计划失败: {error:#}"
                )));
            }
        };
        match self
            .start_persisted_plan_until(plan, deadline, &BTreeSet::new())
            .await
        {
            Ok(summary) => Ok(summary),
            Err(error) => Err(self.fail_lifecycle(error).await),
        }
    }

    async fn start_persisted_plan_until(
        &mut self,
        plan: NodeRuntimePlan,
        deadline: Instant,
        skip_networks: &BTreeSet<String>,
    ) -> Result<StartSummary, RuntimeControlError> {
        self.reconcile_finished_sessions().await;
        let configured = plan.len();
        let mut summary = StartSummary {
            requested: configured,
            ..StartSummary::default()
        };

        let selected_names = plan
            .networks()
            .iter()
            .map(|network| network.name.clone())
            .collect::<BTreeSet<_>>();
        let mut service_failures = self
            .restore_owned_embedded_services(selected_names, skip_networks, deadline)
            .await?;

        for spec in plan.into_networks() {
            if skip_networks.contains(&spec.name) {
                continue;
            }
            if self.sessions.contains_key(&spec.name) {
                summary.already_running.push(spec.name);
                continue;
            }
            if let Some(message) = service_failures.remove(&spec.name) {
                let error = RuntimeControlError::Launch(format!(
                    "内嵌服务恢复失败，未启动数据面: {message}"
                ));
                self.record_launch_failure(&spec.name, &error).await;
                summary.failed.push(StartFailure {
                    network_name: spec.name,
                    message: error.to_string(),
                    stage: error.network_setup_stage(),
                });
                continue;
            }
            let network_name = spec.name.clone();
            match self.start_spec_until(spec, deadline).await {
                Ok(()) => summary.started.push(network_name),
                Err(error) => summary.failed.push(StartFailure {
                    network_name,
                    message: error.to_string(),
                    stage: error.network_setup_stage(),
                }),
            }
        }

        self.reconcile_finished_sessions().await;
        let active = self.sessions.len();
        let failed = summary
            .failed
            .iter()
            .map(|failure| failure.network_name.clone())
            .collect::<Vec<_>>();
        let lifecycle = if configured > 0 && active == 0 {
            self.lifecycle
                .failed_with_configuration(configured, failed)
                .await
        } else {
            self.lifecycle.running(active, failed).await
        };
        self.state.set_lifecycle(lifecycle).await;
        Ok(summary)
    }

    async fn restore_owned_embedded_services(
        &self,
        selected_names: BTreeSet<String>,
        skip_networks: &BTreeSet<String>,
        deadline: Instant,
    ) -> Result<HashMap<String, String>, RuntimeControlError> {
        let networks = self.configs.list_networks().map_err(|error| {
            RuntimeControlError::Persistence(format!("加载持久化网络列表失败: {error:#}"))
        })?;
        let running_services = self
            .services
            .list()
            .await
            .map_err(map_service_error)?
            .into_iter()
            .map(|service| service.network_name)
            .collect::<BTreeSet<_>>();
        let mut service_failures = HashMap::new();
        for network in networks.iter().filter(|network| {
            selected_names.contains(&network.name)
                && network.owns_embedded_services
                && !skip_networks.contains(&network.name)
        }) {
            if let Err(error) = self
                .restore_owned_embedded_service(network, &running_services, deadline)
                .await
            {
                service_failures.insert(network.name.clone(), error.to_string());
            }
        }
        Ok(service_failures)
    }

    async fn restore_owned_embedded_service(
        &self,
        network: &NetworkConfig,
        running_services: &BTreeSet<String>,
        deadline: Instant,
    ) -> Result<(), RuntimeControlError> {
        if running_services.contains(&network.name) {
            if let Some(service) = self
                .services
                .list()
                .await
                .map_err(map_service_error)?
                .into_iter()
                .find(|service| service.network_name == network.name)
            {
                let service = if network.relay_enabled && service.relay_addr.is_none() {
                    match self.services.enable_relay(&network.name).await {
                        Ok(updated) => updated,
                        Err(error) => {
                            return Err(self
                                .project_embedded_service_start_failure(&network.name, error)
                                .await);
                        }
                    }
                } else {
                    service
                };
                self.record_running_embedded_service(&service).await;
                return Ok(());
            }
        }

        self.mark_embedded_service_status(&network.name, NodeEmbeddedServiceStatus::Starting)
            .await;
        let host_public_key = self
            .configs
            .load_node_identity()
            .map_err(|error| {
                RuntimeControlError::Persistence(format!(
                    "网络 [{}] 加载节点身份失败: {error:#}",
                    network.name
                ))
            })?
            .public_key_bytes();
        let request = EmbeddedServiceStartRequest::new(
            network.name.clone(),
            network.signaling_port.unwrap_or(0),
            network.relay_port.unwrap_or(0),
        )
        .with_host_public_key(host_public_key)
        .with_relay_enabled(network.relay_enabled);
        let request = match network.external_host.clone() {
            Some(host) => request.with_external_host(host),
            None => request.with_external_host("127.0.0.1"),
        };
        let request = match network.ip_pool.clone() {
            Some(pool) => request.with_ip_pool(pool),
            None => request,
        };
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            self.mark_embedded_service_status(&network.name, NodeEmbeddedServiceStatus::Failed)
                .await;
            return Err(RuntimeControlError::Timeout(format!(
                "start/timeout: 网络 [{}] 的内嵌服务未在启动期限内恢复",
                network.name
            )));
        }
        let service = match timeout(remaining, self.services.start(request)).await {
            Ok(Ok(service)) => service,
            Ok(Err(error)) => {
                return Err(self
                    .project_embedded_service_start_failure(&network.name, error)
                    .await);
            }
            Err(_) => {
                self.mark_embedded_service_status(&network.name, NodeEmbeddedServiceStatus::Failed)
                    .await;
                return Err(RuntimeControlError::Timeout(format!(
                    "start/timeout: 网络 [{}] 的内嵌服务未在期限内恢复",
                    network.name
                )));
            }
        };
        self.record_running_embedded_service(&service).await;

        if let Err(error) = self.configs.set_embedded_service_metadata(
            &network.name,
            service.signaling_addr.port(),
            service
                .relay_addr
                .map(|addr| addr.port())
                .or(network.relay_port),
            network.relay_enabled,
            network.external_host.clone(),
            network.ip_pool.clone(),
        ) {
            self.stop_embedded_service(&network.name).await;
            return Err(RuntimeControlError::Persistence(format!(
                "网络 [{}] 的内嵌服务已启动，但端点持久化失败: {error:#}",
                network.name
            )));
        }
        Ok(())
    }

    async fn project_embedded_service_start_failure(
        &self,
        network_name: &str,
        error: EmbeddedServiceError,
    ) -> RuntimeControlError {
        if matches!(&error, EmbeddedServiceError::RelayStartFailed { .. }) {
            if let Ok(service) = self.services.list().await.and_then(|services| {
                services
                    .into_iter()
                    .find(|service| service.network_name == network_name)
                    .ok_or_else(|| EmbeddedServiceError::NotFound(network_name.into()))
            }) {
                self.record_running_embedded_service(&service).await;
                return map_service_error(error);
            }
        }
        self.mark_embedded_service_status(network_name, NodeEmbeddedServiceStatus::Failed)
            .await;
        map_service_error(error)
    }

    async fn stop_node(&mut self) -> Result<StopSummary, RuntimeControlError> {
        let deadline = Instant::now() + self.timeouts.stop;
        let lifecycle = self.lifecycle.begin_stop().await;
        self.state.set_lifecycle(lifecycle).await;
        let summary = match self.stop_until(Vec::new(), true, true, deadline).await {
            Ok(summary) => summary,
            Err(error) => return Err(self.fail_lifecycle(error).await),
        };
        let cleanly_stopped = summary.is_success();
        let lifecycle = if cleanly_stopped {
            self.lifecycle.stopped().await
        } else {
            self.lifecycle.failed().await
        };
        self.state.set_lifecycle(lifecycle).await;
        Ok(summary)
    }

    async fn restart_node(&mut self) -> Result<RestartSummary, RuntimeControlError> {
        let deadline = Instant::now() + self.timeouts.restart;
        let plan =
            NodeRuntimePlan::load(&self.configs, NodeNetworkSelection::All).map_err(|error| {
                RuntimeControlError::Persistence(format!(
                    "restart/{}: 加载重启配置失败，现有运行态未改变: {error:#}",
                    RestartPhase::PersistLoad.as_str()
                ))
            })?;
        let names = plan
            .networks()
            .iter()
            .map(|network| network.name.clone())
            .collect::<Vec<_>>();

        let lifecycle = self.lifecycle.begin_stop().await;
        self.state.set_lifecycle(lifecycle).await;
        let stopped = match self.stop_until(Vec::new(), true, true, deadline).await {
            Ok(summary) => summary,
            Err(error) => return Err(self.fail_lifecycle(error).await),
        };

        let mut summary = RestartSummary {
            requested: plan.len(),
            ..RestartSummary::default()
        };
        for failure in stopped.failed {
            summary.failed.push(RestartFailure {
                network_name: failure.network_name,
                phase: RestartPhase::Stop,
                message: failure.message,
                stage: None,
            });
        }

        let stopped_runtime_outcomes = stopped.cleanup.runtime_outcomes;
        summary.cleanup = match self.inspect_cleanup(&names).await {
            Ok(cleanup) => cleanup,
            Err(error) => return Err(self.fail_lifecycle(error).await),
        };
        summary.cleanup.runtime_outcomes = stopped_runtime_outcomes.clone();

        let mut blocked = summary
            .failed
            .iter()
            .map(|failure| failure.network_name.clone())
            .collect::<BTreeSet<_>>();
        for name in &names {
            if summary.cleanup.blocks_restart(name) && !blocked.contains(name) {
                summary.failed.push(RestartFailure {
                    network_name: name.clone(),
                    phase: RestartPhase::Cleanup,
                    message: cleanup_block_message(&summary.cleanup, name),
                    stage: None,
                });
                blocked.insert(name.clone());
            }
        }

        let lifecycle = self.lifecycle.begin_start(plan.len(), Vec::new()).await;
        self.state.set_lifecycle(lifecycle).await;
        let started = match self
            .start_persisted_plan_until(plan, deadline, &blocked)
            .await
        {
            Ok(summary) => summary,
            Err(error) => return Err(self.fail_lifecycle(error).await),
        };
        summary.restarted = started.started;
        for failure in started.failed {
            summary.failed.push(RestartFailure {
                network_name: failure.network_name,
                phase: RestartPhase::Start,
                message: failure.message,
                stage: failure.stage,
            });
        }
        summary.cleanup = match self.inspect_cleanup(&names).await {
            Ok(mut cleanup) => {
                cleanup.runtime_outcomes = stopped_runtime_outcomes;
                cleanup
            }
            Err(error) => return Err(self.fail_lifecycle(error).await),
        };
        self.publish_restart_lifecycle(&summary).await;
        Ok(summary)
    }

    async fn start_selection(
        &mut self,
        selection: NodeNetworkSelection,
    ) -> Result<StartSummary, RuntimeControlError> {
        let plan = NodeRuntimePlan::load(&self.configs, selection).map_err(|error| {
            RuntimeControlError::Persistence(format!("加载网络启动计划失败: {error:#}"))
        })?;
        self.start_plan(plan).await
    }

    async fn start_plan(
        &mut self,
        plan: NodeRuntimePlan,
    ) -> Result<StartSummary, RuntimeControlError> {
        let deadline = Instant::now() + self.timeouts.start;
        self.start_plan_until(plan, deadline).await
    }

    async fn start_plan_until(
        &mut self,
        plan: NodeRuntimePlan,
        deadline: Instant,
    ) -> Result<StartSummary, RuntimeControlError> {
        self.reconcile_finished_sessions().await;
        let requested = plan.len();
        let mut summary = StartSummary {
            requested,
            ..StartSummary::default()
        };

        let selected_names = plan
            .networks()
            .iter()
            .map(|network| network.name.clone())
            .collect::<BTreeSet<_>>();
        let no_skips = BTreeSet::new();
        let mut service_failures = self
            .restore_owned_embedded_services(selected_names, &no_skips, deadline)
            .await?;

        for spec in plan.into_networks() {
            if self.sessions.contains_key(&spec.name) {
                summary.already_running.push(spec.name);
                continue;
            }
            if let Some(message) = service_failures.remove(&spec.name) {
                let error = RuntimeControlError::Launch(format!(
                    "内嵌服务恢复失败，未启动数据面: {message}"
                ));
                self.record_launch_failure(&spec.name, &error).await;
                summary.failed.push(StartFailure {
                    network_name: spec.name,
                    message: error.to_string(),
                    stage: error.network_setup_stage(),
                });
                continue;
            }

            let network_name = spec.name.clone();
            match self.start_spec_until(spec, deadline).await {
                Ok(()) => summary.started.push(network_name),
                Err(error) => {
                    summary.failed.push(StartFailure {
                        network_name,
                        message: error.to_string(),
                        stage: error.network_setup_stage(),
                    });
                }
            }
        }

        self.publish_dynamic_lifecycle().await;
        Ok(summary)
    }

    async fn start_spec(
        &mut self,
        spec: super::NodeNetworkSpec,
    ) -> Result<(), RuntimeControlError> {
        self.start_spec_until(spec, Instant::now() + self.timeouts.start)
            .await
    }

    async fn start_spec_until(
        &mut self,
        mut spec: super::NodeNetworkSpec,
        deadline: Instant,
    ) -> Result<(), RuntimeControlError> {
        if let Some(outcome) = self.cleanup_failures.get(&spec.name) {
            return Err(RuntimeControlError::Unavailable(format!(
                "start/cleanup: 网络 [{}] 的既有运行资源尚未清理，禁止重新启动: {}",
                spec.name,
                outcome.summary()
            )));
        }
        if self.retained_owners.contains_key(&spec.name) {
            return Err(RuntimeControlError::Unavailable(format!(
                "start/cleanup: 网络 [{}] 仍保留可重试的运行时资源所有者，禁止重新启动",
                spec.name
            )));
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(RuntimeControlError::Timeout(format!(
                "start/timeout: 网络 [{}] 未在启动期限内开始",
                spec.name
            )));
        }
        commands::apply_runtime_options_to_config(&mut spec.config, &self.options).map_err(
            |error| {
                RuntimeControlError::Invalid(format!(
                    "应用网络 [{}] 启动覆盖失败: {error:#}",
                    spec.name
                ))
            },
        )?;

        let config_snapshot = runtime_metadata::network_config_snapshot(&spec.name, &spec.config);
        let initial_roles = roles_from_config(&spec.config);
        self.record_network_config(config_snapshot.clone()).await;
        self.record_network(NodeNetworkSnapshot {
            name: spec.name.clone(),
            tun_name: crate::multi_network::derive_tun_name(
                Some(&spec.name),
                &spec.config.tun.name,
            ),
            status: NodeNetworkStatus::Connecting,
            assigned_ipv6: None,
            assigned_ipv4: None,
            p2p_listen_addr: None,
            uptime_secs: 0,
            roles: initial_roles.clone(),
            peer_count: 0,
            traffic: NodeTrafficSnapshot::default(),
        })
        .await;

        let network_name = spec.name.clone();
        let network_id = spec
            .network_id
            .as_deref()
            .map(|network_id| NetworkId::parse(network_id.to_owned()))
            .transpose()
            .map_err(|error| {
                RuntimeControlError::Invalid(format!(
                    "网络 [{}] 的权威网络 ID 无法解析: {error}",
                    spec.name
                ))
            })?;
        let local_exit_enabled = self.configs.node_local_exit_enabled().map_err(|error| {
            RuntimeControlError::Persistence(format!(
                "网络 [{network_name}] 读取本机 Exit owner intent 失败: {error:#}"
            ))
        })?;
        let launch = self.launcher.launch(NetworkLaunchRequest {
            network_name: network_name.clone(),
            network_id,
            config_path: spec.config_path.to_string_lossy().into_owned(),
            config: spec.config,
            local_exit_enabled,
        });
        let started = match timeout(remaining.min(self.timeouts.start), launch).await {
            Ok(started) => started,
            Err(_) => {
                let error = RuntimeControlError::Timeout(format!(
                    "start/timeout: 网络 [{network_name}] 未在启动期限内完成"
                ));
                self.record_launch_failure(&network_name, &error).await;
                return Err(error);
            }
        };
        let started = match started {
            Ok(started) => started,
            Err(error) => {
                self.record_launch_failure(&network_name, &error).await;
                return Err(error);
            }
        };

        self.record_identity(started.identity).await;
        self.record_network(NodeNetworkSnapshot {
            name: network_name.clone(),
            tun_name: started.tun_name.clone(),
            status: NodeNetworkStatus::Online,
            assigned_ipv6: Some(started.assigned_ipv6),
            assigned_ipv4: started.assigned_ipv4,
            p2p_listen_addr: Some(started.p2p_listen_addr),
            uptime_secs: 0,
            roles: initial_roles,
            peer_count: 0,
            traffic: NodeTrafficSnapshot::default(),
        })
        .await;
        self.state
            .update(|snapshot| {
                if snapshot
                    .last_error
                    .as_ref()
                    .is_some_and(|problem| problem.network_name.as_deref() == Some(&network_name))
                {
                    snapshot.last_error = None;
                }
            })
            .await;
        if !self.publish_started_routing(&config_snapshot).await {
            let _ = started.shutdown.send(true);
            let _ = timeout(self.timeouts.stop, started.handle).await;
            return Err(RuntimeControlError::Persistence(format!(
                "网络 [{network_name}] 运行时已启动，但权威路由摘要未能发布，已停止该运行时"
            )));
        }
        self.cleanup_failures.remove(&network_name);
        self.retained_owners.remove(&network_name);
        self.completed_cleanup.remove(&network_name);
        self.sessions.insert(
            network_name,
            NetworkRuntimeSession {
                network_id: started.network_id,
                signaling: started.signaling,
                coordinator_recovery: None,
                coordinator_emergency: None,
                coordinator_continuity: CoordinatorContinuityMetrics::default(),
                tun_name: started.tun_name,
                routing_control: started.routing_control,
                shutdown: started.shutdown,
                handle: started.handle,
            },
        );
        self.refresh_local_exit_capability_state().await;
        Ok(())
    }

    async fn create(
        &mut self,
        command: NodeCreateNetworkCommand,
    ) -> Result<CreateResult, RuntimeControlError> {
        if self.configs.network_exists(&command.name)
            || self.sessions.contains_key(&command.name)
            || self
                .services
                .list()
                .await
                .map_err(map_service_error)?
                .iter()
                .any(|service| service.network_name == command.name)
        {
            return Err(RuntimeControlError::CreationConflict {
                reason: "duplicate_name",
                message: format!("网络 [{}] 已存在或正在运行", command.name),
            });
        }

        // Candidate admission derives its authoritative network ID from the
        // signed NetworkRecord. Persist the record and owner configuration
        // before binding embedded services so startup can never use a
        // fabricated or implicit network identity.
        let coordinator_url = format!("ws://127.0.0.1:{}", command.signaling_port);
        let created = match self.configs.create_owned_network(
            command.create_profile(),
            &coordinator_url,
            true,
        ) {
            Ok(created) => created,
            Err(error) => return Err(map_network_creation_error(&command.name, error)),
        };

        let host_public_key = match self.configs.load_node_identity() {
            Ok(identity) => identity.public_key_bytes(),
            Err(error) => {
                self.remove_created_network_persistence(&command.name).await;
                return Err(RuntimeControlError::Persistence(format!(
                    "网络 [{}] 加载节点身份失败: {error:#}",
                    command.name
                )));
            }
        };
        let service_request = EmbeddedServiceStartRequest::new(
            command.name.clone(),
            command.signaling_port,
            command.relay_port,
        )
        .with_host_public_key(host_public_key);
        let service_request = match command.external_host.clone() {
            Some(host) => service_request.with_external_host(host),
            None => service_request.with_external_host("127.0.0.1"),
        };
        self.mark_embedded_service_status(&command.name, NodeEmbeddedServiceStatus::Starting)
            .await;
        let service = match self.services.start(service_request).await {
            Ok(service) => service,
            Err(error) => {
                if matches!(&error, EmbeddedServiceError::RelayStartFailed { .. }) {
                    self.rollback_created_network(&command.name).await;
                } else {
                    self.rollback_created_network_after_service_failure(&command.name)
                        .await;
                }
                return Err(map_service_error(error));
            }
        };
        self.record_running_embedded_service(&service).await;
        let joined_membership = match self
            .configs
            .persist_owned_network_membership(&created.record)
        {
            Ok(membership) => membership,
            Err(error) => {
                self.rollback_created_network(&command.name).await;
                return Err(RuntimeControlError::Persistence(format!(
                    "网络 [{}] 的创建者成员关系持久化失败，已回滚: {error:#}",
                    command.name
                )));
            }
        };
        self.record_membership(&joined_membership, None).await;
        let config_file = created.config_path;
        if let Err(error) = self.configs.set_embedded_service_metadata(
            &command.name,
            service.signaling_addr.port(),
            service.relay_addr.map(|addr| addr.port()),
            true,
            command.external_host.clone(),
            None,
        ) {
            self.rollback_created_network(&command.name).await;
            return Err(RuntimeControlError::Persistence(format!(
                "网络 [{}] 的内嵌服务已启动，但端点持久化失败，已回滚: {error:#}",
                command.name
            )));
        }
        let config = match ClientConfig::from_file(&config_file) {
            Ok(config) => config,
            Err(error) => {
                self.rollback_created_network(&command.name).await;
                return Err(RuntimeControlError::Persistence(format!(
                    "读取新网络 [{}] 配置失败，已回滚: {error:#}",
                    command.name
                )));
            }
        };
        self.record_network_config(runtime_metadata::network_config_snapshot(
            &command.name,
            &config,
        ))
        .await;

        if command.auto_connect {
            let spec = super::NodeNetworkSpec {
                name: command.name.clone(),
                network_id: Some(created.record.network_id.to_string()),
                config_path: config_file,
                config,
            };
            if let Err(error) = self.start_spec(spec).await {
                self.rollback_created_network(&command.name).await;
                return Err(RuntimeControlError::Launch(format!(
                    "网络 [{}] 运行时启动失败，已回滚配置与内嵌服务: {error}",
                    command.name
                )));
            }
            self.record_membership(&joined_membership, Some(NodeNetworkStatus::Online))
                .await;
        } else {
            self.record_network(NodeNetworkSnapshot {
                name: command.name.clone(),
                tun_name: crate::multi_network::derive_tun_name(
                    Some(&command.name),
                    &config.tun.name,
                ),
                status: NodeNetworkStatus::Offline,
                assigned_ipv6: None,
                assigned_ipv4: None,
                p2p_listen_addr: None,
                uptime_secs: 0,
                roles: roles_from_config(&config),
                peer_count: 0,
                traffic: NodeTrafficSnapshot::default(),
            })
            .await;
        }

        self.publish_dynamic_lifecycle().await;
        Ok(CreateResult {
            network_name: command.name,
            network_id: created.record.network_id.to_string(),
            public_key: created.record.public_key.to_string(),
            owner_role: MembershipRole::Owner,
            next_steps: vec![
                NodeNetworkCreationNextStep::InviteMembers,
                NodeNetworkCreationNextStep::ConfigurePolicy,
                NodeNetworkCreationNextStep::ContributeCapabilities,
            ],
            config_saved: true,
            started: command.auto_connect,
            ticket: service.ticket,
        })
    }

    async fn join(
        &mut self,
        command: Box<NodeJoinNetworkCommand>,
    ) -> Result<JoinResult, RuntimeControlError> {
        let (coordinator_url, default_name) = parse_join_target(&command.target)?;
        let network_name = command.network_name.clone().unwrap_or(default_name);

        if self.configs.network_exists(&network_name) {
            self.verify_existing_join_identity(&network_name, &coordinator_url, command.as_ref())?;
            if !command.start {
                return Ok(JoinResult {
                    network_name,
                    coordinator_url,
                    config_saved: false,
                    started: false,
                });
            }
            let summary = self
                .start_selection(NodeNetworkSelection::Named(vec![network_name.clone()]))
                .await?;
            if summary.started.is_empty() && !summary.failed.is_empty() {
                return Err(RuntimeControlError::Launch(summary.message("加入网络")));
            }
            return Ok(JoinResult {
                network_name,
                coordinator_url,
                config_saved: false,
                started: !summary.started.is_empty() || !summary.already_running.is_empty(),
            });
        }

        let config_file = self
            .configs
            .create_network(&network_name, &coordinator_url, true)
            .map_err(|error| {
                RuntimeControlError::Persistence(format!(
                    "保存网络 [{network_name}] 配置失败: {error:#}"
                ))
            })?;

        if let Some(network_id) = command.network_id.as_deref() {
            let network_public_key = command
                .discovery_record
                .as_ref()
                .map(|record| record.document.public_key.to_string())
                .or_else(|| {
                    command
                        .signed_join_preview
                        .as_ref()
                        .map(|preview| preview.public_key.to_string())
                });
            let metadata_result = match network_public_key.as_deref() {
                Some(public_key) => self.configs.set_network_identity_metadata(
                    &network_name,
                    network_id,
                    public_key,
                ),
                None => self
                    .configs
                    .set_network_id_metadata(&network_name, network_id),
            };
            if let Err(error) = metadata_result {
                let rollback = self.rollback_failed_join(&network_name).await;
                return Err(join_rollback_error(
                    format!("网络 [{network_name}] 已保存，但写入稳定身份失败: {error:#}"),
                    rollback.err(),
                ));
            }
        }

        let mut started = false;
        if command.start {
            let config = match ClientConfig::from_file(&config_file) {
                Ok(config) => config,
                Err(error) => {
                    let rollback = self.rollback_failed_join(&network_name).await;
                    return Err(join_rollback_error(
                        format!("网络 [{network_name}] 已保存，但读取配置失败: {error:#}"),
                        rollback.err(),
                    ));
                }
            };
            let spec = super::NodeNetworkSpec {
                name: network_name.clone(),
                network_id: command
                    .discovery_record
                    .as_ref()
                    .map(|record| record.document.network_id.to_string())
                    .or_else(|| command.network_id.clone()),
                config_path: config_file,
                config,
            };
            if let Err(error) = self.start_spec(spec).await {
                let rollback = self.rollback_failed_join(&network_name).await;
                return Err(join_rollback_error(
                    format!("网络 [{network_name}] 配置已保存，但启动失败: {error}"),
                    rollback.err(),
                ));
            }
            started = true;
        }

        self.publish_dynamic_lifecycle().await;
        Ok(JoinResult {
            network_name,
            coordinator_url,
            config_saved: true,
            started,
        })
    }

    fn verify_existing_join_identity(
        &self,
        network_name: &str,
        coordinator_url: &str,
        command: &NodeJoinNetworkCommand,
    ) -> Result<(), RuntimeControlError> {
        let networks = self.configs.list_networks().map_err(|error| {
            RuntimeControlError::Persistence(format!(
                "网络 [{network_name}] 已存在，但读取本地身份元数据失败: {error:#}"
            ))
        })?;
        let existing = networks
            .into_iter()
            .find(|network| network.name == network_name)
            .ok_or_else(|| {
                RuntimeControlError::Conflict(format!("网络配置已存在: {network_name}"))
            })?;

        if existing.coordinator_url != coordinator_url {
            return Err(RuntimeControlError::Conflict(format!(
                "网络 [{network_name}] 已绑定其他协调端点，不能覆盖为 [{coordinator_url}]"
            )));
        }

        let Some(expected_id) = command.network_id.as_deref() else {
            return Ok(());
        };
        match existing.network_id.as_deref() {
            Some(existing_id) if existing_id == expected_id => Ok(()),
            Some(existing_id) => Err(RuntimeControlError::Conflict(format!(
                "网络 [{network_name}] 已绑定稳定网络 ID [{existing_id}]，与加入记录 [{expected_id}] 不一致"
            ))),
            None => {
                let network_public_key = command
                    .discovery_record
                    .as_ref()
                    .map(|record| record.document.public_key.to_string())
                    .or_else(|| {
                        command
                            .signed_join_preview
                            .as_ref()
                            .map(|preview| preview.public_key.to_string())
                    });
                let metadata_result = match network_public_key.as_deref() {
                    Some(public_key) => self.configs.set_network_identity_metadata(
                        network_name,
                        expected_id,
                        public_key,
                    ),
                    None => self
                        .configs
                        .set_network_id_metadata(network_name, expected_id),
                };
                metadata_result.map_err(|error| {
                    RuntimeControlError::Persistence(format!(
                        "网络 [{network_name}] 的稳定身份元数据写入失败: {error:#}"
                    ))
                })
            }
        }
    }

    async fn leave(&mut self, network_name: String) -> Result<LeaveResult, RuntimeControlError> {
        let config_removed = self.configs.network_exists(&network_name);
        let deadline = Instant::now() + self.timeouts.stop;
        match self
            .stop_until(vec![network_name.clone()], false, true, deadline)
            .await
        {
            Ok(summary) if summary.is_success() => {}
            Ok(summary) => {
                return Err(RuntimeControlError::Launch(format!(
                    "退出网络 [{network_name}] 前停止运行时失败: {}",
                    summary.message()
                )));
            }
            Err(RuntimeControlError::NotFound(_)) => {
                // An offline configured network has no actor/service to stop.
            }
            Err(error) => return Err(error),
        }

        if config_removed {
            self.configs
                .delete_network(&network_name)
                .map_err(|error| {
                    RuntimeControlError::Persistence(format!(
                        "退出网络 [{network_name}] 时删除本地配置失败: {error:#}"
                    ))
                })?;
        }
        self.forget_network(&network_name).await;
        self.publish_dynamic_lifecycle().await;

        Ok(LeaveResult {
            network_name,
            config_removed,
        })
    }

    async fn rollback_failed_join(&self, network_name: &str) -> Result<(), RuntimeControlError> {
        self.state
            .update(|snapshot| {
                snapshot
                    .configuration
                    .networks
                    .retain(|network| network.network_name != network_name);
                snapshot
                    .networks
                    .retain(|network| network.name != network_name);
                snapshot
                    .peers
                    .retain(|peer| peer.network_name != network_name);
                snapshot
                    .routing_summaries
                    .retain(|summary| summary.network_name != network_name);
                if snapshot
                    .last_error
                    .as_ref()
                    .is_some_and(|problem| problem.network_name.as_deref() == Some(network_name))
                {
                    snapshot.last_error = None;
                }
            })
            .await;
        self.configs.delete_network(network_name).map_err(|error| {
            RuntimeControlError::Persistence(format!(
                "回滚网络 [{network_name}] 的未完成配置失败: {error:#}"
            ))
        })
    }

    async fn update_routing(
        &mut self,
        command: NodeUpdateRoutingCommand,
    ) -> Result<ipc::RoutingUpdateResult, RuntimeControlError> {
        self.reconcile_finished_sessions().await;

        let network_name = command.network_name.clone();
        let routing_control = self
            .sessions
            .get(&network_name)
            .map(|session| session.routing_control.clone())
            .ok_or_else(|| RuntimeControlError::NotFound(format!("网络未运行: {network_name}")))?;

        let previous = self
            .state
            .snapshot()
            .await
            .configuration
            .networks
            .into_iter()
            .find(|network| network.network_name == network_name)
            .ok_or_else(|| {
                RuntimeControlError::Persistence(format!(
                    "网络 [{network_name}] 的共享配置快照缺失，未应用路由更新"
                ))
            })?;
        let previous_roles = self
            .state
            .snapshot()
            .await
            .networks
            .into_iter()
            .find(|network| network.name == network_name)
            .map(|network| network.roles)
            .ok_or_else(|| {
                RuntimeControlError::Persistence(format!(
                    "网络 [{network_name}] 的共享运行快照缺失，未应用路由更新"
                ))
            })?;
        ensure_member_role(&network_name, &previous_roles)?;

        if let Err(problem) = command.validate_runtime_policy() {
            let message = problem.message.clone();
            self.record_routing_problem(
                &network_name,
                NodeRoutingSummaryStatus::Invalid,
                "runtime.routing.problem.invalid",
                NodeProblemCode::InvalidRequest,
                message.clone(),
            )
            .await;
            return Err(RuntimeControlError::Invalid(message));
        }

        let result = match request_runtime_routing_update(
            &routing_control,
            &command,
            self.timeouts.routing_update,
        )
        .await
        {
            Ok(result) => result,
            Err(RuntimeRoutingRequestError::ControlClosed) => {
                self.record_routing_problem(
                    &network_name,
                    NodeRoutingSummaryStatus::ApplyFailed,
                    "runtime.routing.problem.hot_update_failed",
                    NodeProblemCode::Unavailable,
                    format!("网络 [{network_name}] 的路由运行时控制通道已关闭，未应用新的路由策略"),
                )
                .await;
                return Err(RuntimeControlError::NotFound(format!(
                    "网络 [{network_name}] 的运行时控制通道已关闭"
                )));
            }
            Err(failure) => {
                let failure_message = describe_routing_request_failure(&failure);
                let rollback = rollback_runtime_routing(
                    &routing_control,
                    &previous,
                    self.timeouts.routing_update,
                )
                .await;
                return Err(match rollback {
                    Ok(()) => {
                        self.record_routing_problem(
                            &network_name,
                            NodeRoutingSummaryStatus::ApplyFailed,
                            "runtime.routing.problem.hot_update_failed",
                            NodeProblemCode::Unavailable,
                            format!(
                                "网络 [{network_name}] 路由热更新失败: {failure_message}；旧路由已重新应用"
                            ),
                        )
                        .await;
                        RuntimeControlError::Persistence(format!(
                            "网络 [{network_name}] 路由热更新失败: {failure_message}；已通过运行时重新应用旧配置，文件和运行态已回滚"
                        ))
                    }
                    Err(rollback_error) => {
                        self.record_routing_problem(
                            &network_name,
                            NodeRoutingSummaryStatus::ApplyFailed,
                            "runtime.routing.problem.rollback_failed",
                            NodeProblemCode::Unavailable,
                            format!(
                                "网络 [{network_name}] 路由热更新失败且旧路由回滚失败: {failure_message}；{rollback_error}"
                            ),
                        )
                        .await;
                        RuntimeControlError::Persistence(format!(
                            "网络 [{network_name}] 路由热更新失败: {failure_message}；旧配置回滚未能确认: {rollback_error}"
                        ))
                    }
                });
            }
        };

        let mut updated = previous.clone();
        updated.routing_mode = command.mode;
        updated.exit_node = command.exit_node.clone();
        updated.exit_node_enabled = command.is_exit_node;

        let mut updated_roles = previous_roles.clone();
        set_role_enabled(
            &mut updated_roles,
            NodeNetworkRole::ExitNode,
            command.is_exit_node,
        );

        if !self
            .publish_role_projection(updated, updated_roles.clone())
            .await
        {
            let rollback =
                rollback_runtime_routing(&routing_control, &previous, self.timeouts.routing_update)
                    .await;
            // The failed publish did not replace an existing entry. Restore the
            // previous projection if a concurrent state transition removed it
            // while the runtime update was in flight.
            self.restore_role_projection(previous, previous_roles).await;
            return Err(match rollback {
                Ok(()) => {
                    self.record_routing_problem(
                        &network_name,
                        NodeRoutingSummaryStatus::ApplyFailed,
                        "runtime.routing.problem.publish_failed",
                        NodeProblemCode::Internal,
                        format!(
                            "网络 [{network_name}] 的权威路由摘要发布失败，已重新应用旧路由并回滚共享快照"
                        ),
                    )
                    .await;
                    RuntimeControlError::Persistence(format!(
                        "网络 [{network_name}] 的共享快照发布失败，运行态、持久化配置和共享快照已回滚"
                    ))
                }
                Err(rollback_error) => {
                    self.record_routing_problem(
                        &network_name,
                        NodeRoutingSummaryStatus::ApplyFailed,
                        "runtime.routing.problem.rollback_failed",
                        NodeProblemCode::Internal,
                        format!(
                            "网络 [{network_name}] 的权威路由摘要发布失败且运行态回滚失败: {rollback_error}"
                        ),
                    )
                    .await;
                    RuntimeControlError::Persistence(format!(
                        "网络 [{network_name}] 的共享快照发布失败，且运行态回滚失败: {rollback_error}"
                    ))
                }
            });
        }

        self.events.publish(NodeEventKind::RoutingChanged {
            network_name,
            mode: command.mode,
            exit_node: command.exit_node,
            is_exit_node: command.is_exit_node,
        });
        if updated_roles != previous_roles {
            self.events.publish(NodeEventKind::RoleChanged {
                network_name: command.network_name,
                role: NodeNetworkRole::ExitNode,
                enabled: command.is_exit_node,
                roles: updated_roles,
            });
        }
        self.refresh_local_exit_capability_state().await;

        Ok(result)
    }

    async fn set_network_role(
        &mut self,
        command: NodeSetNetworkRoleCommand,
    ) -> Result<RoleUpdateResult, RuntimeControlError> {
        self.reconcile_finished_sessions().await;

        match command.role {
            NodeNetworkRole::Member => {
                return Err(RuntimeControlError::Conflict(
                    "成员角色是节点网络内的基线能力，不能单独启用或禁用".into(),
                ));
            }
            NodeNetworkRole::Discovery => {
                return Err(RuntimeControlError::Unavailable(format!(
                    "{:?} 角色目前只能随内嵌协调服务整体创建/停止；尚未提供不重启网络即可独立切换该能力的后端，未修改任何状态",
                    command.role
                )));
            }
            NodeNetworkRole::Relay => return self.set_relay_role(command).await,
            NodeNetworkRole::ExitNode => {}
        }

        let network_name = command.network_name.clone();
        let routing_control = self
            .sessions
            .get(&network_name)
            .map(|session| session.routing_control.clone())
            .ok_or_else(|| RuntimeControlError::NotFound(format!("网络未运行: {network_name}")))?;

        let before = self.state.snapshot().await;
        let previous_config = before
            .configuration
            .networks
            .iter()
            .find(|network| network.network_name == network_name)
            .cloned()
            .ok_or_else(|| {
                RuntimeControlError::Persistence(format!(
                    "网络 [{network_name}] 的共享配置快照缺失，未应用角色更新"
                ))
            })?;
        let previous_roles = before
            .networks
            .iter()
            .find(|network| network.name == network_name)
            .map(|network| network.roles.clone())
            .ok_or_else(|| {
                RuntimeControlError::Persistence(format!(
                    "网络 [{network_name}] 的共享运行快照缺失，未应用角色更新"
                ))
            })?;
        ensure_member_role(&network_name, &previous_roles)?;

        let mut desired_roles = previous_roles.clone();
        set_role_enabled(&mut desired_roles, command.role, command.enabled);
        let routing_command = NodeUpdateRoutingCommand {
            network_name: network_name.clone(),
            mode: previous_config.routing_mode,
            exit_node: previous_config.exit_node.clone(),
            is_exit_node: desired_roles.contains(&NodeNetworkRole::ExitNode),
        };

        match request_runtime_routing_update(
            &routing_control,
            &routing_command,
            self.timeouts.routing_update,
        )
        .await
        {
            Ok(_) => {}
            Err(RuntimeRoutingRequestError::ControlClosed) => {
                self.record_routing_problem(
                    &network_name,
                    NodeRoutingSummaryStatus::ApplyFailed,
                    "runtime.routing.problem.hot_update_failed",
                    NodeProblemCode::Unavailable,
                    format!("网络 [{network_name}] 的角色运行时控制通道已关闭，未应用新的角色策略"),
                )
                .await;
                return Err(RuntimeControlError::NotFound(format!(
                    "网络 [{network_name}] 的运行时控制通道已关闭"
                )));
            }
            Err(failure) => {
                let failure_message = describe_routing_request_failure(&failure);
                let rollback = rollback_runtime_routing(
                    &routing_control,
                    &previous_config,
                    self.timeouts.routing_update,
                )
                .await;
                return Err(match rollback {
                    Ok(()) => {
                        self.record_routing_problem(
                            &network_name,
                            NodeRoutingSummaryStatus::ApplyFailed,
                            "runtime.routing.problem.hot_update_failed",
                            NodeProblemCode::Unavailable,
                            format!(
                                "网络 [{network_name}] 角色运行态更新失败: {failure_message}；旧路由已重新应用"
                            ),
                        )
                        .await;
                        RuntimeControlError::Persistence(format!(
                            "网络 [{network_name}] 角色运行态更新失败: {failure_message}；已重新应用旧配置，运行态与持久化配置已回滚"
                        ))
                    }
                    Err(rollback_error) => {
                        self.record_routing_problem(
                            &network_name,
                            NodeRoutingSummaryStatus::ApplyFailed,
                            "runtime.routing.problem.rollback_failed",
                            NodeProblemCode::Unavailable,
                            format!(
                                "网络 [{network_name}] 角色运行态更新失败且旧路由回滚失败: {failure_message}；{rollback_error}"
                            ),
                        )
                        .await;
                        RuntimeControlError::Persistence(format!(
                            "网络 [{network_name}] 角色运行态更新失败: {failure_message}；旧配置回滚未能确认: {rollback_error}"
                        ))
                    }
                });
            }
        }

        let mut updated_config = previous_config.clone();
        updated_config.exit_node_enabled = desired_roles.contains(&NodeNetworkRole::ExitNode);
        if !self
            .publish_role_projection(updated_config, desired_roles.clone())
            .await
        {
            let rollback = rollback_runtime_routing(
                &routing_control,
                &previous_config,
                self.timeouts.routing_update,
            )
            .await;
            self.restore_role_projection(previous_config, previous_roles.clone())
                .await;
            return Err(match rollback {
                Ok(()) => {
                    self.record_routing_problem(
                        &network_name,
                        NodeRoutingSummaryStatus::ApplyFailed,
                        "runtime.routing.problem.publish_failed",
                        NodeProblemCode::Internal,
                        format!(
                            "网络 [{network_name}] 的角色摘要发布失败，已重新应用旧路由并回滚共享快照"
                        ),
                    )
                    .await;
                    RuntimeControlError::Persistence(format!(
                        "网络 [{network_name}] 的角色共享快照发布失败，运行态、持久化配置和共享快照已回滚"
                    ))
                }
                Err(rollback_error) => {
                    self.record_routing_problem(
                        &network_name,
                        NodeRoutingSummaryStatus::ApplyFailed,
                        "runtime.routing.problem.rollback_failed",
                        NodeProblemCode::Internal,
                        format!(
                            "网络 [{network_name}] 的角色摘要发布失败且运行态回滚失败: {rollback_error}"
                        ),
                    )
                    .await;
                    RuntimeControlError::Persistence(format!(
                        "网络 [{network_name}] 的角色共享快照发布失败，且运行态回滚失败: {rollback_error}"
                    ))
                }
            });
        }

        self.events.publish(NodeEventKind::RoleChanged {
            network_name: network_name.clone(),
            role: command.role,
            enabled: command.enabled,
            roles: desired_roles.clone(),
        });
        self.refresh_local_exit_capability_state().await;

        Ok(RoleUpdateResult {
            network_name,
            role: command.role,
            enabled: command.enabled,
            roles: desired_roles,
        })
    }

    async fn set_relay_role(
        &mut self,
        command: NodeSetNetworkRoleCommand,
    ) -> Result<RoleUpdateResult, RuntimeControlError> {
        let network_name = command.network_name.clone();
        if !self.sessions.contains_key(&network_name) {
            return Err(RuntimeControlError::NotFound(format!(
                "网络未运行: {network_name}"
            )));
        }

        let persisted = self
            .configs
            .list_networks()
            .map_err(|error| {
                RuntimeControlError::Persistence(format!(
                    "网络 [{network_name}] 读取 Relay owner intent 失败: {error:#}"
                ))
            })?
            .into_iter()
            .find(|network| network.name == network_name)
            .ok_or_else(|| {
                RuntimeControlError::Persistence(format!(
                    "网络 [{network_name}] 的持久化元数据缺失，未应用 Relay 角色更新"
                ))
            })?;
        if !persisted.owns_embedded_services {
            return Err(RuntimeControlError::Unavailable(format!(
                "网络 [{network_name}] 不由本机托管内嵌服务，不能切换 Relay"
            )));
        }

        let services = self.services.list().await.map_err(map_service_error)?;
        if !services
            .iter()
            .any(|service| service.network_name == network_name)
        {
            return Err(RuntimeControlError::NotFound(format!(
                "网络 [{network_name}] 的内嵌服务未运行，无法切换 Relay"
            )));
        }

        let before = self.state.snapshot().await;
        let previous_config = before
            .configuration
            .networks
            .iter()
            .find(|network| network.network_name == network_name)
            .cloned()
            .ok_or_else(|| {
                RuntimeControlError::Persistence(format!(
                    "网络 [{network_name}] 的共享配置快照缺失，未应用 Relay 角色更新"
                ))
            })?;
        let previous_roles = before
            .networks
            .iter()
            .find(|network| network.name == network_name)
            .map(|network| network.roles.clone())
            .ok_or_else(|| {
                RuntimeControlError::Persistence(format!(
                    "网络 [{network_name}] 的共享运行快照缺失，未应用 Relay 角色更新"
                ))
            })?;
        ensure_member_role(&network_name, &previous_roles)?;
        let previous_relay = before
            .embedded_services
            .iter()
            .find(|service| service.network_name == network_name)
            .cloned()
            .ok_or_else(|| {
                RuntimeControlError::Persistence(format!(
                    "网络 [{network_name}] 的内嵌服务快照缺失，未应用 Relay 角色更新"
                ))
            })?;

        let mut desired_roles = previous_roles.clone();
        set_role_enabled(&mut desired_roles, NodeNetworkRole::Relay, command.enabled);

        let transition_status = if command.enabled {
            NodeEmbeddedServiceStatus::Starting
        } else {
            NodeEmbeddedServiceStatus::Stopping
        };
        self.record_embedded_relay_status(&network_name, transition_status, None)
            .await;
        let updated = if command.enabled {
            self.services.enable_relay(&network_name).await
        } else {
            self.services.disable_relay(&network_name).await
        };
        let updated = match updated {
            Ok(updated) => updated,
            Err(error) => {
                let failure_health = match &error {
                    EmbeddedServiceError::RelayStartFailed { health, .. }
                    | EmbeddedServiceError::RelayStopFailed { health, .. } => {
                        Some((**health).clone())
                    }
                    _ => None,
                };
                self.record_embedded_relay_projection(
                    &network_name,
                    NodeEmbeddedServiceStatus::Failed,
                    None,
                    failure_health,
                    None,
                    None,
                )
                .await;
                return Err(map_service_error(error));
            }
        };

        let relay_port = updated
            .relay_addr
            .map(|addr| addr.port())
            .or(previous_relay.relay_addr.map(|addr| addr.port()));
        if let Err(error) =
            self.configs
                .set_embedded_relay_intent(&network_name, command.enabled, relay_port)
        {
            let rollback = self
                .rollback_relay_runtime(&network_name, !command.enabled)
                .await;
            self.restore_relay_projection(&network_name, &previous_relay)
                .await;
            return Err(match rollback {
                Ok(()) => RuntimeControlError::Persistence(format!(
                    "网络 [{network_name}] 的 Relay owner intent 持久化失败: {error:#}；运行态已回滚"
                )),
                Err(rollback_error) => RuntimeControlError::Persistence(format!(
                    "网络 [{network_name}] 的 Relay owner intent 持久化失败: {error:#}；运行态回滚失败: {rollback_error}"
                )),
            });
        }

        if !self
            .publish_network_roles(&network_name, desired_roles.clone())
            .await
        {
            let runtime_rollback = self
                .rollback_relay_runtime(&network_name, !command.enabled)
                .await;
            let persistence_rollback = self.configs.set_embedded_relay_intent(
                &network_name,
                persisted.relay_enabled,
                persisted.relay_port,
            );
            self.restore_role_projection(previous_config, previous_roles.clone())
                .await;
            self.restore_relay_projection(&network_name, &previous_relay)
                .await;
            return Err(match (runtime_rollback, persistence_rollback) {
                (Ok(()), Ok(())) => RuntimeControlError::Persistence(format!(
                    "网络 [{network_name}] 的 Relay 角色共享快照发布失败；运行态、owner intent 和角色已回滚"
                )),
                (runtime_result, persistence_result) => RuntimeControlError::Persistence(format!(
                    "网络 [{network_name}] 的 Relay 角色共享快照发布失败；运行态回滚: {}；owner intent 回滚: {}",
                    runtime_result
                        .err()
                        .map_or_else(|| "成功".into(), |error| error.to_string()),
                    persistence_result
                        .err()
                        .map_or_else(|| "成功".into(), |error| format!("{error:#}"))
                )),
            });
        }

        self.record_embedded_relay_snapshot(&updated).await;
        if desired_roles != previous_roles {
            self.events.publish(NodeEventKind::RoleChanged {
                network_name: network_name.clone(),
                role: NodeNetworkRole::Relay,
                enabled: command.enabled,
                roles: desired_roles.clone(),
            });
        }

        Ok(RoleUpdateResult {
            network_name,
            role: NodeNetworkRole::Relay,
            enabled: command.enabled,
            roles: desired_roles,
        })
    }

    async fn rollback_relay_runtime(
        &self,
        network_name: &str,
        enabled: bool,
    ) -> Result<(), RuntimeControlError> {
        let result = if enabled {
            self.services.enable_relay(network_name).await.map(|_| ())
        } else {
            self.services.disable_relay(network_name).await.map(|_| ())
        };
        result.map_err(map_service_error)
    }

    async fn restore_relay_projection(
        &self,
        network_name: &str,
        service: &NodeEmbeddedServiceSnapshot,
    ) {
        self.record_embedded_relay_projection(
            network_name,
            service.relay_status,
            service.relay_addr,
            service.relay_health.clone(),
            service.relay_authorization.clone(),
            service.relay_governance.clone(),
        )
        .await;
    }

    async fn stop(
        &mut self,
        networks: Vec<String>,
        all: bool,
    ) -> Result<StopSummary, RuntimeControlError> {
        let deadline = Instant::now() + self.timeouts.stop;
        self.stop_until(networks, all, true, deadline).await
    }

    async fn stop_until(
        &mut self,
        networks: Vec<String>,
        all: bool,
        stop_embedded_services: bool,
        deadline: Instant,
    ) -> Result<StopSummary, RuntimeControlError> {
        self.reconcile_finished_sessions().await;
        let services = if stop_embedded_services {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(RuntimeControlError::Timeout(
                    "stop/timeout: 在枚举待停止网络前已超过期限".into(),
                ));
            }
            match timeout(remaining, self.services.list()).await {
                Ok(Ok(services)) => services
                    .into_iter()
                    .map(|service| service.network_name)
                    .collect::<BTreeSet<_>>(),
                Ok(Err(error)) => return Err(map_service_error(error)),
                Err(_) => {
                    return Err(RuntimeControlError::Timeout(
                        "stop/timeout: 枚举内嵌服务超过停止期限".into(),
                    ));
                }
            }
        } else {
            BTreeSet::new()
        };
        let targets = if all {
            self.sessions
                .keys()
                .cloned()
                .chain(services.iter().cloned())
                .chain(self.cleanup_failures.keys().cloned())
                .chain(self.completed_cleanup.keys().cloned())
                .collect::<BTreeSet<_>>()
        } else {
            let requested = networks.into_iter().collect::<BTreeSet<_>>();
            let missing = requested
                .iter()
                .filter(|name| {
                    !self.sessions.contains_key(*name)
                        && !services.contains(*name)
                        && !self.cleanup_failures.contains_key(*name)
                        && !self.completed_cleanup.contains_key(*name)
                })
                .cloned()
                .collect::<Vec<_>>();
            if !missing.is_empty() {
                return Err(RuntimeControlError::NotFound(format!(
                    "网络未运行: {}",
                    missing.join(", ")
                )));
            }
            requested
        };

        // Send every shutdown signal before waiting. This lets independent
        // network actors and owned services stop concurrently under one
        // manager-wide deadline instead of multiplying the timeout by target
        // count.
        let mut pending_sessions = Vec::new();
        let mut service_targets = Vec::new();
        for name in &targets {
            if let Some(session) = self.sessions.remove(name) {
                self.mark_network_status(Some(name), NodeNetworkStatus::Stopping)
                    .await;
                let accepted = session.shutdown.send(true).is_ok();
                pending_sessions.push((name.clone(), session, accepted));
            }
            let has_embedded_service = services.contains(name);
            if has_embedded_service {
                self.mark_embedded_service_status(name, NodeEmbeddedServiceStatus::Stopping)
                    .await;
                service_targets.push(name.clone());
            }
        }

        let services = self.services.clone();
        let session_waits =
            pending_sessions
                .into_iter()
                .map(|(network_name, session, accepted)| async move {
                    wait_for_session_shutdown(network_name.clone(), session, accepted, deadline)
                        .await
                });
        let service_waits = service_targets.into_iter().map(|network_name| {
            let services = services.clone();
            async move {
                let result = stop_embedded_service_until(&services, &network_name, deadline).await;
                (network_name, result)
            }
        });
        let (session_results, service_results) =
            tokio::join!(join_all(session_waits), join_all(service_waits));

        let mut failures = BTreeMap::<String, Vec<String>>::new();
        let mut runtime_outcomes = BTreeMap::new();
        let mut clean_session_names = BTreeSet::new();
        let mut observed_session_names = BTreeSet::new();
        for session_result in session_results {
            let network_name = session_result.network_name;
            observed_session_names.insert(network_name.clone());
            if let Some(session) = session_result.retry_session {
                // The task is still running its bounded cleanup. Keep the
                // session so a later stop can observe the real outcome.
                self.sessions.insert(network_name.clone(), session);
                let message = session_result.result.err().map_or_else(
                    || "停止任务未返回清理结果".into(),
                    |error| error.to_string(),
                );
                failures
                    .entry(network_name.clone())
                    .or_default()
                    .push(message.clone());
                let mut outcome = RuntimeCleanupOutcome::default();
                outcome.record("运行时清理", CleanupStatus::Unknown(message));
                self.cleanup_failures
                    .insert(network_name.clone(), outcome.clone());
                runtime_outcomes.insert(network_name.clone(), outcome);
                self.mark_network_status(Some(&network_name), NodeNetworkStatus::Failed)
                    .await;
                continue;
            }

            match session_result.result {
                Ok(exit) => {
                    if exit.cleanup.is_clean() {
                        clean_session_names.insert(network_name.clone());
                        self.cleanup_failures.remove(&network_name);
                        self.retained_owners.remove(&network_name);
                    } else {
                        if let Some(owner) = exit.retained_owner {
                            self.retained_owners.insert(network_name.clone(), owner);
                        }
                        failures
                            .entry(network_name.clone())
                            .or_default()
                            .push(format!("资源清理未完成: {}", exit.cleanup.summary()));
                        self.cleanup_failures
                            .insert(network_name.clone(), exit.cleanup.clone());
                        self.mark_network_status(Some(&network_name), NodeNetworkStatus::Failed)
                            .await;
                    }
                    runtime_outcomes.insert(network_name, exit.cleanup);
                }
                Err(error) => {
                    let message = error.to_string();
                    failures
                        .entry(network_name.clone())
                        .or_default()
                        .push(message.clone());
                    let mut outcome = RuntimeCleanupOutcome::default();
                    outcome.record("运行时清理", CleanupStatus::Unknown(message));
                    self.cleanup_failures
                        .insert(network_name.clone(), outcome.clone());
                    runtime_outcomes.insert(network_name.clone(), outcome);
                    self.mark_network_status(Some(&network_name), NodeNetworkStatus::Failed)
                        .await;
                }
            }
        }
        for name in &targets {
            if observed_session_names.contains(name) {
                continue;
            }
            if let Some(outcome) = self.completed_cleanup.remove(name) {
                clean_session_names.insert(name.clone());
                runtime_outcomes.insert(name.clone(), outcome);
                continue;
            }
            if let Some(outcome) = self.cleanup_failures.get(name).cloned() {
                failures
                    .entry(name.clone())
                    .or_default()
                    .push(format!("资源清理仍未完成: {}", outcome.summary()));
                runtime_outcomes.insert(name.clone(), outcome);
                self.mark_network_status(Some(name), NodeNetworkStatus::Failed)
                    .await;
            }
        }

        self.finish_clean_networks(&clean_session_names).await;
        for (network_name, result) in service_results {
            match result {
                Ok(()) => {
                    self.mark_embedded_service_status(
                        &network_name,
                        NodeEmbeddedServiceStatus::Stopped,
                    )
                    .await;
                }
                Err(error) => {
                    self.mark_embedded_service_status(
                        &network_name,
                        NodeEmbeddedServiceStatus::Failed,
                    )
                    .await;
                    failures
                        .entry(network_name)
                        .or_default()
                        .push(error.to_string());
                }
            }
        }

        let target_names = targets.iter().cloned().collect::<Vec<_>>();
        let mut stopped = Vec::with_capacity(targets.len());
        let mut failed = Vec::new();
        for name in targets {
            if let Some(mut messages) = failures.remove(&name) {
                messages.sort();
                messages.dedup();
                failed.push(StopFailure {
                    network_name: name,
                    message: messages.join("; "),
                });
            } else {
                stopped.push(name);
            }
        }

        let mut cleanup = self.inspect_cleanup(&target_names).await?;
        cleanup.runtime_outcomes = runtime_outcomes;
        self.publish_stop_lifecycle(&failed).await;
        self.refresh_local_exit_capability_state_with_deadline(deadline)
            .await;
        Ok(StopSummary {
            stopped,
            failed,
            cleanup,
        })
    }

    async fn restart(
        &mut self,
        networks: Vec<String>,
        all: bool,
    ) -> Result<RestartSummary, RuntimeControlError> {
        self.reconcile_finished_sessions().await;
        let deadline = Instant::now() + self.timeouts.restart;
        let targets = if all {
            self.sessions
                .keys()
                .chain(self.cleanup_failures.keys())
                .chain(self.completed_cleanup.keys())
                .cloned()
                .collect::<BTreeSet<_>>()
        } else {
            let requested = networks.into_iter().collect::<BTreeSet<_>>();
            let missing = requested
                .iter()
                .filter(|name| {
                    !self.sessions.contains_key(*name)
                        && !self.cleanup_failures.contains_key(*name)
                        && !self.completed_cleanup.contains_key(*name)
                })
                .cloned()
                .collect::<Vec<_>>();
            if !missing.is_empty() {
                return Err(RuntimeControlError::NotFound(format!(
                    "网络未运行，无法重启: {}",
                    missing.join(", ")
                )));
            }
            requested
        };

        if targets.is_empty() {
            return Ok(RestartSummary::default());
        }
        let names = targets.into_iter().collect::<Vec<_>>();

        // Load and validate every target before changing the live runtime. A
        // malformed persisted file therefore leaves the old sessions untouched.
        let plan = NodeRuntimePlan::load(&self.configs, NodeNetworkSelection::Named(names.clone()))
            .map_err(|error| {
                RuntimeControlError::Persistence(format!(
                    "restart/{}: 加载重启配置失败，现有运行态未改变: {error:#}",
                    RestartPhase::PersistLoad.as_str()
                ))
            })?;
        if Instant::now() >= deadline {
            return Err(RuntimeControlError::Timeout(
                "restart/timeout: 在 persist_load 阶段超过期限".into(),
            ));
        }

        let mut summary = RestartSummary {
            requested: names.len(),
            ..RestartSummary::default()
        };
        let stopped_summary = self
            .stop_until(names.clone(), false, false, deadline)
            .await?;
        let mut stopped = stopped_summary
            .stopped
            .iter()
            .cloned()
            .collect::<BTreeSet<_>>();
        for failure in stopped_summary.failed {
            summary.failed.push(RestartFailure {
                network_name: failure.network_name,
                phase: RestartPhase::Stop,
                message: failure.message,
                stage: None,
            });
        }

        summary.cleanup = self.inspect_cleanup(&names).await?;
        summary.cleanup.runtime_outcomes = stopped_summary.cleanup.runtime_outcomes;
        for name in &names {
            if !summary.cleanup.blocks_restart(name) {
                continue;
            }
            stopped.remove(name);
            summary
                .failed
                .retain(|failure| failure.network_name != *name);
            summary.failed.push(RestartFailure {
                network_name: name.clone(),
                phase: RestartPhase::Cleanup,
                message: cleanup_block_message(&summary.cleanup, name),
                stage: None,
            });
        }

        for spec in plan.into_networks() {
            let network_name = spec.name.clone();
            if !stopped.contains(&network_name) {
                continue;
            }
            match self.start_spec_until(spec, deadline).await {
                Ok(()) => summary.restarted.push(network_name),
                Err(error) => summary.failed.push(RestartFailure {
                    network_name,
                    phase: RestartPhase::Start,
                    message: error.to_string(),
                    stage: error.network_setup_stage(),
                }),
            }
        }

        self.publish_restart_lifecycle(&summary).await;
        Ok(summary)
    }

    async fn rebuild_tun(
        &mut self,
        network_name: String,
    ) -> Result<RestartSummary, RuntimeControlError> {
        let capabilities = self.state.snapshot().await.tun_capabilities;
        if !capabilities.can_rebuild {
            let reason = capabilities
                .blocking_reason()
                .unwrap_or(crate::tun::TunCapabilityReason::Unknown);
            let stage = reason.network_setup_stage();
            return Err(RuntimeControlError::NetworkSetup {
                stage,
                message: stage.fallback_message().into(),
            });
        }

        self.restart(vec![network_name], false).await
    }

    async fn cleanup_residuals(
        &mut self,
        networks: Vec<String>,
        all: bool,
    ) -> Result<StopSummary, RuntimeControlError> {
        self.reconcile_finished_sessions().await;
        let requested = networks.into_iter().collect::<BTreeSet<_>>();
        let targets = if all {
            self.retained_owners
                .keys()
                .chain(self.cleanup_failures.keys())
                .cloned()
                .collect::<BTreeSet<_>>()
        } else {
            let running = requested
                .iter()
                .filter(|name| self.sessions.contains_key(*name))
                .cloned()
                .collect::<Vec<_>>();
            if !running.is_empty() {
                return Err(RuntimeControlError::Conflict(format!(
                    "以下网络仍在运行，必须先等待运行会话退出再清理残留: {}",
                    running.join(", ")
                )));
            }
            let missing = requested
                .iter()
                .filter(|name| {
                    !self.retained_owners.contains_key(*name)
                        && !self.cleanup_failures.contains_key(*name)
                })
                .cloned()
                .collect::<Vec<_>>();
            if !missing.is_empty() {
                return Err(RuntimeControlError::NotFound(format!(
                    "以下网络没有可重试的运行时残留所有者: {}",
                    missing.join(", ")
                )));
            }
            requested
        };

        let deadline = Instant::now() + self.timeouts.stop;
        let target_names = targets.iter().cloned().collect::<Vec<_>>();
        let mut cleaned = BTreeSet::new();
        let mut failed = Vec::new();
        let mut runtime_outcomes = BTreeMap::new();

        for network_name in targets {
            if self.sessions.contains_key(&network_name) {
                failed.push(StopFailure {
                    network_name: network_name.clone(),
                    message: "运行会话尚未退出，拒绝绕过会话所有权直接清理残留".into(),
                });
                continue;
            }
            let Some(owner) = self.retained_owners.get_mut(&network_name) else {
                let message = self.cleanup_failures.get(&network_name).map_or_else(
                    || "运行时资源所有者已丢失，无法安全重试清理".into(),
                    |outcome| {
                        format!(
                            "运行时资源所有者已丢失，进程重启后不可恢复；最后结果: {}",
                            outcome.summary()
                        )
                    },
                );
                failed.push(StopFailure {
                    network_name: network_name.clone(),
                    message,
                });
                continue;
            };

            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                failed.push(StopFailure {
                    network_name: network_name.clone(),
                    message: "cleanup/timeout: 在清理该网络前已超过期限".into(),
                });
                continue;
            }

            let (outcome, timed_out) = match timeout(remaining, owner.shutdown()).await {
                Ok(outcome) => (outcome, false),
                Err(_) => {
                    let mut outcome = RuntimeCleanupOutcome::default();
                    outcome.record(
                        "运行时清理",
                        CleanupStatus::Deferred("重试在停止期限内未完成".into()),
                    );
                    (outcome, true)
                }
            };
            runtime_outcomes.insert(network_name.clone(), outcome.clone());
            if outcome.is_clean() {
                self.retained_owners.remove(&network_name);
                self.cleanup_failures.remove(&network_name);
                self.completed_cleanup.insert(network_name.clone(), outcome);
                cleaned.insert(network_name);
            } else {
                self.cleanup_failures
                    .insert(network_name.clone(), outcome.clone());
                self.completed_cleanup.remove(&network_name);
                failed.push(StopFailure {
                    network_name: network_name.clone(),
                    message: if timed_out {
                        format!("cleanup/timeout: 资源清理重试未完成: {}", outcome.summary())
                    } else {
                        format!("资源清理重试未完成: {}", outcome.summary())
                    },
                });
                self.mark_network_status(Some(&network_name), NodeNetworkStatus::Failed)
                    .await;
            }
        }

        self.finish_clean_networks(&cleaned).await;
        let mut cleanup = self.inspect_cleanup(&target_names).await?;
        cleanup.runtime_outcomes = runtime_outcomes;
        self.publish_dynamic_lifecycle().await;
        Ok(StopSummary {
            stopped: cleaned.into_iter().collect(),
            failed,
            cleanup,
        })
    }

    async fn diagnostics_cancellable(
        &mut self,
        cancel: watch::Receiver<bool>,
    ) -> Result<NodeDiagnosticsSnapshot, RuntimeControlError> {
        let result =
            run_diagnostic_collection(self.timeouts.diagnostics, cancel, self.diagnostics()).await;

        // Diagnostics currently use only in-memory snapshots. Keep this explicit
        // completion step so cancellation and timeout cannot mark an operation
        // terminal before actor-owned collection state has been released.
        self.finish_diagnostic_run().await;
        result
    }

    async fn finish_diagnostic_run(&mut self) {
        self.reconcile_finished_sessions().await;
    }

    async fn diagnostics(&mut self) -> Result<NodeDiagnosticsSnapshot, RuntimeControlError> {
        let started_at_unix_ms = unix_time_ms();
        let diagnostic_id = next_diagnostic_id(started_at_unix_ms);
        self.reconcile_finished_sessions().await;
        let lifecycle = self.lifecycle.snapshot().await;
        let snapshot = self.state.snapshot().await;
        let mut names = self
            .sessions
            .keys()
            .chain(self.retained_owners.keys())
            .chain(self.cleanup_failures.keys())
            .chain(self.completed_cleanup.keys())
            .cloned()
            .collect::<BTreeSet<_>>();
        names.extend(
            self.registry
                .get_all()
                .await
                .into_iter()
                .map(|session| session.network_name),
        );
        names.extend(snapshot.networks.iter().map(|network| network.name.clone()));
        names.extend(
            snapshot
                .configuration
                .networks
                .iter()
                .map(|network| network.network_name.clone()),
        );
        names.extend(snapshot.peers.iter().map(|peer| peer.network_name.clone()));
        names.extend(
            self.services
                .list()
                .await
                .map_err(map_service_error)?
                .into_iter()
                .map(|service| service.network_name),
        );
        let names = names.into_iter().collect::<Vec<_>>();
        let mut cleanup = self.inspect_cleanup(&names).await?;
        let mut outcomes = self.completed_cleanup.clone();
        outcomes.extend(self.cleanup_failures.clone());
        cleanup.runtime_outcomes = outcomes;

        let retained_owner_networks = self.retained_owners.keys().cloned().collect::<Vec<_>>();
        let cleanup_candidate_networks = self
            .retained_owners
            .keys()
            .chain(self.cleanup_failures.keys())
            .cloned()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let residue = cleanup.to_public();
        let checks = build_diagnostic_checks(&snapshot, &residue, started_at_unix_ms);
        let recovery_actions = diagnostic_recovery_actions(&checks, &residue);
        let protected_networks = snapshot
            .networks
            .iter()
            .filter(|network| {
                matches!(
                    network.status,
                    NodeNetworkStatus::Online | NodeNetworkStatus::Degraded
                )
            })
            .map(|network| network.name.as_str())
            .collect::<BTreeSet<_>>();
        let protected_interfaces = self
            .sessions
            .iter()
            .filter(|(network_name, _)| protected_networks.contains(network_name.as_str()))
            .map(|(_, session)| session.tun_name.clone())
            .collect::<Vec<_>>();
        let leak_detection = collect_leak_detection(
            LeakDetectionContext::new(
                snapshot.traffic_session.state == TrafficProtectionState::Protected,
                protected_interfaces,
            ),
            unix_time_ms(),
        )
        .await;
        let completed_at_unix_ms = unix_time_ms();
        let exit_health = build_exit_health_snapshot(
            &snapshot,
            &leak_detection,
            started_at_unix_ms,
            completed_at_unix_ms,
        );
        let exit_health_assessment = exit_health.assess(completed_at_unix_ms);
        let leak_detection_assessment = leak_detection.assess(completed_at_unix_ms);
        let sanitized_report = build_sanitized_diagnostic_report(
            &diagnostic_id,
            started_at_unix_ms,
            completed_at_unix_ms,
            false,
            &checks,
            &exit_health,
            &leak_detection,
            &recovery_actions,
            &residue,
        );
        Ok(NodeDiagnosticsSnapshot {
            diagnostic_id,
            started_at_unix_ms,
            completed_at_unix_ms,
            timed_out: false,
            lifecycle,
            retained_owner_networks,
            cleanup_candidate_networks,
            residue,
            checks,
            exit_health,
            exit_health_assessment,
            leak_detection,
            leak_detection_assessment,
            recovery_actions,
            sanitized_report,
        })
    }

    async fn finish_clean_networks(&self, clean_networks: &BTreeSet<String>) {
        if clean_networks.is_empty() {
            return;
        }
        join_all(
            clean_networks
                .iter()
                .map(|name| self.registry.unregister(name)),
        )
        .await;
        self.state
            .update(|snapshot| {
                for network in snapshot
                    .networks
                    .iter_mut()
                    .filter(|network| clean_networks.contains(network.name.as_str()))
                {
                    network.status = NodeNetworkStatus::Offline;
                    network.assigned_ipv6 = None;
                    network.assigned_ipv4 = None;
                    network.p2p_listen_addr = None;
                    network.uptime_secs = 0;
                    network.peer_count = 0;
                }
                snapshot
                    .peers
                    .retain(|peer| !clean_networks.contains(peer.network_name.as_str()));
                for summary in snapshot
                    .routing_summaries
                    .iter_mut()
                    .filter(|summary| clean_networks.contains(summary.network_name.as_str()))
                {
                    summary.status = NodeRoutingSummaryStatus::Stale;
                    summary.problem = Some(routing_problem(
                        &summary.network_name,
                        "runtime.routing.problem.stopped",
                        NodeProblemCode::Unavailable,
                        format!(
                            "网络 [{}] 已停止，最后一次成功路由应用不再生效",
                            summary.network_name
                        ),
                    ));
                }
            })
            .await;
    }

    async fn inspect_cleanup(
        &self,
        names: &[String],
    ) -> Result<RuntimeCleanupReport, RuntimeControlError> {
        let name_set = names.iter().map(String::as_str).collect::<BTreeSet<_>>();
        let listener_names = self
            .registry
            .get_all()
            .await
            .iter()
            .filter(|session| name_set.contains(session.network_name.as_str()))
            .map(|session| session.network_name.clone())
            .collect::<BTreeSet<_>>();
        let peer_names = self
            .state
            .snapshot()
            .await
            .peers
            .iter()
            .filter(|peer| name_set.contains(peer.network_name.as_str()))
            .map(|peer| peer.network_name.clone())
            .collect::<BTreeSet<_>>();
        let session_names = names
            .iter()
            .filter(|name| self.sessions.contains_key(*name))
            .cloned()
            .collect::<BTreeSet<_>>();
        let embedded_services = self
            .services
            .list()
            .await
            .map_err(map_service_error)?
            .iter()
            .filter(|service| name_set.contains(service.network_name.as_str()))
            .count();
        let residual_networks = session_names
            .iter()
            .chain(listener_names.iter())
            .chain(peer_names.iter())
            .cloned()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        Ok(RuntimeCleanupReport {
            sessions_remaining: session_names.len(),
            listeners_remaining: listener_names.len(),
            peers_remaining: peer_names.len(),
            embedded_services_remaining: embedded_services,
            configuration_retained: names.iter().all(|name| self.configs.network_exists(name)),
            runtime_outcomes: self
                .cleanup_failures
                .iter()
                .filter(|(name, _)| name_set.contains(name.as_str()))
                .map(|(name, outcome)| (name.clone(), outcome.clone()))
                .collect(),
            residual_networks,
        })
    }

    async fn shutdown(&mut self) -> Result<StopSummary, RuntimeControlError> {
        let summary = self.stop(Vec::new(), true).await?;
        match self.services.shutdown().await {
            Ok(_) | Err(EmbeddedServiceError::ActorClosed) => Ok(summary),
            Err(error) => Err(map_service_error(error)),
        }
    }

    async fn reconcile_finished_sessions(&mut self) {
        let finished = self
            .sessions
            .iter()
            .filter(|(_, session)| session.handle.is_finished())
            .map(|(network_name, _)| network_name.clone())
            .collect::<Vec<_>>();

        let had_finished_sessions = !finished.is_empty();
        for network_name in finished {
            let Some(session) = self.sessions.remove(&network_name) else {
                continue;
            };
            match session.handle.await {
                Ok(exit) if exit.cleanup.is_clean() => {
                    self.cleanup_failures.remove(&network_name);
                    self.retained_owners.remove(&network_name);
                    self.completed_cleanup
                        .insert(network_name.clone(), exit.cleanup);
                    self.registry.unregister(&network_name).await;
                }
                Ok(exit) => {
                    if let Some(owner) = exit.retained_owner {
                        self.retained_owners.insert(network_name.clone(), owner);
                    }
                    self.completed_cleanup.remove(&network_name);
                    self.cleanup_failures
                        .insert(network_name.clone(), exit.cleanup);
                    self.mark_network_status(Some(&network_name), NodeNetworkStatus::Failed)
                        .await;
                }
                Err(error) => {
                    let message = format!(
                        "网络 [{network_name}] 的运行时任务异常退出，清理结果未知: {error}"
                    );
                    let mut outcome = RuntimeCleanupOutcome::default();
                    outcome.record("运行时清理", CleanupStatus::Unknown(message));
                    self.completed_cleanup.remove(&network_name);
                    self.cleanup_failures.insert(network_name.clone(), outcome);
                    self.mark_network_status(Some(&network_name), NodeNetworkStatus::Failed)
                        .await;
                }
            }
        }
        if had_finished_sessions {
            self.refresh_local_exit_capability_state().await;
        }
    }

    async fn remove_created_network_persistence(&self, network_name: &str) {
        if let Ok(Some(joined)) = self
            .configs
            .joined_membership_for_network_name(network_name)
        {
            let _ = self
                .configs
                .remove_joined_membership(joined.membership.network_id.as_str());
        }
        let _ = self.configs.delete_network(network_name);
    }

    async fn rollback_created_network(&mut self, network_name: &str) {
        self.remove_created_network_persistence(network_name).await;
        self.stop_embedded_service(network_name).await;
        self.forget_network(network_name).await;
    }

    async fn rollback_created_network_after_service_failure(&mut self, network_name: &str) {
        self.remove_created_network_persistence(network_name).await;
        if let Err(error) = self.services.stop(network_name).await {
            if !matches!(error, EmbeddedServiceError::NotFound(_)) {
                tracing::warn!("网络 [{network_name}] 内嵌服务启动失败后的清理失败: {error}");
            }
        }
        self.forget_network(network_name).await;
        self.mark_embedded_service_status(network_name, NodeEmbeddedServiceStatus::Failed)
            .await;
    }

    async fn stop_embedded_service(&self, network_name: &str) {
        self.mark_embedded_service_status(network_name, NodeEmbeddedServiceStatus::Stopping)
            .await;
        match self.services.stop(network_name).await {
            Ok(_) | Err(EmbeddedServiceError::NotFound(_)) => {
                self.mark_embedded_service_status(network_name, NodeEmbeddedServiceStatus::Stopped)
                    .await;
            }
            Err(error) => {
                self.mark_embedded_service_status(network_name, NodeEmbeddedServiceStatus::Failed)
                    .await;
                tracing::warn!("网络 [{network_name}] 内嵌服务停止失败: {error}");
            }
        }
    }

    async fn forget_network(&self, network_name: &str) {
        self.state
            .update(|snapshot| {
                snapshot
                    .networks
                    .retain(|network| network.name != network_name);
                snapshot
                    .configuration
                    .networks
                    .retain(|network| network.network_name != network_name);
                snapshot
                    .memberships
                    .retain(|membership| membership.network_name != network_name);
                snapshot
                    .embedded_services
                    .retain(|service| service.network_name != network_name);
                snapshot
                    .peers
                    .retain(|peer| peer.network_name != network_name);
                snapshot
                    .routing_summaries
                    .retain(|summary| summary.network_name != network_name);
                if snapshot
                    .last_error
                    .as_ref()
                    .is_some_and(|problem| problem.network_name.as_deref() == Some(network_name))
                {
                    snapshot.last_error = None;
                }
            })
            .await;
    }

    async fn record_launch_failure(&self, network_name: &str, error: &RuntimeControlError) {
        self.mark_network_status(Some(network_name), NodeNetworkStatus::Failed)
            .await;
        self.state
            .update(|snapshot| {
                snapshot.last_error =
                    Some(runtime_control_problem(error).for_network(network_name));
            })
            .await;
        self.record_routing_problem(
            network_name,
            NodeRoutingSummaryStatus::ApplyFailed,
            "runtime.routing.problem.start_failed",
            NodeProblemCode::Unavailable,
            format!("网络 [{network_name}] 路由运行时启动失败，未发布生效中的策略"),
        )
        .await;
    }

    async fn record_identity(&self, identity: NodeIdentitySnapshot) {
        if let Some(existing) = self.state.snapshot().await.identity {
            if conflicting_identity_binding(&existing, &identity) {
                tracing::warn!(
                    "节点运行时检测到不同的网络身份，共享节点快照继续使用首次加载的身份"
                );
            }
            return;
        }

        self.state
            .update(|snapshot| {
                if snapshot.identity.is_none() {
                    snapshot.identity = Some(identity);
                }
            })
            .await;
    }

    async fn record_running_embedded_service(&self, service: &EmbeddedServiceSnapshot) {
        let snapshot = NodeEmbeddedServiceSnapshot::running_from_embedded(service);
        let network_name = snapshot.network_name.clone();
        let relay_status = snapshot.relay_status;
        self.state
            .update(|state| {
                if let Some(existing) = state
                    .embedded_services
                    .iter_mut()
                    .find(|existing| existing.network_name == snapshot.network_name)
                {
                    *existing = snapshot;
                } else {
                    state.embedded_services.push(snapshot);
                }
                state
                    .embedded_services
                    .sort_by(|left, right| left.network_name.cmp(&right.network_name));
            })
            .await;
        self.events.publish(NodeEventKind::EmbeddedServiceChanged {
            network_name: network_name.clone(),
            status: NodeEmbeddedServiceStatus::Running,
        });
        self.events.publish(NodeEventKind::RelayChanged {
            network_name,
            status: relay_status,
        });
    }

    async fn mark_embedded_service_status(
        &self,
        network_name: &str,
        status: NodeEmbeddedServiceStatus,
    ) {
        let relay_status = match status {
            NodeEmbeddedServiceStatus::Starting => NodeEmbeddedServiceStatus::Starting,
            NodeEmbeddedServiceStatus::Stopping => NodeEmbeddedServiceStatus::Stopping,
            NodeEmbeddedServiceStatus::Failed => NodeEmbeddedServiceStatus::Failed,
            NodeEmbeddedServiceStatus::Running | NodeEmbeddedServiceStatus::Stopped => status,
        };
        let snapshot = NodeEmbeddedServiceSnapshot {
            network_name: network_name.to_string(),
            status,
            signaling_addr: None,
            relay_status,
            relay_addr: None,
            relay_health: None,
            relay_authorization: None,
            relay_governance: None,
            coordinator_candidates: None,
        };
        self.state
            .update(|state| {
                if let Some(existing) = state
                    .embedded_services
                    .iter_mut()
                    .find(|existing| existing.network_name == snapshot.network_name)
                {
                    *existing = snapshot;
                } else {
                    state.embedded_services.push(snapshot);
                }
                state
                    .embedded_services
                    .sort_by(|left, right| left.network_name.cmp(&right.network_name));
            })
            .await;
        self.events.publish(NodeEventKind::EmbeddedServiceChanged {
            network_name: network_name.to_string(),
            status,
        });
        self.events.publish(NodeEventKind::RelayChanged {
            network_name: network_name.to_string(),
            status: relay_status,
        });
    }

    /// Project only a Relay lifecycle transition. Embedded signaling remains
    /// independently observable and is deliberately not republished here.
    async fn record_embedded_relay_status(
        &self,
        network_name: &str,
        status: NodeEmbeddedServiceStatus,
        relay_addr: Option<SocketAddr>,
    ) {
        self.record_embedded_relay_projection(network_name, status, relay_addr, None, None, None)
            .await;
    }

    async fn record_embedded_relay_snapshot(&self, service: &EmbeddedServiceSnapshot) {
        let relay_status = match service.relay_health.as_ref().map(|health| health.lifecycle) {
            Some(fuckipv6_common::domain::RelayLifecycleState::Running) => {
                NodeEmbeddedServiceStatus::Running
            }
            Some(fuckipv6_common::domain::RelayLifecycleState::Starting) => {
                NodeEmbeddedServiceStatus::Starting
            }
            Some(fuckipv6_common::domain::RelayLifecycleState::Stopping) => {
                NodeEmbeddedServiceStatus::Stopping
            }
            Some(
                fuckipv6_common::domain::RelayLifecycleState::Failed
                | fuckipv6_common::domain::RelayLifecycleState::Unsupported,
            ) => NodeEmbeddedServiceStatus::Failed,
            Some(fuckipv6_common::domain::RelayLifecycleState::Stopped) => {
                if service.relay_addr.is_some() {
                    NodeEmbeddedServiceStatus::Failed
                } else {
                    NodeEmbeddedServiceStatus::Stopped
                }
            }
            None => {
                if service.relay_addr.is_some() {
                    NodeEmbeddedServiceStatus::Running
                } else {
                    NodeEmbeddedServiceStatus::Stopped
                }
            }
        };
        self.record_embedded_relay_projection(
            &service.network_name,
            relay_status,
            service.relay_addr,
            service.relay_health.clone(),
            service.relay_authorization.clone(),
            service.relay_governance.clone(),
        )
        .await;
    }

    #[allow(clippy::too_many_arguments)]
    async fn record_embedded_relay_projection(
        &self,
        network_name: &str,
        status: NodeEmbeddedServiceStatus,
        relay_addr: Option<SocketAddr>,
        relay_health: Option<RelayHealthSnapshot>,
        relay_authorization: Option<RelayAuthorizationAuditSnapshot>,
        relay_governance: Option<RelayGovernanceAuditSnapshot>,
    ) {
        let mut updated = false;
        self.state
            .update(|state| {
                if let Some(existing) = state
                    .embedded_services
                    .iter_mut()
                    .find(|existing| existing.network_name == network_name)
                {
                    existing.relay_status = status;
                    existing.relay_addr = relay_addr;
                    existing.relay_health = relay_health;
                    existing.relay_authorization = relay_authorization;
                    existing.relay_governance = relay_governance;
                    updated = true;
                }
            })
            .await;
        if updated {
            self.events.publish(NodeEventKind::RelayChanged {
                network_name: network_name.to_string(),
                status,
            });
        } else {
            tracing::warn!("网络 [{network_name}] 缺少内嵌服务快照，未发布 Relay 状态事件");
        }
    }

    async fn publish_started_routing(&self, config: &NodeNetworkConfigSnapshot) -> bool {
        let mut published = false;
        self.state
            .update(|snapshot| {
                let has_config = snapshot
                    .configuration
                    .networks
                    .iter()
                    .any(|network| network.network_name == config.network_name);
                let has_runtime = snapshot
                    .networks
                    .iter()
                    .any(|network| network.name == config.network_name);
                if !has_config || !has_runtime {
                    return;
                }
                replace_routing_summary(
                    snapshot,
                    routing_summary_with_status(
                        snapshot,
                        config,
                        NodeRoutingSummaryStatus::Current,
                        None,
                    ),
                );
                published = true;
            })
            .await;
        published
    }

    async fn record_routing_problem(
        &self,
        network_name: &str,
        status: NodeRoutingSummaryStatus,
        message_key: &'static str,
        code: NodeProblemCode,
        message: impl Into<String>,
    ) {
        let problem = routing_problem(network_name, message_key, code, message);
        self.state
            .update(|snapshot| {
                let config = snapshot
                    .configuration
                    .networks
                    .iter()
                    .find(|config| config.network_name == network_name)
                    .cloned();
                if let Some(config) = config {
                    replace_routing_summary(
                        snapshot,
                        routing_summary_with_status(snapshot, &config, status, Some(problem)),
                    );
                } else if let Some(summary) = snapshot
                    .routing_summaries
                    .iter_mut()
                    .find(|summary| summary.network_name == network_name)
                {
                    summary.status = status;
                    summary.problem = Some(problem);
                }
            })
            .await;
    }

    async fn record_network_config(&self, config: NodeNetworkConfigSnapshot) {
        self.state
            .update(|snapshot| {
                if let Some(existing) = snapshot
                    .configuration
                    .networks
                    .iter_mut()
                    .find(|existing| existing.network_name == config.network_name)
                {
                    *existing = config;
                } else {
                    snapshot.configuration.networks.push(config);
                }
            })
            .await;
    }

    async fn publish_role_projection(
        &self,
        config: NodeNetworkConfigSnapshot,
        roles: Vec<NodeNetworkRole>,
    ) -> bool {
        let mut published = false;
        self.state
            .update(|snapshot| {
                let has_config = snapshot
                    .configuration
                    .networks
                    .iter()
                    .any(|network| network.network_name == config.network_name);
                let has_runtime = snapshot
                    .networks
                    .iter()
                    .any(|network| network.name == config.network_name);
                if !has_config || !has_runtime {
                    return;
                }

                if let Some(existing) = snapshot
                    .configuration
                    .networks
                    .iter_mut()
                    .find(|network| network.network_name == config.network_name)
                {
                    *existing = config.clone();
                }
                if let Some(existing) = snapshot
                    .networks
                    .iter_mut()
                    .find(|network| network.name == config.network_name)
                {
                    existing.roles = roles.clone();
                }
                replace_routing_summary(
                    snapshot,
                    routing_summary_with_status(
                        snapshot,
                        &config,
                        NodeRoutingSummaryStatus::Current,
                        None,
                    ),
                );
                published = true;
            })
            .await;
        published
    }

    /// Update only the runtime role projection for capabilities whose owner
    /// intent is not represented by the Exit routing configuration.
    async fn publish_network_roles(&self, network_name: &str, roles: Vec<NodeNetworkRole>) -> bool {
        let mut published = false;
        self.state
            .update(|snapshot| {
                if let Some(existing) = snapshot
                    .networks
                    .iter_mut()
                    .find(|network| network.name == network_name)
                {
                    existing.roles = roles;
                    published = true;
                }
            })
            .await;
        published
    }

    async fn restore_role_projection(
        &self,
        config: NodeNetworkConfigSnapshot,
        roles: Vec<NodeNetworkRole>,
    ) {
        let network_name = config.network_name.clone();
        self.state
            .update(|snapshot| {
                if let Some(existing) = snapshot
                    .configuration
                    .networks
                    .iter_mut()
                    .find(|network| network.network_name == network_name)
                {
                    *existing = config;
                } else {
                    snapshot.configuration.networks.push(config);
                }
                if let Some(existing) = snapshot
                    .networks
                    .iter_mut()
                    .find(|network| network.name == network_name)
                {
                    existing.roles = roles;
                }
            })
            .await;
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

    async fn record_membership(
        &self,
        joined: &JoinedNetworkMembership,
        runtime: Option<NodeNetworkStatus>,
    ) {
        let membership = network_membership_snapshot(joined, runtime);
        self.state
            .update(|snapshot| {
                snapshot
                    .memberships
                    .retain(|existing| existing.network_id != membership.network_id);
                snapshot.memberships.push(membership);
                snapshot
                    .memberships
                    .sort_by(|left, right| left.network_name.cmp(&right.network_name));
            })
            .await;
    }

    async fn mark_network_status(&self, network_name: Option<&str>, status: NodeNetworkStatus) {
        let snapshot = self
            .state
            .update(|snapshot| {
                if status == NodeNetworkStatus::Offline {
                    snapshot
                        .peers
                        .retain(|peer| network_name.is_none_or(|name| peer.network_name != name));
                }
                for network in &mut snapshot.networks {
                    if network_name.is_none_or(|name| network.name == name) {
                        network.status = status;
                        if status == NodeNetworkStatus::Offline {
                            network.peer_count = 0;
                        }
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

    async fn publish_dynamic_lifecycle(&self) {
        if self.lifecycle.snapshot().await.phase != NodePhase::Running {
            return;
        }
        let configured = self
            .configs
            .list_networks()
            .map_or(0, |networks| networks.len());
        let snapshot = self.state.snapshot().await;
        let failed = snapshot
            .networks
            .iter()
            .filter(|network| network.status == NodeNetworkStatus::Failed)
            .map(|network| network.name.clone())
            .collect();
        let lifecycle = self
            .lifecycle
            .running_with_configuration(configured, self.sessions.len(), failed)
            .await;
        self.state.set_lifecycle(lifecycle).await;
    }

    async fn publish_restart_lifecycle(&self, summary: &RestartSummary) {
        let failed = summary
            .failed
            .iter()
            .map(|failure| failure.network_name.clone())
            .collect();
        let lifecycle = self
            .lifecycle
            .running_with_configuration(summary.requested, self.sessions.len(), failed)
            .await;
        self.state.set_lifecycle(lifecycle).await;
    }

    async fn publish_stop_lifecycle(&self, failures: &[StopFailure]) {
        if self.lifecycle.snapshot().await.phase != NodePhase::Running {
            return;
        }
        let configured = self
            .configs
            .list_networks()
            .map_or(0, |networks| networks.len());
        let failed = failures
            .iter()
            .map(|failure| failure.network_name.clone())
            .collect();
        let lifecycle = self
            .lifecycle
            .running_with_configuration(configured, self.sessions.len(), failed)
            .await;
        self.state.set_lifecycle(lifecycle).await;
    }
}

fn conflicting_identity_binding(
    existing: &NodeIdentitySnapshot,
    incoming: &NodeIdentitySnapshot,
) -> bool {
    existing.node_id != incoming.node_id
        || existing.public_key_fingerprint != incoming.public_key_fingerprint
}

struct SessionShutdownResult {
    network_name: String,
    result: Result<NetworkRuntimeExit, RuntimeControlError>,
    retry_session: Option<NetworkRuntimeSession>,
}

async fn wait_for_session_shutdown(
    network_name: String,
    session: NetworkRuntimeSession,
    accepted: bool,
    deadline: Instant,
) -> SessionShutdownResult {
    let NetworkRuntimeSession {
        network_id,
        signaling,
        coordinator_recovery,
        coordinator_emergency,
        coordinator_continuity,
        tun_name,
        routing_control,
        shutdown,
        mut handle,
    } = session;
    if !accepted {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return SessionShutdownResult {
                network_name: network_name.clone(),
                result: Err(RuntimeControlError::Timeout(format!(
                    "stop/timeout: 网络 [{network_name}] 的关闭通道已断开且任务未退出；清理结果未知"
                ))),
                retry_session: Some(NetworkRuntimeSession {
                    network_id: network_id.clone(),
                    signaling: signaling.clone(),
                    coordinator_recovery: coordinator_recovery.clone(),
                    coordinator_emergency: coordinator_emergency.clone(),
                    coordinator_continuity: coordinator_continuity.clone(),
                    tun_name: tun_name.clone(),
                    routing_control,
                    shutdown,
                    handle,
                }),
            };
        }
        return match timeout(remaining, &mut handle).await {
            Ok(Ok(outcome)) => SessionShutdownResult {
                network_name: network_name.clone(),
                result: Ok(outcome),
                retry_session: None,
            },
            Ok(Err(error)) => SessionShutdownResult {
                network_name: network_name.clone(),
                result: Err(RuntimeControlError::Unavailable(format!(
                    "stop/task_failed: 网络 [{network_name}] 的运行时任务异常退出: {error}"
                ))),
                retry_session: None,
            },
            Err(_) => SessionShutdownResult {
                network_name: network_name.clone(),
                result: Err(RuntimeControlError::Timeout(format!(
                    "stop/timeout: 网络 [{network_name}] 的关闭通道已断开且任务未退出；清理结果未知"
                ))),
                retry_session: Some(NetworkRuntimeSession {
                    network_id: network_id.clone(),
                    signaling: signaling.clone(),
                    coordinator_recovery: coordinator_recovery.clone(),
                    coordinator_emergency: coordinator_emergency.clone(),
                    coordinator_continuity: coordinator_continuity.clone(),
                    tun_name: tun_name.clone(),
                    routing_control,
                    shutdown,
                    handle,
                }),
            },
        };
    }

    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        return SessionShutdownResult {
            network_name: network_name.clone(),
            result: Err(RuntimeControlError::Timeout(format!(
                "stop/timeout: 网络 [{network_name}] 未在期限内退出；清理结果未知，任务继续执行清理"
            ))),
            retry_session: Some(NetworkRuntimeSession {
                network_id: network_id.clone(),
                signaling: signaling.clone(),
                coordinator_recovery: coordinator_recovery.clone(),
                coordinator_emergency: coordinator_emergency.clone(),
                coordinator_continuity: coordinator_continuity.clone(),
                tun_name: tun_name.clone(),
                routing_control,
                shutdown,
                handle,
            }),
        };
    }

    match timeout(remaining, &mut handle).await {
        Ok(Ok(outcome)) => SessionShutdownResult {
            network_name: network_name.clone(),
            result: Ok(outcome),
            retry_session: None,
        },
        Ok(Err(error)) => SessionShutdownResult {
            network_name: network_name.clone(),
            result: Err(RuntimeControlError::Unavailable(format!(
                "stop/task_failed: 网络 [{network_name}] 的运行时任务异常退出，清理结果未知: {error}"
            ))),
            retry_session: None,
        },
        Err(_) => SessionShutdownResult {
            network_name: network_name.clone(),
            result: Err(RuntimeControlError::Timeout(format!(
                "stop/timeout: 网络 [{network_name}] 未在期限内退出；清理结果未知，任务继续执行清理"
            ))),
            retry_session: Some(NetworkRuntimeSession {
                network_id: network_id.clone(),
                signaling: signaling.clone(),
                coordinator_recovery: coordinator_recovery.clone(),
                coordinator_emergency: coordinator_emergency.clone(),
                coordinator_continuity: coordinator_continuity.clone(),
                tun_name: tun_name.clone(),
                routing_control,
                shutdown,
                handle,
            }),
        },
    }
}

async fn stop_embedded_service_until(
    services: &EmbeddedServiceManager,
    network_name: &str,
    deadline: Instant,
) -> Result<(), RuntimeControlError> {
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        return Err(RuntimeControlError::Timeout(format!(
            "stop/timeout: 网络 [{network_name}] 的内嵌服务未在停止期限内退出"
        )));
    }

    match timeout(remaining, services.stop(network_name)).await {
        Ok(Ok(_)) | Ok(Err(EmbeddedServiceError::NotFound(_))) => Ok(()),
        Ok(Err(error)) => Err(map_service_error(error)),
        Err(_) => Err(RuntimeControlError::Timeout(format!(
            "stop/timeout: 网络 [{network_name}] 的内嵌服务未在停止期限内退出"
        ))),
    }
}

fn parse_join_target(target: &str) -> Result<(String, String), RuntimeControlError> {
    let target = target.trim();
    if target.starts_with(NetworkTicket::URI_SCHEME) {
        let ticket = NetworkTicket::parse(target).map_err(|error| {
            RuntimeControlError::Invalid(format!("解析网络邀请票据失败: {error:#}"))
        })?;
        return Ok((ticket.websocket_url(), ticket.network_name));
    }

    if target.starts_with("ws://") || target.starts_with("wss://") {
        return Ok((
            target.to_string(),
            ConfigManager::extract_network_name(target),
        ));
    }

    let coordinator_url = if target.contains(':') {
        format!("ws://{target}")
    } else {
        format!("ws://{target}:6666")
    };
    Ok((
        coordinator_url.clone(),
        ConfigManager::extract_network_name(&coordinator_url),
    ))
}

fn join_rollback_error(
    message: String,
    rollback_error: Option<RuntimeControlError>,
) -> RuntimeControlError {
    match rollback_error {
        Some(rollback_error) => RuntimeControlError::Persistence(format!(
            "{message}；且回滚未完成配置失败: {rollback_error}"
        )),
        None => RuntimeControlError::Launch(message),
    }
}

fn cleanup_block_message(report: &RuntimeCleanupReport, network_name: &str) -> String {
    let mut reasons = Vec::new();
    if report
        .residual_networks
        .iter()
        .any(|name| name == network_name)
    {
        reasons.push("仍检测到 session、listener 或 peer 残留".to_string());
    }
    if let Some(outcome) = report.runtime_outcomes.get(network_name) {
        reasons.extend(outcome.issues());
    }
    if reasons.is_empty() {
        "停止后的资源清理结果不干净".into()
    } else {
        reasons.join("; ")
    }
}

fn map_service_error(error: EmbeddedServiceError) -> RuntimeControlError {
    match error {
        EmbeddedServiceError::InvalidRequest(message) => RuntimeControlError::Invalid(message),
        EmbeddedServiceError::AlreadyRunning(name) => {
            RuntimeControlError::Conflict(format!("网络 [{name}] 的内嵌服务已在运行"))
        }
        EmbeddedServiceError::NotFound(name) => {
            RuntimeControlError::NotFound(format!("网络 [{name}] 的内嵌服务未运行"))
        }
        EmbeddedServiceError::StartFailed {
            network_name,
            message,
        } => RuntimeControlError::Launch(format!(
            "网络 [{network_name}] 的内嵌服务启动失败: {message}"
        )),
        EmbeddedServiceError::StopFailed {
            network_name,
            message,
        } => RuntimeControlError::Unavailable(format!(
            "网络 [{network_name}] 的内嵌服务停止失败: {message}"
        )),
        EmbeddedServiceError::ShutdownFailed { message } => {
            RuntimeControlError::Unavailable(format!("内嵌网络服务关闭未完全成功: {message}"))
        }
        EmbeddedServiceError::PersistenceFailed {
            network_name,
            message,
        } => RuntimeControlError::Unavailable(format!(
            "网络 [{network_name}] 的持久化管理操作失败: {message}"
        )),
        EmbeddedServiceError::OperationFailed {
            network_name,
            message,
        } => RuntimeControlError::Unavailable(format!(
            "网络 [{network_name}] 的管理操作失败: {message}"
        )),
        EmbeddedServiceError::RelayStartFailed {
            network_name,
            reason_code,
            message,
            ..
        } => RuntimeControlError::RelayLifecycleFailed {
            reason_code,
            message: format!("网络 [{network_name}] 的 Relay 启动失败: {message}"),
        },
        EmbeddedServiceError::RelayStopFailed {
            network_name,
            reason_code,
            message,
            ..
        } => RuntimeControlError::RelayLifecycleFailed {
            reason_code,
            message: format!("网络 [{network_name}] 的 Relay 停止失败: {message}"),
        },
        EmbeddedServiceError::ActorClosed => RuntimeControlError::ActorClosed,
    }
}

async fn request_runtime_routing_update(
    routing_control: &mpsc::UnboundedSender<ipc::RuntimeCommand>,
    command: &NodeUpdateRoutingCommand,
    wait: Duration,
) -> Result<ipc::RoutingUpdateResult, RuntimeRoutingRequestError> {
    let (reply, response) = oneshot::channel();
    routing_control
        .send(ipc::RuntimeCommand::UpdateRouting {
            mode: runtime_routing_mode_name(command.mode).into(),
            exit_node: command.exit_node.clone(),
            is_exit_node: command.is_exit_node,
            reply,
        })
        .map_err(|_| RuntimeRoutingRequestError::ControlClosed)?;

    match timeout(wait, response).await {
        Err(_) => Err(RuntimeRoutingRequestError::TimedOut(wait)),
        Ok(Err(_)) => Err(RuntimeRoutingRequestError::ResponseClosed),
        Ok(Ok(Ok(result))) => Ok(result),
        Ok(Ok(Err(error))) => Err(RuntimeRoutingRequestError::Rejected(error)),
    }
}

async fn rollback_runtime_routing(
    routing_control: &mpsc::UnboundedSender<ipc::RuntimeCommand>,
    previous: &NodeNetworkConfigSnapshot,
    wait: Duration,
) -> Result<(), RuntimeControlError> {
    let command = NodeUpdateRoutingCommand {
        network_name: previous.network_name.clone(),
        mode: previous.routing_mode,
        exit_node: previous.exit_node.clone(),
        is_exit_node: previous.exit_node_enabled,
    };
    request_runtime_routing_update(routing_control, &command, wait)
        .await
        .map(|_| ())
        .map_err(|error| {
            RuntimeControlError::Persistence(format!(
                "网络 [{}] 的旧路由回滚失败: {}",
                previous.network_name,
                describe_routing_request_failure(&error)
            ))
        })
}

fn describe_routing_request_failure(error: &RuntimeRoutingRequestError) -> String {
    match error {
        RuntimeRoutingRequestError::ControlClosed => "运行时控制通道已关闭".into(),
        RuntimeRoutingRequestError::TimedOut(wait) => {
            format!("等待运行时响应超时（{} 秒）", wait.as_secs_f64())
        }
        RuntimeRoutingRequestError::ResponseClosed => "运行时在完成前关闭了响应通道".into(),
        RuntimeRoutingRequestError::Rejected(message) => format!("运行时拒绝更新: {message}"),
        RuntimeRoutingRequestError::LocalExitRejected(error) => {
            format!("运行时拒绝本机 Exit 请求: {error}")
        }
    }
}

async fn request_runtime_local_exit(
    routing_control: &mpsc::UnboundedSender<ipc::RuntimeCommand>,
    enabled: bool,
    wait: Duration,
) -> Result<ipc::LocalExitRuntimeStatus, RuntimeRoutingRequestError> {
    let (reply, response) = oneshot::channel();
    routing_control
        .send(ipc::RuntimeCommand::SetLocalExitCapability { enabled, reply })
        .map_err(|_| RuntimeRoutingRequestError::ControlClosed)?;

    match timeout(wait, response).await {
        Err(_) => Err(RuntimeRoutingRequestError::TimedOut(wait)),
        Ok(Err(_)) => Err(RuntimeRoutingRequestError::ResponseClosed),
        Ok(Ok(Ok(result))) => Ok(result),
        Ok(Ok(Err(error))) => Err(RuntimeRoutingRequestError::LocalExitRejected(error)),
    }
}

async fn request_runtime_local_exit_status(
    routing_control: &mpsc::UnboundedSender<ipc::RuntimeCommand>,
    wait: Duration,
) -> Result<ipc::LocalExitRuntimeStatus, RuntimeRoutingRequestError> {
    let (reply, response) = oneshot::channel();
    routing_control
        .send(ipc::RuntimeCommand::LocalExitStatus { reply })
        .map_err(|_| RuntimeRoutingRequestError::ControlClosed)?;

    match timeout(wait, response).await {
        Err(_) => Err(RuntimeRoutingRequestError::TimedOut(wait)),
        Ok(Err(_)) => Err(RuntimeRoutingRequestError::ResponseClosed),
        Ok(Ok(Ok(result))) => Ok(result),
        Ok(Ok(Err(error))) => Err(RuntimeRoutingRequestError::LocalExitRejected(error)),
    }
}

pub(crate) fn local_exit_unsupported_error(
    capabilities: &ExitNodeCapabilities,
) -> Option<RuntimeControlError> {
    if capabilities.supported {
        return None;
    }
    let message = if capabilities.unsupported_reasons.is_empty() {
        "当前平台缺少可用的本机 Exit 数据面能力".to_string()
    } else {
        capabilities.unsupported_explanation()
    };
    if capabilities
        .unsupported_reasons
        .contains(&ExitNodeUnsupportedReason::PrivilegesRequired)
    {
        Some(RuntimeControlError::PermissionDenied(message))
    } else {
        Some(RuntimeControlError::Unsupported(message))
    }
}

pub(crate) fn runtime_control_problem(error: &RuntimeControlError) -> NodeProblem {
    NodeProblem::new(error.problem_code(), error.to_string())
        .with_message_key(error.message_key())
        .retryable(error.retryable())
}

fn local_exit_request_error(
    network_name: &str,
    error: &RuntimeRoutingRequestError,
) -> RuntimeControlError {
    match error {
        RuntimeRoutingRequestError::ControlClosed => RuntimeControlError::Unavailable(format!(
            "网络 [{network_name}] 的本机 Exit 运行时控制通道已关闭"
        )),
        RuntimeRoutingRequestError::TimedOut(wait) => RuntimeControlError::Timeout(format!(
            "网络 [{network_name}] 等待本机 Exit 启停响应超时（{} 秒）",
            wait.as_secs_f64()
        )),
        RuntimeRoutingRequestError::ResponseClosed => RuntimeControlError::Unavailable(format!(
            "网络 [{network_name}] 的运行时在完成本机 Exit 启停前关闭了响应通道"
        )),
        RuntimeRoutingRequestError::LocalExitRejected(error) => {
            local_exit_error_with_network(network_name, error.clone())
        }
        RuntimeRoutingRequestError::Rejected(message) => RuntimeControlError::Unavailable(format!(
            "网络 [{network_name}] 的运行时拒绝本机 Exit 启停: {message}"
        )),
    }
}

fn local_exit_error_with_network(
    network_name: &str,
    error: RuntimeControlError,
) -> RuntimeControlError {
    let context = format!("网络 [{network_name}] 的本机 Exit 启停");
    match error {
        RuntimeControlError::ActorClosed => {
            RuntimeControlError::Unavailable(format!("{context}：节点运行时已关闭"))
        }
        RuntimeControlError::Invalid(message) => {
            RuntimeControlError::Invalid(format!("{context}：{message}"))
        }
        RuntimeControlError::Conflict(message) => {
            RuntimeControlError::Conflict(format!("{context}：{message}"))
        }
        RuntimeControlError::CreationConflict { reason, message } => {
            RuntimeControlError::CreationConflict {
                reason,
                message: format!("{context}：{message}"),
            }
        }
        RuntimeControlError::PermissionDenied(message) => {
            RuntimeControlError::PermissionDenied(format!("{context}：{message}"))
        }
        RuntimeControlError::Cancelled(message) => {
            RuntimeControlError::Cancelled(format!("{context}：{message}"))
        }
        RuntimeControlError::Unsupported(message) => {
            RuntimeControlError::Unsupported(format!("{context}：{message}"))
        }
        RuntimeControlError::NotFound(message) => {
            RuntimeControlError::NotFound(format!("{context}：{message}"))
        }
        RuntimeControlError::Unavailable(message) => {
            RuntimeControlError::Unavailable(format!("{context}：{message}"))
        }
        RuntimeControlError::RelayLifecycleFailed {
            reason_code,
            message,
        } => RuntimeControlError::RelayLifecycleFailed {
            reason_code,
            message: format!("{context}：{message}"),
        },
        RuntimeControlError::ResourceExhausted(message) => {
            RuntimeControlError::ResourceExhausted(format!("{context}：{message}"))
        }
        RuntimeControlError::Persistence(message) => {
            RuntimeControlError::Persistence(format!("{context}：{message}"))
        }
        RuntimeControlError::Storage(message) => {
            RuntimeControlError::Storage(format!("{context}：{message}"))
        }
        RuntimeControlError::Launch(message) => {
            RuntimeControlError::Launch(format!("{context}：{message}"))
        }
        RuntimeControlError::NetworkSetup { stage, message } => RuntimeControlError::NetworkSetup {
            stage,
            message: format!("{context}：{message}"),
        },
        RuntimeControlError::Timeout(message) => {
            RuntimeControlError::Timeout(format!("{context}：{message}"))
        }
        RuntimeControlError::JoinRequest {
            code,
            message_key,
            message,
        } => RuntimeControlError::JoinRequest {
            code,
            message_key,
            message: format!("{context}：{message}"),
        },
        RuntimeControlError::Invitation {
            code,
            message_key,
            message,
        } => RuntimeControlError::Invitation {
            code,
            message_key,
            message: format!("{context}：{message}"),
        },
        RuntimeControlError::CoordinatorCandidate {
            code,
            message_key,
            message,
        } => RuntimeControlError::CoordinatorCandidate {
            code,
            message_key,
            message: format!("{context}：{message}"),
        },
        RuntimeControlError::AdminRecovery {
            code,
            message_key,
            message,
        } => RuntimeControlError::AdminRecovery {
            code,
            message_key,
            message: format!("{context}：{message}"),
        },
    }
}

fn local_exit_request_results(
    results: Vec<(
        String,
        Result<ipc::LocalExitRuntimeStatus, RuntimeRoutingRequestError>,
    )>,
) -> (Vec<ipc::LocalExitRuntimeStatus>, Option<NodeProblem>) {
    let mut statuses = Vec::with_capacity(results.len());
    let mut problem = None;
    for (network_name, result) in results {
        match result {
            Ok(status) => statuses.push(status),
            Err(error) if problem.is_none() => {
                let error = local_exit_request_error(&network_name, &error);
                problem = Some(runtime_control_problem(&error));
            }
            Err(_) => {}
        }
    }
    (statuses, problem)
}

fn local_exit_results_are_draining(
    results: &[(
        String,
        Result<ipc::LocalExitRuntimeStatus, RuntimeRoutingRequestError>,
    )],
) -> bool {
    results.iter().any(|(_, result)| {
        result.as_ref().is_ok_and(|status| {
            matches!(
                status.stage,
                ipc::LocalExitRuntimeStage::Draining | ipc::LocalExitRuntimeStage::Cleaning
            )
        })
    })
}

fn local_exit_capability_from_statuses(
    owner_exit_enabled: bool,
    statuses: &[ipc::LocalExitRuntimeStatus],
    problem: Option<NodeProblem>,
) -> LocalExitCapabilitySnapshot {
    let network_allowed = statuses.iter().any(|status| status.network_allowed);
    let effective = statuses
        .iter()
        .filter(|status| status.effective)
        .collect::<Vec<_>>();
    let healthy = !effective.is_empty() && effective.iter().all(|status| status.healthy);
    let active_clients = statuses.iter().fold(0u32, |total, status| {
        total.saturating_add(status.active_clients)
    });
    let total_clients = statuses.iter().fold(0u64, |total, status| {
        total.saturating_add(status.total_clients)
    });
    let draining = statuses
        .iter()
        .any(|status| status.stage == ipc::LocalExitRuntimeStage::Draining);
    let cleaning = statuses
        .iter()
        .any(|status| status.stage == ipc::LocalExitRuntimeStage::Cleaning);
    let failed = statuses
        .iter()
        .any(|status| status.stage == ipc::LocalExitRuntimeStage::Failed);
    let status_problem = statuses.iter().find_map(|status| status.problem.clone());
    let effective_unhealthy =
        owner_exit_enabled && !draining && !cleaning && !effective.is_empty() && !healthy;
    let problem = problem.or(status_problem).or_else(|| {
        effective_unhealthy.then(|| {
            NodeProblem::new(
                NodeProblemCode::Unavailable,
                "本机 Exit 已授权，但数据面健康检查未通过",
            )
            .retryable(true)
        })
    });
    let stage = if problem.is_some() || failed {
        LocalExitCapabilityStage::Failed
    } else if cleaning {
        LocalExitCapabilityStage::Cleaning
    } else if draining {
        LocalExitCapabilityStage::Draining
    } else if !owner_exit_enabled {
        LocalExitCapabilityStage::Disabled
    } else if !network_allowed {
        LocalExitCapabilityStage::WaitingForNetworkAuthorization
    } else if healthy {
        LocalExitCapabilityStage::Enabled
    } else {
        LocalExitCapabilityStage::HealthChecking
    };

    LocalExitCapabilitySnapshot {
        owner_exit_enabled,
        stage,
        healthy: stage == LocalExitCapabilityStage::Enabled,
        network_allowed,
        active_clients,
        total_clients,
        problem,
        last_transition_at_unix_ms: 0,
    }
}

fn runtime_routing_mode_name(mode: NodeRoutingMode) -> &'static str {
    match mode {
        NodeRoutingMode::MeshOnly => "MeshOnly",
        NodeRoutingMode::BypassMainland => "BypassMainland",
        NodeRoutingMode::GlobalProxy => "GlobalProxy",
    }
}

fn roles_from_config(config: &ClientConfig) -> Vec<NodeNetworkRole> {
    let mut roles = vec![NodeNetworkRole::Member];
    if config.exit_node.enabled {
        roles.push(NodeNetworkRole::ExitNode);
    }
    roles
}

fn map_network_creation_error(
    network_name: &str,
    error: NetworkCreationError,
) -> RuntimeControlError {
    match error {
        NetworkCreationError::Invalid(message) => {
            RuntimeControlError::Invalid(format!("网络 [{network_name}] 创建参数无效: {message}"))
        }
        NetworkCreationError::DuplicateName(_) => RuntimeControlError::CreationConflict {
            reason: "duplicate_name",
            message: format!("网络 [{network_name}] 已经存在"),
        },
        NetworkCreationError::DuplicateId(network_id) => RuntimeControlError::CreationConflict {
            reason: "duplicate_id",
            message: format!("网络 [{network_name}] 的稳定 ID [{network_id}] 已存在"),
        },
        NetworkCreationError::PermissionDenied(message) => RuntimeControlError::PermissionDenied(
            format!("创建网络 [{network_name}] 的权限不足: {message}"),
        ),
        NetworkCreationError::Storage(message) => {
            RuntimeControlError::Storage(format!("保存网络 [{network_name}] 失败: {message}"))
        }
    }
}

fn ensure_member_role(
    network_name: &str,
    roles: &[NodeNetworkRole],
) -> Result<(), RuntimeControlError> {
    if roles.contains(&NodeNetworkRole::Member) {
        return Ok(());
    }
    Err(RuntimeControlError::Persistence(format!(
        "网络 [{network_name}] 的共享运行快照缺少基线 Member 角色，未应用角色更新"
    )))
}

fn set_role_enabled(roles: &mut Vec<NodeNetworkRole>, role: NodeNetworkRole, enabled: bool) {
    roles.retain(|existing| *existing != role);
    if enabled {
        roles.push(role);
    }
    roles.sort_by_key(|role| match role {
        NodeNetworkRole::Member => 0,
        NodeNetworkRole::Discovery => 1,
        NodeNetworkRole::Relay => 2,
        NodeNetworkRole::ExitNode => 3,
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::node::{
        NodeCommand, NodeCommandBusError, NodeCommandOutcome, NodeCommandRequest,
        NodeErrorCategory, NodeEvent, NodeInterfaceKind, NodeInterfaceOwnership,
        NodeInterfaceSnapshot, NodeInterfaceStatus, NodePeerSnapshot,
        NodeSetLocalExitCapabilityCommand,
    };
    use crate::p2p::connection::P2pEngine;
    use crate::stats::StatsCollector;
    use crate::tun::{
        TunCapabilities, TunCapabilityReason, TunPlatform, TUN_CAPABILITY_SCHEMA_VERSION,
    };
    use fuckipv6_common::domain::{
        NodeId, RelayAuthorizationAuditSnapshot, RelayExistingSessionPolicy,
        RelayGovernanceAuditSnapshot, RelayHealthSnapshot, RelayLifecycleState, RelayLimitPolicy,
        RelayReachabilityState, RelayUsageSnapshot,
    };
    use futures::FutureExt;
    use std::collections::HashSet;
    use std::net::{Ipv4Addr, Ipv6Addr};
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
    use std::sync::Mutex;
    use std::time::Duration;
    use tokio::sync::broadcast;

    fn identity_snapshot(
        node_id: &str,
        display_name: Option<&str>,
        fingerprint: &str,
        created_at_unix_ms: Option<u64>,
    ) -> NodeIdentitySnapshot {
        NodeIdentitySnapshot {
            node_id: node_id.to_string(),
            display_name: display_name.map(str::to_string),
            public_key_fingerprint: fingerprint.to_string(),
            created_at_unix_ms,
        }
    }

    #[test]
    fn identity_binding_ignores_non_key_metadata_changes() {
        let existing = identity_snapshot("node-a", Some("old name"), "key-a", Some(10));
        let incoming = identity_snapshot("node-a", None, "key-a", None);

        assert!(!conflicting_identity_binding(&existing, &incoming));
    }

    #[test]
    fn identity_binding_warns_on_node_id_or_public_key_fingerprint_changes() {
        let existing = identity_snapshot("node-a", None, "key-a", None);
        let changed_node_id = identity_snapshot("node-b", None, "key-a", None);
        let changed_fingerprint = identity_snapshot("node-a", None, "key-b", None);

        assert!(conflicting_identity_binding(&existing, &changed_node_id));
        assert!(conflicting_identity_binding(
            &existing,
            &changed_fingerprint
        ));
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct RoutingCall {
        network_name: String,
        mode: NodeRoutingMode,
        exit_node: Option<String>,
        is_exit_node: bool,
    }

    #[derive(Clone, Default)]
    struct FakeLauncher {
        registry: Option<MultiNetworkRegistry>,
        launches: Arc<Mutex<Vec<String>>>,
        failures: Arc<Mutex<HashSet<String>>>,
        shutdowns: Arc<AtomicUsize>,
        routing_calls: Arc<Mutex<Vec<RoutingCall>>>,
        routing_failures: Arc<Mutex<HashSet<String>>>,
        next_routing_failures: Arc<Mutex<HashSet<String>>>,
        next_routing_persistence_failures: Arc<Mutex<HashSet<String>>>,
        runtime_exit_nodes: Arc<Mutex<HashMap<String, bool>>>,
        local_exit_calls: Arc<Mutex<Vec<(String, bool)>>>,
        next_local_exit_failures: Arc<Mutex<HashSet<String>>>,
        runtime_local_exit_owners: Arc<Mutex<HashMap<String, bool>>>,
        local_exit_health: Arc<Mutex<HashMap<String, bool>>>,
        local_exit_clients: Arc<Mutex<HashMap<String, (u32, u64)>>>,
        local_exit_drain_responses: Arc<Mutex<HashMap<String, usize>>>,
        routing_delay_ms: Arc<AtomicU64>,
        routing_in_flight: Arc<AtomicUsize>,
        routing_max_in_flight: Arc<AtomicUsize>,
        launch_delay_ms: Arc<AtomicU64>,
        launch_attempts: Arc<AtomicUsize>,
        shutdown_delay_ms: Arc<AtomicU64>,
        shutdown_delays_ms: Arc<Mutex<HashMap<String, u64>>>,
        cleanup_failures: Arc<Mutex<HashSet<String>>>,
        residual_cleanup_delays_ms: Arc<Mutex<HashMap<String, u64>>>,
        lost_owners: Arc<Mutex<HashSet<String>>>,
        launched_tun_names: Arc<Mutex<Vec<(String, String)>>>,
        launched_network_ids: Arc<Mutex<Vec<Option<String>>>>,
    }

    struct FakeResidualRuntimeOwner {
        network_name: String,
        p2p: P2pEngine,
        registry: Option<MultiNetworkRegistry>,
        cleanup_failures: Arc<Mutex<HashSet<String>>>,
        cleanup_delays_ms: Arc<Mutex<HashMap<String, u64>>>,
        shutdowns: Arc<AtomicUsize>,
    }

    impl ResidualRuntimeOwner for FakeResidualRuntimeOwner {
        fn shutdown(&mut self) -> Pin<Box<dyn Future<Output = RuntimeCleanupOutcome> + Send + '_>> {
            Box::pin(async move {
                self.shutdowns.fetch_add(1, Ordering::SeqCst);
                let delay = self
                    .cleanup_delays_ms
                    .lock()
                    .expect("cleanup delays lock poisoned")
                    .get(&self.network_name)
                    .copied()
                    .unwrap_or_default();
                if delay > 0 {
                    tokio::time::sleep(Duration::from_millis(delay)).await;
                }
                let mut cleanup = RuntimeCleanupOutcome::default();
                let status = if self
                    .cleanup_failures
                    .lock()
                    .expect("cleanup failure lock poisoned")
                    .contains(&self.network_name)
                {
                    CleanupStatus::Failed("测试清理失败".into())
                } else {
                    CleanupStatus::Removed
                };
                cleanup.record("测试运行时", status);
                if cleanup.is_clean() {
                    if let Err(error) = self.p2p.shutdown().await {
                        cleanup.record("测试 P2P", CleanupStatus::Failed(format!("{error:#}")));
                    }
                }
                if cleanup.is_clean() {
                    if let Some(registry) = &self.registry {
                        registry.unregister(&self.network_name).await;
                    }
                }
                cleanup
            })
        }
    }

    impl FakeLauncher {
        fn with_registry(registry: MultiNetworkRegistry) -> Self {
            Self {
                registry: Some(registry),
                ..Self::default()
            }
        }

        fn fail(&self, network_name: &str) {
            self.failures
                .lock()
                .expect("failure lock poisoned")
                .insert(network_name.to_string());
        }

        fn recover(&self, network_name: &str) {
            self.failures
                .lock()
                .expect("failure lock poisoned")
                .remove(network_name);
        }

        fn launches(&self) -> Vec<String> {
            self.launches.lock().expect("launch lock poisoned").clone()
        }

        fn shutdown_count(&self) -> usize {
            self.shutdowns.load(Ordering::SeqCst)
        }

        fn fail_routing(&self, network_name: &str) {
            self.routing_failures
                .lock()
                .expect("routing failure lock poisoned")
                .insert(network_name.to_string());
        }

        fn fail_next_routing(&self, network_name: &str) {
            self.next_routing_failures
                .lock()
                .expect("next routing failure lock poisoned")
                .insert(network_name.to_string());
        }

        fn fail_next_routing_persistence(&self, network_name: &str) {
            self.next_routing_persistence_failures
                .lock()
                .expect("next routing persistence failure lock poisoned")
                .insert(network_name.to_string());
        }

        fn set_routing_delay(&self, delay: Duration) {
            self.routing_delay_ms
                .store(delay.as_millis() as u64, Ordering::SeqCst);
        }

        fn routing_calls(&self) -> Vec<RoutingCall> {
            self.routing_calls
                .lock()
                .expect("routing call lock poisoned")
                .clone()
        }

        fn max_routing_concurrency(&self) -> usize {
            self.routing_max_in_flight.load(Ordering::SeqCst)
        }

        fn runtime_exit_node(&self, network_name: &str) -> Option<bool> {
            self.runtime_exit_nodes
                .lock()
                .expect("runtime exit node lock poisoned")
                .get(network_name)
                .copied()
        }

        fn fail_next_local_exit(&self, network_name: &str) {
            self.next_local_exit_failures
                .lock()
                .expect("next local exit failure lock poisoned")
                .insert(network_name.into());
        }

        fn local_exit_calls(&self) -> Vec<(String, bool)> {
            self.local_exit_calls
                .lock()
                .expect("local exit call lock poisoned")
                .clone()
        }

        fn runtime_local_exit_owner(&self, network_name: &str) -> Option<bool> {
            self.runtime_local_exit_owners
                .lock()
                .expect("runtime local exit owner lock poisoned")
                .get(network_name)
                .copied()
        }

        fn set_local_exit_health(&self, network_name: &str, healthy: bool) {
            self.local_exit_health
                .lock()
                .expect("local exit health lock poisoned")
                .insert(network_name.into(), healthy);
        }

        fn set_local_exit_clients(&self, network_name: &str, active: u32, total: u64) {
            self.local_exit_clients
                .lock()
                .expect("local exit clients lock poisoned")
                .insert(network_name.into(), (active, total));
        }

        fn set_local_exit_drain_responses(&self, network_name: &str, responses: usize) {
            self.local_exit_drain_responses
                .lock()
                .expect("local exit drain response lock poisoned")
                .insert(network_name.into(), responses);
        }

        fn set_launch_delay(&self, delay: Duration) {
            self.launch_delay_ms
                .store(delay.as_millis() as u64, Ordering::SeqCst);
        }

        fn launch_attempts(&self) -> usize {
            self.launch_attempts.load(Ordering::SeqCst)
        }

        fn set_shutdown_delay(&self, delay: Duration) {
            self.shutdown_delay_ms
                .store(delay.as_millis() as u64, Ordering::SeqCst);
        }

        fn set_shutdown_delay_for(&self, network_name: &str, delay: Duration) {
            self.shutdown_delays_ms
                .lock()
                .expect("shutdown delays lock poisoned")
                .insert(network_name.into(), delay.as_millis() as u64);
        }

        fn fail_cleanup(&self, network_name: &str) {
            self.cleanup_failures
                .lock()
                .expect("cleanup failure lock poisoned")
                .insert(network_name.into());
        }

        fn recover_cleanup(&self, network_name: &str) {
            self.cleanup_failures
                .lock()
                .expect("cleanup failure lock poisoned")
                .remove(network_name);
        }

        fn lose_owner(&self, network_name: &str) {
            self.lost_owners
                .lock()
                .expect("lost owner lock poisoned")
                .insert(network_name.into());
        }

        fn set_residual_cleanup_delay(&self, network_name: &str, delay: Duration) {
            self.residual_cleanup_delays_ms
                .lock()
                .expect("residual cleanup delays lock poisoned")
                .insert(network_name.into(), delay.as_millis() as u64);
        }

        fn launched_tun_names(&self) -> Vec<(String, String)> {
            self.launched_tun_names
                .lock()
                .expect("launched tun names lock poisoned")
                .clone()
        }

        fn launched_network_ids(&self) -> Vec<Option<String>> {
            self.launched_network_ids
                .lock()
                .expect("launched network ids lock poisoned")
                .clone()
        }
    }

    impl NetworkRuntimeLauncher for FakeLauncher {
        fn launch(&self, request: NetworkLaunchRequest) -> RuntimeFuture<StartedNetworkRuntime> {
            let launches = self.launches.clone();
            let failures = self.failures.clone();
            let shutdowns = self.shutdowns.clone();
            let routing_calls = self.routing_calls.clone();
            let routing_failures = self.routing_failures.clone();
            let next_routing_failures = self.next_routing_failures.clone();
            let next_routing_persistence_failures = self.next_routing_persistence_failures.clone();
            let runtime_exit_nodes = self.runtime_exit_nodes.clone();
            let local_exit_calls = self.local_exit_calls.clone();
            let next_local_exit_failures = self.next_local_exit_failures.clone();
            let runtime_local_exit_owners = self.runtime_local_exit_owners.clone();
            let local_exit_health = self.local_exit_health.clone();
            let local_exit_clients = self.local_exit_clients.clone();
            let local_exit_drain_responses = self.local_exit_drain_responses.clone();
            let routing_delay_ms = self.routing_delay_ms.clone();
            let routing_in_flight = self.routing_in_flight.clone();
            let routing_max_in_flight = self.routing_max_in_flight.clone();
            let launch_delay_ms = self.launch_delay_ms.clone();
            let launch_attempts = self.launch_attempts.clone();
            let shutdown_delay_ms = self.shutdown_delay_ms.clone();
            let shutdown_delays_ms = self.shutdown_delays_ms.clone();
            let cleanup_failures = self.cleanup_failures.clone();
            let residual_cleanup_delays_ms = self.residual_cleanup_delays_ms.clone();
            let lost_owners = self.lost_owners.clone();
            let launched_tun_names = self.launched_tun_names.clone();
            let launched_network_ids = self.launched_network_ids.clone();
            let registry = self.registry.clone();

            Box::pin(async move {
                launch_attempts.fetch_add(1, Ordering::SeqCst);
                let launch_delay = launch_delay_ms.load(Ordering::SeqCst);
                if launch_delay > 0 {
                    tokio::time::sleep(Duration::from_millis(launch_delay)).await;
                }
                if failures
                    .lock()
                    .expect("failure lock poisoned")
                    .contains(&request.network_name)
                {
                    return Err(RuntimeControlError::Launch(format!(
                        "网络 [{}] 测试启动失败",
                        request.network_name
                    )));
                }

                launches
                    .lock()
                    .expect("launch lock poisoned")
                    .push(request.network_name.clone());
                launched_network_ids
                    .lock()
                    .expect("launched network ids lock poisoned")
                    .push(request.network_id.as_ref().map(ToString::to_string));
                let tun_name = crate::multi_network::derive_tun_name(
                    Some(&request.network_name),
                    &request.config.tun.name,
                );
                launched_tun_names
                    .lock()
                    .expect("launched tun names lock poisoned")
                    .push((
                        request.network_name.clone(),
                        request.config.tun.name.clone(),
                    ));
                let network_name = request.network_name.clone();
                let initial_local_exit_enabled = request.local_exit_enabled;
                runtime_local_exit_owners
                    .lock()
                    .expect("runtime local exit owner lock poisoned")
                    .insert(network_name.clone(), initial_local_exit_enabled);
                let routing_network_name = network_name.clone();
                let shutdown_network_name = network_name.clone();
                let config_path = PathBuf::from(request.config_path);
                let mut current_config = request.config;
                let p2p = P2pEngine::new_loopback_for_test()
                    .await
                    .expect("create loopback p2p engine");
                let p2p_listen_addr = p2p.local_addr();
                if let Some(registry) = &registry {
                    registry
                        .register(NetworkSessionHandle {
                            network_name: network_name.clone(),
                            tun_name: tun_name.clone(),
                            assigned_ip: Ipv6Addr::LOCALHOST,
                            assigned_ipv4: Some(Ipv4Addr::LOCALHOST),
                            p2p: p2p.clone(),
                            stats: StatsCollector::new(),
                        })
                        .await;
                }
                let cleanup_registry = registry.clone();
                let cleanup_p2p = p2p.clone();
                let (routing_control, mut routing_control_rx) = mpsc::unbounded_channel();
                let (shutdown, mut shutdown_rx) = watch::channel(false);
                let handle = tokio::spawn(async move {
                    loop {
                        tokio::select! {
                            changed = shutdown_rx.changed() => {
                                if changed.is_err() || *shutdown_rx.borrow() {
                                    let shutdown_delay = shutdown_delays_ms
                                        .lock()
                                        .expect("shutdown delays lock poisoned")
                                        .get(&shutdown_network_name)
                                        .copied()
                                        .unwrap_or_else(|| shutdown_delay_ms.load(Ordering::SeqCst));
                                    if shutdown_delay > 0 {
                                        tokio::time::sleep(Duration::from_millis(shutdown_delay)).await;
                                    }
                                    break;
                                }
                            }
                            command = routing_control_rx.recv() => {
                                let Some(command) = command else {
                                    break;
                                };
                                match command {
                                    ipc::RuntimeCommand::UpdateRouting {
                                        mode,
                                        exit_node,
                                        is_exit_node,
                                        reply,
                                    } => {
                                        let node_mode = match mode.as_str() {
                                            "MeshOnly" => NodeRoutingMode::MeshOnly,
                                            "BypassMainland" => NodeRoutingMode::BypassMainland,
                                            "GlobalProxy" => NodeRoutingMode::GlobalProxy,
                                            _ => {
                                                let _ = reply.send(Err(format!("未知路由模式: {mode}")));
                                                continue;
                                            }
                                        };
                                        let in_flight = routing_in_flight.fetch_add(1, Ordering::SeqCst) + 1;
                                        routing_max_in_flight.fetch_max(in_flight, Ordering::SeqCst);
                                        routing_calls
                                            .lock()
                                            .expect("routing call lock poisoned")
                                            .push(RoutingCall {
                                                network_name: routing_network_name.clone(),
                                                mode: node_mode,
                                                exit_node: exit_node.clone(),
                                                is_exit_node,
                                            });

                                        let delay_ms = routing_delay_ms.load(Ordering::SeqCst);
                                        if delay_ms > 0 {
                                            tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                                        }

                                        let persistent_failure = routing_failures
                                            .lock()
                                            .expect("routing failure lock poisoned")
                                            .contains(&routing_network_name);
                                        let one_shot_failure = next_routing_failures
                                            .lock()
                                            .expect("next routing failure lock poisoned")
                                            .remove(&routing_network_name);
                                        let failed = persistent_failure || one_shot_failure;
                                        let result = if failed {
                                            Err("测试运行时拒绝路由热更新".to_string())
                                        } else {
                                            let mut updated_config = current_config.clone();
                                            updated_config.routing.mode = match node_mode {
                                                NodeRoutingMode::MeshOnly => {
                                                    fuckipv6_common::RoutingMode::MeshOnly
                                                }
                                                NodeRoutingMode::BypassMainland => {
                                                    fuckipv6_common::RoutingMode::BypassMainland
                                                }
                                                NodeRoutingMode::GlobalProxy => {
                                                    fuckipv6_common::RoutingMode::GlobalProxy
                                                }
                                            };
                                            updated_config.routing.exit_node = exit_node.clone();
                                            updated_config.exit_node.enabled = is_exit_node;
                                            let persistence_failed = next_routing_persistence_failures
                                                .lock()
                                                .expect("routing persistence failure lock poisoned")
                                                .remove(&routing_network_name);
                                            let persisted = if persistence_failed {
                                                Err("测试运行时注入持久化失败".to_string())
                                            } else {
                                                updated_config
                                                    .save_to_file(&config_path)
                                                    .map_err(|error| format!("测试持久化失败: {error:#}"))
                                            };
                                            persisted
                                                .map(|_| {
                                                    current_config = updated_config;
                                                    runtime_exit_nodes
                                                        .lock()
                                                        .expect("runtime exit node lock poisoned")
                                                        .insert(routing_network_name.clone(), is_exit_node);
                                                    ipc::RoutingUpdateResult {
                                                    mode: format!("{node_mode:?}"),
                                                    exit_node,
                                                    is_exit_node,
                                                    }
                                                })
                                        };
                                        routing_in_flight.fetch_sub(1, Ordering::SeqCst);
                                        let _ = reply.send(result);
                                    }
                                    ipc::RuntimeCommand::LocalExitStatus { reply } => {
                                        let enabled = runtime_local_exit_owners
                                            .lock()
                                            .expect("runtime local exit owner lock poisoned")
                                            .get(&routing_network_name)
                                            .copied()
                                            .unwrap_or(false);
                                        let network_allowed = current_config.exit_node.enabled;
                                        let effective = enabled && network_allowed;
                                        let health_override = local_exit_health
                                            .lock()
                                            .expect("local exit health lock poisoned")
                                            .get(&routing_network_name)
                                            .copied()
                                            .unwrap_or(true);
                                        let (active_clients, total_clients) = local_exit_clients
                                            .lock()
                                            .expect("local exit clients lock poisoned")
                                            .get(&routing_network_name)
                                            .copied()
                                            .unwrap_or((0, 0));
                                        let draining = if enabled {
                                            false
                                        } else {
                                            let mut responses = local_exit_drain_responses
                                                .lock()
                                                .expect("local exit drain response lock poisoned");
                                            match responses.get_mut(&routing_network_name) {
                                                Some(remaining) if *remaining > 0 => {
                                                    *remaining -= 1;
                                                    true
                                                }
                                                _ => false,
                                            }
                                        };
                                        let healthy = effective && health_override && !draining;
                                        let stage = if healthy {
                                            ipc::LocalExitRuntimeStage::Enabled
                                        } else if draining {
                                            ipc::LocalExitRuntimeStage::Draining
                                        } else if effective {
                                            ipc::LocalExitRuntimeStage::Failed
                                        } else {
                                            ipc::LocalExitRuntimeStage::Disabled
                                        };
                                        let _ = reply.send(Ok(ipc::LocalExitRuntimeStatus {
                                            owner_exit_enabled: enabled,
                                            network_allowed,
                                            effective,
                                            healthy,
                                            stage,
                                            problem: None,
                                            active_clients,
                                            total_clients,
                                        }));
                                    }
                                    ipc::RuntimeCommand::SetLocalExitCapability { enabled, reply } => {
                                        local_exit_calls
                                            .lock()
                                            .expect("local exit call lock poisoned")
                                            .push((routing_network_name.clone(), enabled));
                                        let failed = next_local_exit_failures
                                            .lock()
                                            .expect("next local exit failure lock poisoned")
                                            .remove(&routing_network_name);
                                        if failed {
                                            let _ = reply.send(Err(
                                                RuntimeControlError::Unavailable(format!(
                                                    "网络 [{}] 测试本机 Exit 启停失败",
                                                    routing_network_name
                                                )),
                                            ));
                                            continue;
                                        }
                                        let network_allowed = current_config.exit_node.enabled;
                                        let effective = enabled && network_allowed;
                                        let health_override = local_exit_health
                                            .lock()
                                            .expect("local exit health lock poisoned")
                                            .get(&routing_network_name)
                                            .copied()
                                            .unwrap_or(true);
                                        let (active_clients, total_clients) = local_exit_clients
                                            .lock()
                                            .expect("local exit clients lock poisoned")
                                            .get(&routing_network_name)
                                            .copied()
                                            .unwrap_or((0, 0));
                                        let draining = if enabled {
                                            false
                                        } else {
                                            let mut responses = local_exit_drain_responses
                                                .lock()
                                                .expect("local exit drain response lock poisoned");
                                            match responses.get_mut(&routing_network_name) {
                                                Some(remaining) if *remaining > 0 => {
                                                    *remaining -= 1;
                                                    true
                                                }
                                                _ => false,
                                            }
                                        };
                                        runtime_local_exit_owners
                                            .lock()
                                            .expect("runtime local exit owner lock poisoned")
                                            .insert(routing_network_name.clone(), enabled);
                                        let healthy = effective && health_override && !draining;
                                        let stage = if healthy {
                                            ipc::LocalExitRuntimeStage::Enabled
                                        } else if draining {
                                            ipc::LocalExitRuntimeStage::Draining
                                        } else if effective {
                                            ipc::LocalExitRuntimeStage::Failed
                                        } else {
                                            ipc::LocalExitRuntimeStage::Disabled
                                        };
                                        let _ = reply.send(Ok(ipc::LocalExitRuntimeStatus {
                                            owner_exit_enabled: enabled,
                                            network_allowed,
                                            effective,
                                            healthy,
                                            stage,
                                            problem: None,
                                            active_clients,
                                            total_clients,
                                        }));
                                    }
                                    ipc::RuntimeCommand::ProbePeer { reply, .. } => {
                                        let _ = reply.send(Err("测试运行时不支持探测".into()));
                                    }
                                    ipc::RuntimeCommand::Disconnect { reply } => {
                                        let _ = reply.send(Ok(()));
                                        break;
                                    }
                                }
                            }
                        }
                    }
                    let mut owner = FakeResidualRuntimeOwner {
                        network_name: shutdown_network_name.clone(),
                        p2p: cleanup_p2p,
                        registry: cleanup_registry,
                        cleanup_failures,
                        cleanup_delays_ms: residual_cleanup_delays_ms,
                        shutdowns,
                    };
                    let cleanup = owner.shutdown().await;
                    let owner_lost = lost_owners
                        .lock()
                        .expect("lost owner lock poisoned")
                        .contains(&shutdown_network_name);
                    let retained_owner = if cleanup.is_clean() || owner_lost {
                        None
                    } else {
                        Some(Box::new(owner) as Box<dyn ResidualRuntimeOwner>)
                    };
                    NetworkRuntimeExit {
                        cleanup,
                        retained_owner,
                    }
                });

                Ok(StartedNetworkRuntime {
                    network_id: request.network_id.map(|network_id| network_id.to_string()),
                    signaling: None,
                    tun_name,
                    assigned_ipv6: Ipv6Addr::LOCALHOST,
                    assigned_ipv4: Some(Ipv4Addr::LOCALHOST),
                    p2p_listen_addr,
                    identity: NodeIdentitySnapshot {
                        node_id: format!("node-{network_name}"),
                        display_name: None,
                        public_key_fingerprint: format!("fingerprint-{network_name}"),
                        created_at_unix_ms: None,
                    },
                    routing_control,
                    shutdown,
                    handle,
                })
            })
        }
    }

    struct TestHarness {
        configs: ConfigManager,
        state: NodeStateStore,
        events: NodeEventBus,
        lifecycle: NodeLifecycle,
        registry: MultiNetworkRegistry,
        services: EmbeddedServiceManager,
        runtime: NetworkRuntimeManager,
        fake: FakeLauncher,
        config_dir: PathBuf,
    }

    impl TestHarness {
        async fn new(label: &str) -> Self {
            Self::new_with_routing_timeout(label, ROUTING_UPDATE_TIMEOUT).await
        }

        async fn new_with_routing_timeout(label: &str, routing_update_timeout: Duration) -> Self {
            Self::new_with_timeouts(
                label,
                RuntimeTimeouts {
                    routing_update: routing_update_timeout,
                    ..RuntimeTimeouts::default()
                },
            )
            .await
        }

        async fn new_with_timeouts(label: &str, timeouts: RuntimeTimeouts) -> Self {
            static NEXT_ID: AtomicU64 = AtomicU64::new(0);
            let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
            let config_dir = std::env::temp_dir().join(format!(
                "fuckipv6-runtime-control-{label}-{}-{id}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&config_dir);
            let configs = ConfigManager::new_for_dir(config_dir.clone()).expect("config manager");

            let events = NodeEventBus::default();
            let state = NodeStateStore::with_events(events.clone());
            let lifecycle = NodeLifecycle::new();
            let running = lifecycle.running(0, Vec::new()).await;
            state.set_lifecycle(running).await;
            let services = EmbeddedServiceManager::spawn_with_config(4, configs.clone());
            let registry = MultiNetworkRegistry::new();
            let fake = FakeLauncher::with_registry(registry.clone());
            let runtime = NetworkRuntimeManager::spawn_with_launcher_and_timeouts(
                configs.clone(),
                RuntimeOptions::default(),
                state.clone(),
                events.clone(),
                lifecycle.clone(),
                registry.clone(),
                services.clone(),
                Arc::new(fake.clone()),
                timeouts,
            );

            Self {
                configs,
                state,
                events,
                lifecycle,
                registry,
                services,
                runtime,
                fake,
                config_dir,
            }
        }

        async fn shutdown(&self) {
            let _ = self.runtime.shutdown().await;
            for session in self.registry.get_all().await {
                let _ = session.p2p.shutdown().await;
                self.registry.unregister(&session.network_name).await;
            }
            let _ = std::fs::remove_dir_all(&self.config_dir);
        }
    }

    async fn spawn_test_coordinator_runtime(
        harness: &TestHarness,
        network_name: &str,
        network_id: NetworkId,
        signaling: Arc<SignalingClient>,
        coordinator_takeover_wakeups: mpsc::UnboundedReceiver<()>,
    ) -> (
        mpsc::Sender<RuntimeControlCommand>,
        tokio::task::JoinHandle<()>,
    ) {
        let (shutdown, mut shutdown_rx) = watch::channel(false);
        let (routing_control, _routing_rx) = mpsc::unbounded_channel();
        let data_plane = tokio::spawn(async move {
            let _ = shutdown_rx.changed().await;
            NetworkRuntimeExit {
                cleanup: RuntimeCleanupOutcome::default(),
                retained_owner: None,
            }
        });
        let actor = NetworkRuntimeActor {
            configs: harness.configs.clone(),
            options: RuntimeOptions::default(),
            state: harness.state.clone(),
            events: harness.events.clone(),
            lifecycle: harness.lifecycle.clone(),
            registry: harness.registry.clone(),
            services: harness.services.clone(),
            launcher: Arc::new(harness.fake.clone()),
            sessions: HashMap::from([(
                network_name.to_string(),
                NetworkRuntimeSession {
                    network_id: Some(network_id.to_string()),
                    signaling: Some(signaling),
                    coordinator_recovery: None,
                    coordinator_emergency: None,
                    coordinator_continuity: CoordinatorContinuityMetrics::default(),
                    tun_name: format!("fuckipv6-{network_name}"),
                    routing_control,
                    shutdown,
                    handle: data_plane,
                },
            )]),
            retained_owners: HashMap::new(),
            cleanup_failures: BTreeMap::new(),
            completed_cleanup: BTreeMap::new(),
            timeouts: RuntimeTimeouts::default(),
            coordinator_takeover_wakeups: Some(coordinator_takeover_wakeups),
        };
        let (command_tx, command_rx) = mpsc::channel(8);
        let task = tokio::spawn(run_actor(command_rx, actor));
        (command_tx, task)
    }

    async fn shutdown_test_coordinator_runtime(
        command_tx: mpsc::Sender<RuntimeControlCommand>,
        task: tokio::task::JoinHandle<()>,
    ) {
        let operation = OperationGate::new()
            .try_acquire()
            .expect("acquire runtime shutdown operation");
        let (reply, response) = oneshot::channel();
        command_tx
            .send(RuntimeControlCommand::Shutdown {
                _operation: operation,
                reply,
            })
            .await
            .expect("runtime command channel closed");
        timeout(Duration::from_secs(2), response)
            .await
            .expect("runtime shutdown timed out")
            .expect("runtime shutdown response closed")
            .expect("runtime shutdown failed");
        timeout(Duration::from_secs(2), task)
            .await
            .expect("runtime actor did not exit")
            .expect("runtime actor task failed");
    }

    fn route_test_coordinator_runtime_message(
        store: &fuckipv6_coordinator::signaling::coordinator_candidates::CoordinatorCandidateStore,
        signaling: &SignalingClient,
        actor: &NodeId,
        network_id: &NetworkId,
        message: fuckipv6_common::SignalingMessage,
    ) -> &'static str {
        use fuckipv6_common::SignalingMessage;

        let now = unix_time_ms();
        match message {
            SignalingMessage::CoordinatorCandidateStatusRequest {
                network_id: requested,
            } => {
                assert_eq!(&requested, network_id);
                let status = store
                    .status(actor, network_id, now)
                    .expect("coordinator store status failed");
                assert!(signaling.route_coordinator_candidate_test_response(
                    SignalingMessage::CoordinatorCandidateStatusAck { status },
                ));
                "status"
            }
            SignalingMessage::CoordinatorRecoveryRequest { report } => {
                assert_eq!(&report.network_id, network_id);
                let response = store
                    .recover(actor, report, now)
                    .expect("coordinator store recovery failed");
                assert!(!response.recovery.governance_writable);
                assert!(signaling.route_coordinator_candidate_test_response(
                    SignalingMessage::CoordinatorRecoveryResponse { response },
                ));
                "recovery"
            }
            SignalingMessage::CoordinatorElectionPropose { proposal } => {
                assert_eq!(&proposal.proposer, actor);
                assert_eq!(&proposal.network_id, network_id);
                let status = store
                    .propose_election(actor, proposal, now)
                    .expect("coordinator election proposal failed");
                assert!(signaling.route_coordinator_candidate_test_response(
                    SignalingMessage::CoordinatorElectionProposeAck { status },
                ));
                "propose"
            }
            SignalingMessage::CoordinatorElectionVote { proposal_id, vote } => {
                assert_eq!(&vote.voter, actor);
                assert_eq!(&vote.network_id, network_id);
                let status = store
                    .record_election_vote(actor, &proposal_id, vote, now)
                    .expect("coordinator election vote failed");
                assert!(signaling.route_coordinator_candidate_test_response(
                    SignalingMessage::CoordinatorElectionVoteAck { status },
                ));
                "vote"
            }
            other => panic!("unexpected Coordinator runtime request: {other:?}"),
        }
    }

    fn create_command(name: &str, auto_connect: bool) -> NodeCreateNetworkCommand {
        NodeCreateNetworkCommand {
            name: name.into(),
            purpose: "A test network".into(),
            icon: "network".into(),
            language: "und".into(),
            region: "ZZ".into(),
            tags: Vec::new(),
            capability_declarations: Vec::new(),
            visibility: Default::default(),
            join_mode: Default::default(),
            exit_contribution: Default::default(),
            exit_contribution_scope: Default::default(),
            exit_contribution_mode: Default::default(),
            exit_selection_policy: Default::default(),
            signaling_port: 0,
            relay_port: 0,
            external_host: None,
            auto_connect,
        }
    }

    fn peer_snapshot(network_name: &str) -> NodePeerSnapshot {
        NodePeerSnapshot {
            peer_id: format!("peer-{network_name}"),
            network_name: network_name.into(),
            virtual_ipv6: Ipv6Addr::LOCALHOST,
            virtual_ipv4: None,
            endpoints: Vec::new(),
            online: true,
            p2p_active: true,
            p2p_healthy: true,
            rtt_us: Some(1_000),
            loss_rate: 0.0,
            traffic: NodeTrafficSnapshot::default(),
        }
    }

    fn routing_command(
        network_name: &str,
        mode: NodeRoutingMode,
        exit_node: Option<&str>,
        is_exit_node: bool,
    ) -> NodeUpdateRoutingCommand {
        NodeUpdateRoutingCommand {
            network_name: network_name.into(),
            mode,
            exit_node: exit_node.map(str::to_string),
            is_exit_node,
        }
    }

    fn role_command(
        network_name: &str,
        role: NodeNetworkRole,
        enabled: bool,
    ) -> NodeSetNetworkRoleCommand {
        NodeSetNetworkRoleCommand {
            network_name: network_name.into(),
            role,
            enabled,
        }
    }

    async fn start_fake_network(harness: &TestHarness, network_name: &str) {
        harness
            .configs
            .create_network(network_name, "ws://127.0.0.1:6666", true)
            .expect("create test config");
        let plan = NodeRuntimePlan::load(
            &harness.configs,
            NodeNetworkSelection::Named(vec![network_name.into()]),
        )
        .expect("load test plan");
        let summary = harness
            .runtime
            .start_plan(plan)
            .await
            .expect("start fake network");
        assert_eq!(summary.started, vec![network_name]);
    }

    fn supported_exit_capabilities() -> fuckipv6_common::ExitNodeCapabilities {
        fuckipv6_common::ExitNodeCapabilities {
            schema_version: fuckipv6_common::EXIT_NODE_CAPABILITY_SCHEMA_VERSION,
            platform: fuckipv6_common::ExitNodePlatform::Linux,
            supported: true,
            ipv4_forwarding: true,
            ipv6_forwarding: true,
            nat: true,
            cleanup: true,
            unsupported_reasons: Vec::new(),
        }
    }

    async fn set_supported_exit_capabilities(harness: &TestHarness) {
        harness
            .state
            .update(|snapshot| {
                snapshot.exit_node_capabilities = supported_exit_capabilities();
            })
            .await;
    }

    fn healthy_relay_health() -> RelayHealthSnapshot {
        RelayHealthSnapshot::observe(
            RelayLifecycleState::Running,
            Some("127.0.0.1:46667".parse().expect("relay address")),
            true,
            RelayReachabilityState::Reachable,
            true,
            true,
            RelayLimitPolicy::default(),
            Vec::new(),
            RelayUsageSnapshot {
                active_connections: 1,
                active_streams: 1,
                last_forwarded_at_unix_ms: Some(1_700_000_099_000),
                ..RelayUsageSnapshot::default()
            },
            1_700_000_100_000,
            None,
        )
    }

    async fn wait_for_routing_calls(fake: &FakeLauncher, expected: usize) {
        timeout(Duration::from_secs(2), async {
            loop {
                if fake.routing_calls().len() >= expected {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("routing update did not reach the fake runtime");
    }

    async fn wait_for_launch_attempts(fake: &FakeLauncher, expected: usize) {
        timeout(Duration::from_secs(2), async {
            loop {
                if fake.launch_attempts() >= expected {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("runtime launch was not attempted");
    }

    fn routing_changed_events(
        receiver: &mut broadcast::Receiver<NodeEvent>,
    ) -> Vec<(String, NodeRoutingMode, Option<String>, bool)> {
        let mut changes = Vec::new();
        while let Ok(event) = receiver.try_recv() {
            if let NodeEventKind::RoutingChanged {
                network_name,
                mode,
                exit_node,
                is_exit_node,
            } = event.kind
            {
                changes.push((network_name, mode, exit_node, is_exit_node));
            }
        }
        changes
    }

    fn role_changed_events(
        receiver: &mut broadcast::Receiver<NodeEvent>,
    ) -> Vec<(String, NodeNetworkRole, bool, Vec<NodeNetworkRole>)> {
        let mut changes = Vec::new();
        while let Ok(event) = receiver.try_recv() {
            if let NodeEventKind::RoleChanged {
                network_name,
                role,
                enabled,
                roles,
            } = event.kind
            {
                changes.push((network_name, role, enabled, roles));
            }
        }
        changes
    }

    fn embedded_service_events(
        receiver: &mut broadcast::Receiver<NodeEvent>,
        network_name: &str,
    ) -> Vec<NodeEmbeddedServiceStatus> {
        let mut statuses = Vec::new();
        while let Ok(event) = receiver.try_recv() {
            if let NodeEventKind::EmbeddedServiceChanged {
                network_name: event_network,
                status,
            } = event.kind
            {
                if event_network == network_name {
                    statuses.push(status);
                }
            }
        }
        statuses
    }

    fn relay_changed_events(
        receiver: &mut broadcast::Receiver<NodeEvent>,
        network_name: &str,
    ) -> Vec<NodeEmbeddedServiceStatus> {
        let mut statuses = Vec::new();
        while let Ok(event) = receiver.try_recv() {
            if let NodeEventKind::RelayChanged {
                network_name: event_network,
                status,
            } = event.kind
            {
                if event_network == network_name {
                    statuses.push(status);
                }
            }
        }
        statuses
    }

    async fn record_running_embedded_service_for_test(
        harness: &TestHarness,
        service: &EmbeddedServiceSnapshot,
    ) {
        let snapshot = NodeEmbeddedServiceSnapshot::running_from_embedded(service);
        harness
            .state
            .update(|state| {
                state
                    .embedded_services
                    .retain(|existing| existing.network_name != snapshot.network_name);
                state.embedded_services.push(snapshot);
            })
            .await;
    }

    #[tokio::test]
    async fn coordinator_takeover_background_candidate_failure_e2e() {
        use fuckipv6_common::domain::{
            CapabilityKind, CoordinatorCandidate, CoordinatorCandidateConfig,
            CoordinatorCandidateDeclaration, CoordinatorCandidateHealth,
            CoordinatorCandidateResources, CoordinatorCandidateWillingness,
            CoordinatorElectionProposal, CoordinatorElectionStage, CoordinatorElectionVote,
            CoordinatorPublicReachability, ExitContributionMode, ExitContributionObligation,
            ExitContributionScope, ExitSelectionPolicy, NetworkCreateProfile, NetworkJoinMode,
            NetworkRecord, NetworkVisibility, PublicKey,
        };
        use fuckipv6_common::Identity;
        use fuckipv6_coordinator::signaling::coordinator_candidates::CoordinatorCandidateStore;

        let owner = Identity::generate().expect("owner identity");
        let candidate_a = Identity::generate().expect("candidate A identity");
        let candidate_b = Identity::generate().expect("candidate B identity");
        let candidate_c = Identity::generate().expect("candidate C identity");
        let owner_id = NodeId::from_public_key(&owner.public_key());
        let candidate_a_id = NodeId::from_public_key(&candidate_a.public_key());
        let candidate_b_id = NodeId::from_public_key(&candidate_b.public_key());
        let candidate_c_id = NodeId::from_public_key(&candidate_c.public_key());
        let now = unix_time_ms();

        let mut network = NetworkRecord::create_owned(
            NetworkCreateProfile {
                name: "background-takeover-e2e".into(),
                purpose: "Verify automatic coordinator takeover through live runtimes".into(),
                icon: "network".into(),
                language: "zh-CN".into(),
                region: "ZZ".into(),
                tags: Vec::new(),
                capability_declarations: vec![CapabilityKind::Coordinator],
                visibility: NetworkVisibility::Private,
                join_mode: NetworkJoinMode::InviteOnly,
                exit_contribution: ExitContributionObligation::NotRequired,
                exit_contribution_scope: ExitContributionScope::NotApplicable,
                exit_contribution_mode: ExitContributionMode::Disabled,
                exit_selection_policy: ExitSelectionPolicy::default(),
            },
            PublicKey::from_bytes([17; 32]),
            owner_id.clone(),
            now.saturating_sub(30_000),
        )
        .expect("create network record");
        network.members.extend([
            candidate_a_id.clone(),
            candidate_b_id.clone(),
            candidate_c_id.clone(),
        ]);
        network.validate().expect("validate network record");
        let network_id = network.network_id.clone();

        let candidate_config = CoordinatorCandidateConfig::signed(
            network_id.clone(),
            1,
            vec![
                CoordinatorCandidate {
                    node_id: candidate_a_id.clone(),
                    priority: 1,
                },
                CoordinatorCandidate {
                    node_id: candidate_b_id.clone(),
                    priority: 2,
                },
                CoordinatorCandidate {
                    node_id: candidate_c_id.clone(),
                    priority: 3,
                },
            ],
            now.saturating_sub(25_000),
            now.saturating_add(30 * 60 * 1000),
            &owner,
        )
        .expect("sign candidate config");

        let b_harness = TestHarness::new("coordinator-background-takeover-b").await;
        let c_harness = TestHarness::new("coordinator-background-takeover-c").await;
        candidate_b
            .save_to_file(b_harness.configs.node_identity_path())
            .expect("install candidate B identity");
        candidate_c
            .save_to_file(c_harness.configs.node_identity_path())
            .expect("install candidate C identity");
        assert_eq!(
            NodeId::from_public_key(&b_harness.configs.load_node_identity().unwrap().public_key()),
            candidate_b_id
        );
        assert_eq!(
            NodeId::from_public_key(&c_harness.configs.load_node_identity().unwrap().public_key()),
            candidate_c_id
        );

        for (harness, network_name) in [
            (&b_harness, "background-takeover-b"),
            (&c_harness, "background-takeover-c"),
        ] {
            harness
                .configs
                .create_network(network_name, "ws://127.0.0.1:6666", true)
                .expect("create runtime network config");
            harness
                .configs
                .set_network_id_metadata(network_name, network_id.as_str())
                .expect("bind runtime network id");
        }

        let network_path = b_harness
            .config_dir
            .join("background-takeover-network.json");
        let store_path = b_harness.config_dir.join("background-takeover-store.json");
        std::fs::write(
            &network_path,
            serde_json::to_vec_pretty(&network).expect("serialize network record"),
        )
        .expect("write network record");
        let store = CoordinatorCandidateStore::new(&store_path, &network_path)
            .expect("create coordinator candidate store");
        store
            .update_config(&owner_id, 0, candidate_config.clone(), now)
            .expect("install candidate config");

        let declaration_issued_at = now.saturating_sub(20_000);
        let declarations = [
            (&candidate_a, now.saturating_sub(10_000), "203.0.113.31:443"),
            (&candidate_b, now + 60_000, "203.0.113.32:443"),
            (&candidate_c, now + 60_000, "203.0.113.33:443"),
        ];
        for (identity, expires_at_unix_ms, endpoint) in declarations {
            let declaration = CoordinatorCandidateDeclaration::signed(
                network_id.clone(),
                candidate_config.revision,
                CoordinatorPublicReachability {
                    public_endpoint: Some(endpoint.parse().expect("candidate endpoint")),
                    reachable_via_relay: true,
                },
                CoordinatorCandidateResources {
                    cpu_cores: 4,
                    memory_bytes: 512 * 1024 * 1024,
                    max_connections: 1_000,
                },
                CoordinatorCandidateWillingness::Willing,
                declaration_issued_at,
                expires_at_unix_ms,
                identity,
            )
            .expect("sign candidate declaration");
            store
                .publish_declaration(
                    &NodeId::from_public_key(&identity.public_key()),
                    declaration,
                    declaration_issued_at,
                )
                .expect("publish candidate declaration");
        }

        let proposal_issued_at = now.saturating_sub(15_000);
        let proposal = CoordinatorElectionProposal::signed(
            network_id.clone(),
            candidate_config.revision,
            1,
            vec![
                candidate_a_id.clone(),
                candidate_b_id.clone(),
                candidate_c_id.clone(),
            ],
            proposal_issued_at,
            now.saturating_sub(5_000),
            &candidate_a,
        )
        .expect("sign term 1 proposal");
        store
            .propose_election(&candidate_a_id, proposal.clone(), proposal_issued_at)
            .expect("start term 1 election");
        for (voter, identity) in [
            (&candidate_a_id, &candidate_a),
            (&candidate_b_id, &candidate_b),
        ] {
            let vote = CoordinatorElectionVote::signed(
                network_id.clone(),
                proposal.candidate_config_revision,
                proposal.term,
                proposal.leader.clone(),
                proposal.candidate_group.clone(),
                now.saturating_sub(14_000),
                now + 30_000,
                identity,
            )
            .expect("sign term 1 vote");
            store
                .record_election_vote(
                    voter,
                    &proposal.proposal_id,
                    vote,
                    now.saturating_sub(14_000),
                )
                .expect("record term 1 vote");
        }
        let failed_leader_status = store
            .status(&owner_id, &network_id, now)
            .expect("load term 1 status");
        let term_one = failed_leader_status
            .latest_term
            .as_ref()
            .expect("term 1 is installed");
        assert_eq!(term_one.leader, candidate_a_id);
        assert!(!failed_leader_status.lease.is_writable());
        assert!(failed_leader_status
            .candidates
            .iter()
            .find(|candidate| candidate.node_id == candidate_a_id)
            .is_some_and(|candidate| {
                candidate.health == CoordinatorCandidateHealth::Stale
                    && !candidate.eligible_for_election
            }));
        assert_eq!(
            failed_leader_status
                .preferred_election_candidate()
                .map(|candidate| &candidate.node_id),
            Some(&candidate_b_id)
        );

        let (b_signaling, mut b_outbound) = SignalingClient::test_channel();
        let (c_signaling, mut c_outbound) = SignalingClient::test_channel();
        let b_signaling = Arc::new(b_signaling);
        let c_signaling = Arc::new(c_signaling);
        let (b_wakeup_tx, b_wakeup_rx) = mpsc::unbounded_channel();
        let (c_wakeup_tx, c_wakeup_rx) = mpsc::unbounded_channel();
        let (b_command_tx, b_runtime) = spawn_test_coordinator_runtime(
            &b_harness,
            "background-takeover-b",
            network_id.clone(),
            b_signaling.clone(),
            b_wakeup_rx,
        )
        .await;
        let (c_command_tx, c_runtime) = spawn_test_coordinator_runtime(
            &c_harness,
            "background-takeover-c",
            network_id.clone(),
            c_signaling.clone(),
            c_wakeup_rx,
        )
        .await;
        b_wakeup_tx
            .send(())
            .expect("queue candidate B authority refresh");
        c_wakeup_tx
            .send(())
            .expect("queue candidate C authority refresh");

        let responder_b_id = candidate_b_id.clone();
        let responder_c_id = candidate_c_id.clone();
        let responder = tokio::spawn(async move {
            let mut proposal_seen = false;
            timeout(Duration::from_secs(10), async {
                while !proposal_seen {
                    tokio::select! {
                        message = b_outbound.recv() => {
                            let message = message.expect("candidate B signaling channel closed");
                            if route_test_coordinator_runtime_message(
                                &store,
                                b_signaling.as_ref(),
                                &responder_b_id,
                                &network_id,
                                message,
                            ) == "propose"
                            {
                                proposal_seen = true;
                            }
                        }
                        message = c_outbound.recv() => {
                            let message = message.expect("candidate C signaling channel closed");
                            route_test_coordinator_runtime_message(
                                &store,
                                c_signaling.as_ref(),
                                &responder_c_id,
                                &network_id,
                                message,
                            );
                        }
                    }
                }
            })
            .await
            .expect("candidate B did not propose after candidate A failed");
            b_wakeup_tx
                .send(())
                .expect("wake candidate B for election voting");
            c_wakeup_tx
                .send(())
                .expect("wake candidate C for election voting");

            let committed_status = timeout(Duration::from_secs(10), async {
                loop {
                    let status = store
                        .status(&owner_id, &network_id, unix_time_ms())
                        .expect("poll committed takeover status");
                    if status
                        .latest_term
                        .as_ref()
                        .is_some_and(|term| term.term == 2 && term.leader == responder_b_id)
                    {
                        break status;
                    }
                    tokio::select! {
                        message = b_outbound.recv() => {
                            let message = message.expect("candidate B signaling channel closed");
                            route_test_coordinator_runtime_message(
                                &store,
                                b_signaling.as_ref(),
                                &responder_b_id,
                                &network_id,
                                message,
                            );
                        }
                        message = c_outbound.recv() => {
                            let message = message.expect("candidate C signaling channel closed");
                            route_test_coordinator_runtime_message(
                                &store,
                                c_signaling.as_ref(),
                                &responder_c_id,
                                &network_id,
                                message,
                            );
                        }
                    }
                }
            })
            .await
            .expect("runtime candidates did not commit term 2");

            b_wakeup_tx
                .send(())
                .expect("wake candidate B after takeover");
            c_wakeup_tx
                .send(())
                .expect("wake candidate C after takeover");
            let mut b_refreshes = 0_u8;
            let mut c_refreshes = 0_u8;
            timeout(Duration::from_secs(10), async {
                while b_refreshes < 2 || c_refreshes < 2 {
                    tokio::select! {
                        message = b_outbound.recv() => {
                            let message = message.expect("candidate B signaling channel closed");
                            let kind = route_test_coordinator_runtime_message(
                                &store,
                                b_signaling.as_ref(),
                                &responder_b_id,
                                &network_id,
                                message,
                            );
                            assert!(matches!(kind, "status" | "recovery"));
                            b_refreshes += 1;
                        }
                        message = c_outbound.recv() => {
                            let message = message.expect("candidate C signaling channel closed");
                            let kind = route_test_coordinator_runtime_message(
                                &store,
                                c_signaling.as_ref(),
                                &responder_c_id,
                                &network_id,
                                message,
                            );
                            assert!(matches!(kind, "status" | "recovery"));
                            c_refreshes += 1;
                        }
                    }
                }
            })
            .await
            .expect("runtime candidates did not refresh the committed term");
            committed_status
        });
        let committed_status = responder.await.expect("coordinator responder task failed");

        let term_two = committed_status
            .latest_term
            .as_ref()
            .expect("term 2 must be installed");
        assert_eq!(term_two.term, 2);
        assert_eq!(term_two.fencing_token.0, 2);
        assert_eq!(term_two.leader, candidate_b_id);
        assert_eq!(
            committed_status.current_leader.as_ref(),
            Some(&candidate_b_id)
        );
        assert!(committed_status.lease.is_writable());
        assert_eq!(
            committed_status
                .config
                .as_ref()
                .map(|config| config.revision),
            Some(candidate_config.revision)
        );
        let election = committed_status
            .latest_election
            .as_ref()
            .expect("term 2 election evidence");
        assert_eq!(election.stage, CoordinatorElectionStage::Committed);
        assert!(election
            .votes
            .iter()
            .any(|vote| vote.voter == candidate_b_id));
        assert!(election
            .votes
            .iter()
            .any(|vote| vote.voter == candidate_c_id));
        assert!(!b_runtime.is_finished());
        assert!(!c_runtime.is_finished());

        shutdown_test_coordinator_runtime(b_command_tx, b_runtime).await;
        shutdown_test_coordinator_runtime(c_command_tx, c_runtime).await;
        b_harness.shutdown().await;
        c_harness.shutdown().await;
    }

    #[tokio::test]
    async fn coordinator_takeover_poll_preserves_the_live_data_plane_session() {
        use fuckipv6_common::domain::{
            CoordinatorCandidateConfigState, CoordinatorEmergencyState, CoordinatorLeaseStatus,
        };
        use fuckipv6_common::SignalingMessage;

        let harness = TestHarness::new("coordinator-takeover-session-continuity").await;
        harness
            .configs
            .create_network("takeover-continuity", "ws://127.0.0.1:6666", true)
            .expect("create network configuration");
        let network_id = NetworkId::from_public_key(b"coordinator-takeover-session-continuity");
        harness
            .configs
            .set_network_id_metadata("takeover-continuity", network_id.as_str())
            .expect("bind network id");

        let (signaling, mut outbound) = SignalingClient::test_channel();
        let signaling = Arc::new(signaling);
        let (shutdown_tx, mut shutdown_rx) = watch::channel(false);
        let (routing_tx, _routing_rx) = mpsc::unbounded_channel();
        let (terminated_tx, mut terminated_rx) = oneshot::channel();
        let data_plane = tokio::spawn(async move {
            let _ = shutdown_rx.changed().await;
            let _ = terminated_tx.send(());
            NetworkRuntimeExit {
                cleanup: RuntimeCleanupOutcome::default(),
                retained_owner: None,
            }
        });

        let mut actor = NetworkRuntimeActor {
            configs: harness.configs.clone(),
            options: RuntimeOptions::default(),
            state: harness.state.clone(),
            events: harness.events.clone(),
            lifecycle: harness.lifecycle.clone(),
            registry: harness.registry.clone(),
            services: harness.services.clone(),
            launcher: Arc::new(harness.fake.clone()),
            sessions: HashMap::from([(
                "takeover-continuity".to_string(),
                NetworkRuntimeSession {
                    network_id: Some(network_id.to_string()),
                    signaling: Some(signaling.clone()),
                    coordinator_recovery: None,
                    coordinator_emergency: None,
                    coordinator_continuity: CoordinatorContinuityMetrics::default(),
                    tun_name: "fuckipv6-takeover-continuity".into(),
                    routing_control: routing_tx,
                    shutdown: shutdown_tx.clone(),
                    handle: data_plane,
                },
            )]),
            retained_owners: HashMap::new(),
            cleanup_failures: BTreeMap::new(),
            completed_cleanup: BTreeMap::new(),
            timeouts: RuntimeTimeouts::default(),
            coordinator_takeover_wakeups: None,
        };

        let response_client = signaling.clone();
        let expected_network_id = network_id.clone();
        let responder = tokio::spawn(async move {
            let status = CoordinatorCandidateStatus {
                network_id: expected_network_id.clone(),
                generated_at_unix_ms: 1_000,
                config_state: CoordinatorCandidateConfigState::Missing,
                config: None,
                candidates: Vec::new(),
                current_leader: None,
                latest_term: None,
                lease: CoordinatorLeaseStatus::no_term(),
                control_plane: Default::default(),
                emergency: CoordinatorEmergencyState::NoCandidatesConfigured,
                emergency_leader: None,
                latest_election: None,
            };
            for round in 0..2 {
                let request = timeout(Duration::from_secs(1), outbound.recv())
                    .await
                    .unwrap_or_else(|_| panic!("takeover round {round} must request status"))
                    .expect("outbound signaling channel must stay open");
                let SignalingMessage::CoordinatorCandidateStatusRequest {
                    network_id: requested,
                } = request
                else {
                    panic!("expected coordinator status request, got {request:?}");
                };
                assert_eq!(requested, expected_network_id);
                assert!(response_client.route_coordinator_candidate_test_response(
                    SignalingMessage::Error {
                        code: 503,
                        message: "coordinator temporarily unavailable".into(),
                    },
                ));
            }

            let request = timeout(Duration::from_secs(1), outbound.recv())
                .await
                .expect("recovery round must request status")
                .expect("outbound signaling channel must stay open");
            let SignalingMessage::CoordinatorCandidateStatusRequest {
                network_id: requested,
            } = request
            else {
                panic!("expected coordinator status request, got {request:?}");
            };
            assert_eq!(requested, expected_network_id);
            assert!(response_client.route_coordinator_candidate_test_response(
                SignalingMessage::CoordinatorCandidateStatusAck {
                    status: status.clone(),
                },
            ));

            let request = timeout(Duration::from_secs(1), outbound.recv())
                .await
                .expect("recovery round must request recovery")
                .expect("outbound signaling channel must stay open");
            let SignalingMessage::CoordinatorRecoveryRequest { report } = request else {
                panic!("expected coordinator recovery request, got {request:?}");
            };
            assert_eq!(report.network_id, expected_network_id);
            assert_eq!(report.observed_audit_sequence, 0);
            let recovery = CoordinatorRecoveryStatus {
                network_id: expected_network_id.clone(),
                stage: fuckipv6_common::domain::CoordinatorRecoveryStage::ReadOnly,
                role: fuckipv6_common::domain::CoordinatorRecoveryRole::Candidate,
                governance_writable: false,
                health_checks_allowed: true,
                data_plane_allowed: true,
                authoritative_term: 0,
                authoritative_fencing_token: fuckipv6_common::domain::FencingToken(0),
                authoritative_leader: NodeId::from_public_key(b"recovery-test-leader"),
                authoritative_candidate_config_revision: 0,
                authoritative_audit_sequence: 7,
                reason_code: "coordinator.recovery.stale_old_leader".into(),
            };
            assert!(response_client.route_coordinator_candidate_test_response(
                SignalingMessage::CoordinatorRecoveryResponse {
                    response: fuckipv6_common::domain::CoordinatorRecoveryResponse {
                        status: status.clone(),
                        recovery,
                        audit_entries: Vec::new(),
                    },
                },
            ));
        });

        timeout(Duration::from_secs(1), actor.refresh_coordinator_takeover())
            .await
            .expect("coordinator takeover poll timed out");
        let session = actor
            .sessions
            .get("takeover-continuity")
            .expect("data-plane session must survive the first control-plane failure");
        assert!(!session.handle.is_finished());
        assert!(!*shutdown_tx.borrow());
        assert_eq!(
            session.coordinator_continuity.control_plane_interruptions,
            1
        );
        assert_eq!(session.coordinator_continuity.consecutive_failures, 1);
        assert!(session.coordinator_continuity.unavailable_since.is_some());
        assert!(session.coordinator_continuity.last_failure_reason.is_some());

        timeout(Duration::from_secs(1), actor.refresh_coordinator_takeover())
            .await
            .expect("second coordinator takeover poll timed out");
        let session = actor
            .sessions
            .get("takeover-continuity")
            .expect("data-plane session must survive repeated control-plane failures");
        assert!(!session.handle.is_finished());
        assert!(!*shutdown_tx.borrow());
        assert_eq!(
            session.coordinator_continuity.control_plane_interruptions,
            1
        );
        assert_eq!(session.coordinator_continuity.consecutive_failures, 2);
        assert_eq!(session.coordinator_continuity.recoveries, 0);

        timeout(Duration::from_secs(1), actor.refresh_coordinator_takeover())
            .await
            .expect("recovery coordinator takeover poll timed out");
        responder.await.expect("responder task failed");

        let session = actor
            .sessions
            .get("takeover-continuity")
            .expect("data-plane session must not be removed");
        assert!(session
            .signaling
            .as_ref()
            .is_some_and(|current| Arc::ptr_eq(current, &signaling)));
        assert!(!session.handle.is_finished());
        assert!(!*shutdown_tx.borrow());
        let recovery = session
            .coordinator_recovery
            .as_ref()
            .expect("recovery cursor must be retained");
        assert!(!recovery.governance_writable);
        assert!(recovery.health_checks_allowed);
        assert!(recovery.data_plane_allowed);
        assert_eq!(recovery.authoritative_audit_sequence, 7);
        assert_eq!(
            session.coordinator_continuity.control_plane_interruptions,
            1
        );
        assert_eq!(session.coordinator_continuity.consecutive_failures, 0);
        assert_eq!(session.coordinator_continuity.recoveries, 1);
        assert!(session.coordinator_continuity.unavailable_since.is_none());
        assert!(session
            .coordinator_continuity
            .last_recovery_duration
            .is_some());
        assert!(session.coordinator_continuity.last_failure_reason.is_none());
        assert!(matches!(
            terminated_rx.try_recv(),
            Err(tokio::sync::oneshot::error::TryRecvError::Empty)
        ));

        let session = actor
            .sessions
            .remove("takeover-continuity")
            .expect("test session");
        session.shutdown.send(true).expect("stop test session");
        timeout(Duration::from_secs(1), session.handle)
            .await
            .expect("test data-plane session did not stop")
            .expect("test data-plane task failed");
        assert_eq!(
            terminated_rx.await.expect("termination signal must arrive"),
            ()
        );
        harness.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn coordinator_takeover_reconnect_wakeup_refreshes_without_waiting_for_poll() {
        use fuckipv6_common::domain::{
            CoordinatorCandidateConfigState, CoordinatorEmergencyState, CoordinatorLeaseStatus,
        };
        use fuckipv6_common::SignalingMessage;

        let harness = TestHarness::new("coordinator-takeover-wakeup").await;
        harness
            .configs
            .create_network("takeover-wakeup", "ws://127.0.0.1:6666", true)
            .expect("create network configuration");
        let network_id = NetworkId::from_public_key(b"coordinator-takeover-wakeup");
        harness
            .configs
            .set_network_id_metadata("takeover-wakeup", network_id.as_str())
            .expect("bind network id");

        let (signaling, mut outbound) = SignalingClient::test_channel();
        let signaling = Arc::new(signaling);
        let (shutdown_tx, mut shutdown_rx) = watch::channel(false);
        let (routing_tx, _routing_rx) = mpsc::unbounded_channel();
        let data_plane = tokio::spawn(async move {
            let _ = shutdown_rx.changed().await;
            NetworkRuntimeExit {
                cleanup: RuntimeCleanupOutcome::default(),
                retained_owner: None,
            }
        });

        let (command_tx, command_rx) = mpsc::channel(4);
        let (wakeup_tx, wakeup_rx) = mpsc::unbounded_channel();
        let actor = NetworkRuntimeActor {
            configs: harness.configs.clone(),
            options: RuntimeOptions::default(),
            state: harness.state.clone(),
            events: harness.events.clone(),
            lifecycle: harness.lifecycle.clone(),
            registry: harness.registry.clone(),
            services: harness.services.clone(),
            launcher: Arc::new(harness.fake.clone()),
            sessions: HashMap::from([(
                "takeover-wakeup".to_string(),
                NetworkRuntimeSession {
                    network_id: Some(network_id.to_string()),
                    signaling: Some(signaling.clone()),
                    coordinator_recovery: None,
                    coordinator_emergency: None,
                    coordinator_continuity: CoordinatorContinuityMetrics::default(),
                    tun_name: "fuckipv6-takeover-wakeup".into(),
                    routing_control: routing_tx,
                    shutdown: shutdown_tx.clone(),
                    handle: data_plane,
                },
            )]),
            retained_owners: HashMap::new(),
            cleanup_failures: BTreeMap::new(),
            completed_cleanup: BTreeMap::new(),
            timeouts: RuntimeTimeouts {
                local_exit_update: Duration::ZERO,
                ..RuntimeTimeouts::default()
            },
            coordinator_takeover_wakeups: Some(wakeup_rx),
        };
        let actor_task = tokio::spawn(run_actor(command_rx, actor));

        let response_client = signaling.clone();
        let expected_network_id = network_id.clone();
        let responder = tokio::spawn(async move {
            let status = CoordinatorCandidateStatus {
                network_id: expected_network_id.clone(),
                generated_at_unix_ms: 1_000,
                config_state: CoordinatorCandidateConfigState::Missing,
                config: None,
                candidates: Vec::new(),
                current_leader: None,
                latest_term: None,
                lease: CoordinatorLeaseStatus::no_term(),
                control_plane: Default::default(),
                emergency: CoordinatorEmergencyState::NoCandidatesConfigured,
                emergency_leader: None,
                latest_election: None,
            };

            // The regular first poll cannot arrive for at least 4.5 seconds.
            // A queued reconnect wakeup must refresh authority well before that.
            let request = timeout(Duration::from_secs(2), outbound.recv())
                .await
                .expect("reconnect wakeup did not refresh coordinator authority")
                .expect("outbound signaling channel closed");
            let SignalingMessage::CoordinatorCandidateStatusRequest {
                network_id: requested,
            } = request
            else {
                panic!("expected coordinator status request, got {request:?}");
            };
            assert_eq!(requested, expected_network_id);
            assert!(response_client.route_coordinator_candidate_test_response(
                SignalingMessage::CoordinatorCandidateStatusAck {
                    status: status.clone(),
                },
            ));

            let request = timeout(Duration::from_secs(2), outbound.recv())
                .await
                .expect("reconnect refresh did not request recovery authority")
                .expect("outbound signaling channel closed");
            let SignalingMessage::CoordinatorRecoveryRequest { report } = request else {
                panic!("expected coordinator recovery request, got {request:?}");
            };
            assert_eq!(report.network_id, expected_network_id);
            let recovery = CoordinatorRecoveryStatus {
                network_id: expected_network_id.clone(),
                stage: fuckipv6_common::domain::CoordinatorRecoveryStage::ReadOnly,
                role: fuckipv6_common::domain::CoordinatorRecoveryRole::Candidate,
                governance_writable: false,
                health_checks_allowed: true,
                data_plane_allowed: true,
                authoritative_term: 0,
                authoritative_fencing_token: fuckipv6_common::domain::FencingToken(0),
                authoritative_leader: NodeId::from_public_key(b"reconnect-test-leader"),
                authoritative_candidate_config_revision: 0,
                authoritative_audit_sequence: 11,
                reason_code: "coordinator.recovery.stale_old_leader".into(),
            };
            assert!(response_client.route_coordinator_candidate_test_response(
                SignalingMessage::CoordinatorRecoveryResponse {
                    response: fuckipv6_common::domain::CoordinatorRecoveryResponse {
                        status,
                        recovery,
                        audit_entries: Vec::new(),
                    },
                },
            ));
        });

        wakeup_tx
            .send(())
            .expect("coordinator takeover wakeup receiver closed");
        responder.await.expect("responder task failed");

        let operation = OperationGate::new()
            .try_acquire()
            .expect("acquire shutdown operation");
        let (reply, response) = oneshot::channel();
        command_tx
            .send(RuntimeControlCommand::Shutdown {
                _operation: operation,
                reply,
            })
            .await
            .expect("runtime actor command channel closed");
        timeout(Duration::from_secs(2), response)
            .await
            .expect("runtime actor shutdown timed out")
            .expect("runtime actor shutdown response closed")
            .expect("runtime actor shutdown failed");
        timeout(Duration::from_secs(2), actor_task)
            .await
            .expect("runtime actor did not exit")
            .expect("runtime actor task failed");
        harness.shutdown().await;
    }

    #[tokio::test]
    async fn emergency_issue_ack_for_remote_leader_does_not_enable_local_authority() {
        use fuckipv6_common::domain::{
            CapabilityKind, CoordinatorCandidate, CoordinatorCandidateConfig,
            CoordinatorEmergencyLeaderStatus, CoordinatorEmergencyState, CoordinatorRecoveryRole,
            CoordinatorRecoveryStage, CoordinatorRecoveryStatus, ExitContributionMode,
            ExitContributionObligation, ExitContributionScope, ExitSelectionPolicy,
            NetworkCreateProfile, NetworkJoinMode, NetworkRecord, NetworkVisibility, PublicKey,
        };
        use fuckipv6_common::{Identity, SignalingMessage};

        let harness = TestHarness::new("coordinator-emergency-remote-leader").await;
        let local_identity = harness
            .configs
            .load_node_identity()
            .expect("load local identity");
        let local_node = NodeId::from_public_key(&local_identity.public_key());
        let candidate_identity = Identity::generate().expect("candidate identity");
        let remote_identity = Identity::generate().expect("remote leader identity");
        let candidate_node = NodeId::from_public_key(&candidate_identity.public_key());
        let remote_node = NodeId::from_public_key(&remote_identity.public_key());
        let now_unix_ms = unix_time_ms();

        let mut network = NetworkRecord::create_owned(
            NetworkCreateProfile {
                name: "emergency-remote-leader".into(),
                purpose: "Verify runtime emergency authority binding".into(),
                icon: "network".into(),
                language: "zh-CN".into(),
                region: "ZZ".into(),
                tags: vec![],
                capability_declarations: vec![CapabilityKind::Coordinator],
                visibility: NetworkVisibility::Private,
                join_mode: NetworkJoinMode::InviteOnly,
                exit_contribution: ExitContributionObligation::NotRequired,
                exit_contribution_scope: ExitContributionScope::NotApplicable,
                exit_contribution_mode: ExitContributionMode::Disabled,
                exit_selection_policy: ExitSelectionPolicy::default(),
            },
            PublicKey::from_bytes([71; 32]),
            local_node.clone(),
            now_unix_ms.saturating_sub(10_000),
        )
        .expect("create network record");
        network
            .members
            .extend([candidate_node.clone(), remote_node.clone()]);
        network.validate().expect("validate network record");
        let network_id = network.network_id.clone();
        harness
            .configs
            .create_network("emergency-remote-leader", "ws://127.0.0.1:6666", true)
            .expect("create network configuration");
        harness
            .configs
            .set_network_id_metadata("emergency-remote-leader", network_id.as_str())
            .expect("bind network id");

        let config = CoordinatorCandidateConfig::signed(
            network_id.clone(),
            1,
            vec![CoordinatorCandidate {
                node_id: candidate_node.clone(),
                priority: 10,
            }],
            now_unix_ms,
            now_unix_ms + 60_000,
            &local_identity,
        )
        .expect("sign candidate config");
        let status =
            CoordinatorCandidateStatus::assess(&network, Some(&config), &[], None, now_unix_ms)
                .expect("assess unavailable candidates");
        assert_eq!(
            status.emergency,
            CoordinatorEmergencyState::AllCandidatesUnavailable
        );
        let lease = CoordinatorEmergencyLeaderStatus::issue(
            &network,
            &config,
            std::slice::from_ref(&remote_node),
            1,
            now_unix_ms,
        )
        .expect("issue remote emergency lease");
        let mut response_status = status.clone();
        response_status
            .attach_emergency_leader(Some(lease), now_unix_ms)
            .expect("attach emergency lease");

        let (signaling, mut outbound) = SignalingClient::test_channel();
        let signaling = Arc::new(signaling);
        let (shutdown_tx, mut shutdown_rx) = watch::channel(false);
        let (routing_tx, _routing_rx) = mpsc::unbounded_channel();
        let (terminated_tx, _terminated_rx) = oneshot::channel();
        let data_plane = tokio::spawn(async move {
            let _ = shutdown_rx.changed().await;
            let _ = terminated_tx.send(());
            NetworkRuntimeExit {
                cleanup: RuntimeCleanupOutcome::default(),
                retained_owner: None,
            }
        });
        let mut actor = NetworkRuntimeActor {
            configs: harness.configs.clone(),
            options: RuntimeOptions::default(),
            state: harness.state.clone(),
            events: harness.events.clone(),
            lifecycle: harness.lifecycle.clone(),
            registry: harness.registry.clone(),
            services: harness.services.clone(),
            launcher: Arc::new(harness.fake.clone()),
            sessions: HashMap::from([(
                "emergency-remote-leader".to_string(),
                NetworkRuntimeSession {
                    network_id: Some(network_id.to_string()),
                    signaling: Some(signaling.clone()),
                    coordinator_recovery: None,
                    coordinator_emergency: None,
                    coordinator_continuity: CoordinatorContinuityMetrics::default(),
                    tun_name: "fuckipv6-emergency-remote-leader".into(),
                    routing_control: routing_tx,
                    shutdown: shutdown_tx.clone(),
                    handle: data_plane,
                },
            )]),
            retained_owners: HashMap::new(),
            cleanup_failures: BTreeMap::new(),
            completed_cleanup: BTreeMap::new(),
            timeouts: RuntimeTimeouts::default(),
            coordinator_takeover_wakeups: None,
        };

        let response_client = signaling.clone();
        let expected_network_id = network_id.clone();
        let expected_status = status.clone();
        let expected_response_status = response_status.clone();
        let expected_remote_node = remote_node.clone();
        let responder = tokio::spawn(async move {
            let request = timeout(Duration::from_secs(1), outbound.recv())
                .await
                .expect("status request timed out")
                .expect("outbound signaling channel closed");
            assert!(matches!(
                request,
                SignalingMessage::CoordinatorCandidateStatusRequest { .. }
            ));
            assert!(response_client.route_coordinator_candidate_test_response(
                SignalingMessage::CoordinatorCandidateStatusAck {
                    status: expected_status.clone(),
                },
            ));

            let request = timeout(Duration::from_secs(1), outbound.recv())
                .await
                .expect("recovery request timed out")
                .expect("outbound signaling channel closed");
            let SignalingMessage::CoordinatorRecoveryRequest { report } = request else {
                panic!("expected coordinator recovery request, got {request:?}");
            };
            assert_eq!(report.network_id, expected_network_id);
            let recovery = CoordinatorRecoveryStatus {
                network_id: expected_network_id.clone(),
                stage: CoordinatorRecoveryStage::ReadOnly,
                role: CoordinatorRecoveryRole::Member,
                governance_writable: false,
                health_checks_allowed: true,
                data_plane_allowed: true,
                authoritative_term: 0,
                authoritative_fencing_token: fuckipv6_common::domain::FencingToken(0),
                authoritative_leader: expected_remote_node.clone(),
                authoritative_candidate_config_revision: 1,
                authoritative_audit_sequence: 0,
                reason_code: "coordinator.recovery.emergency_runtime".into(),
            };
            assert!(response_client.route_coordinator_candidate_test_response(
                SignalingMessage::CoordinatorRecoveryResponse {
                    response: fuckipv6_common::domain::CoordinatorRecoveryResponse {
                        status: expected_status.clone(),
                        recovery,
                        audit_entries: Vec::new(),
                    },
                },
            ));

            let request = timeout(Duration::from_secs(1), outbound.recv())
                .await
                .expect("emergency issue request timed out")
                .expect("outbound signaling channel closed");
            let SignalingMessage::CoordinatorEmergencyLeaderIssue { network_id } = request else {
                panic!("expected emergency issue request, got {request:?}");
            };
            assert_eq!(network_id, expected_network_id);
            assert!(response_client.route_coordinator_candidate_test_response(
                SignalingMessage::CoordinatorEmergencyLeaderIssueAck {
                    response: fuckipv6_common::domain::CoordinatorEmergencyLeaderResponse {
                        status: expected_response_status.clone(),
                        renewed: false,
                    },
                },
            ));
        });

        timeout(Duration::from_secs(1), actor.refresh_coordinator_takeover())
            .await
            .expect("coordinator takeover poll timed out");
        responder.await.expect("responder task failed");

        let session = actor
            .sessions
            .get("emergency-remote-leader")
            .expect("data-plane session must remain registered");
        assert!(session.coordinator_emergency.is_none());
        assert_eq!(
            session
                .coordinator_emergency
                .as_ref()
                .map(|lease| &lease.leader),
            None
        );
        assert!(!session.handle.is_finished());
        assert!(!*shutdown_tx.borrow());

        let session = actor
            .sessions
            .remove("emergency-remote-leader")
            .expect("test session");
        session.shutdown.send(true).expect("stop test session");
        timeout(Duration::from_secs(1), session.handle)
            .await
            .expect("test data-plane session did not stop")
            .expect("test data-plane task failed");
        harness.shutdown().await;
    }

    #[tokio::test]
    async fn local_exit_enable_disable_is_idempotent_and_preserves_network_runtime() {
        let harness = TestHarness::new("local-exit-toggle").await;
        start_fake_network(&harness, "local-exit-toggle").await;
        set_supported_exit_capabilities(&harness).await;
        harness
            .fake
            .set_local_exit_clients("local-exit-toggle", 3, 9);

        harness
            .runtime
            .update_routing(routing_command(
                "local-exit-toggle",
                NodeRoutingMode::GlobalProxy,
                Some("office"),
                true,
            ))
            .await
            .expect("enable network exit role");

        let enabled = harness
            .runtime
            .set_local_exit_capability(true)
            .await
            .expect("enable local exit");
        assert!(enabled.owner_exit_enabled);
        assert!(enabled.network_allowed);
        assert!(enabled.healthy);
        assert_eq!(enabled.stage, LocalExitCapabilityStage::Enabled);
        assert_eq!(enabled.active_clients, 3);
        assert_eq!(enabled.total_clients, 9);
        assert_eq!(enabled.problem, None);
        assert!(enabled.last_transition_at_unix_ms > 0);
        assert!(harness.configs.node_local_exit_enabled().unwrap());

        let enabled_again = harness
            .runtime
            .set_local_exit_capability(true)
            .await
            .expect("repeat enable local exit");
        assert_eq!(enabled_again.stage, LocalExitCapabilityStage::Enabled);
        assert!(enabled_again.owner_exit_enabled);

        let disabled = harness
            .runtime
            .set_local_exit_capability(false)
            .await
            .expect("disable local exit");
        assert!(!disabled.owner_exit_enabled);
        assert!(!disabled.healthy);
        assert_eq!(disabled.stage, LocalExitCapabilityStage::Disabled);
        assert!(!harness.configs.node_local_exit_enabled().unwrap());

        let disabled_again = harness
            .runtime
            .set_local_exit_capability(false)
            .await
            .expect("repeat disable local exit");
        assert_eq!(disabled_again.stage, LocalExitCapabilityStage::Disabled);
        assert_eq!(harness.fake.launches(), vec!["local-exit-toggle"]);
        assert_eq!(harness.fake.shutdown_count(), 0);
        let calls = harness.fake.local_exit_calls();
        assert!(calls
            .iter()
            .any(|(network, enabled)| { network == "local-exit-toggle" && *enabled }));
        assert!(calls
            .iter()
            .any(|(network, enabled)| { network == "local-exit-toggle" && !*enabled }));

        harness.shutdown().await;
    }

    #[tokio::test]
    async fn local_exit_toggle_preserves_other_capabilities_and_embedded_services() {
        let harness = TestHarness::new("local-exit-isolation").await;
        start_fake_network(&harness, "local-exit-isolation").await;
        set_supported_exit_capabilities(&harness).await;
        let embedded_service = NodeEmbeddedServiceSnapshot {
            network_name: "local-exit-isolation".into(),
            status: NodeEmbeddedServiceStatus::Running,
            relay_status: NodeEmbeddedServiceStatus::Running,
            signaling_addr: Some("127.0.0.1:46666".parse().expect("signaling address")),
            relay_addr: Some("127.0.0.1:46667".parse().expect("relay address")),
            relay_health: Some(healthy_relay_health()),
            relay_authorization: None,
            relay_governance: None,
            coordinator_candidates: None,
        };
        harness
            .state
            .update(|snapshot| {
                snapshot.embedded_services = vec![embedded_service.clone()];
                let network = snapshot
                    .networks
                    .iter_mut()
                    .find(|network| network.name == "local-exit-isolation")
                    .expect("running network");
                network.peer_count = 2;
            })
            .await;
        harness
            .runtime
            .update_routing(routing_command(
                "local-exit-isolation",
                NodeRoutingMode::GlobalProxy,
                Some("office"),
                true,
            ))
            .await
            .expect("enable network exit role");

        let before = harness.state.snapshot().await;
        let before_network = before
            .networks
            .iter()
            .find(|network| network.name == "local-exit-isolation")
            .expect("running network");
        assert_eq!(
            before
                .capabilities
                .state(fuckipv6_common::domain::CapabilityKind::Coordinator),
            fuckipv6_common::domain::CapabilityState::Enabled
        );
        assert_eq!(
            before
                .capabilities
                .state(fuckipv6_common::domain::CapabilityKind::Relay),
            fuckipv6_common::domain::CapabilityState::Enabled
        );

        harness
            .runtime
            .set_local_exit_capability(true)
            .await
            .expect("enable local exit");
        harness
            .runtime
            .set_local_exit_capability(false)
            .await
            .expect("disable local exit");

        let after = harness.state.snapshot().await;
        let after_network = after
            .networks
            .iter()
            .find(|network| network.name == "local-exit-isolation")
            .expect("running network");
        assert_eq!(after.embedded_services, before.embedded_services);
        assert_eq!(after.embedded_services, vec![embedded_service]);
        for capability in [
            fuckipv6_common::domain::CapabilityKind::Coordinator,
            fuckipv6_common::domain::CapabilityKind::Relay,
            fuckipv6_common::domain::CapabilityKind::File,
        ] {
            assert_eq!(
                after.capabilities.state(capability),
                before.capabilities.state(capability),
                "local Exit toggle must not change {capability:?}"
            );
        }
        assert_eq!(
            after
                .capabilities
                .state(fuckipv6_common::domain::CapabilityKind::File),
            fuckipv6_common::domain::CapabilityState::Unsupported
        );
        assert_eq!(after_network.roles, before_network.roles);
        assert_eq!(after_network.peer_count, before_network.peer_count);
        assert_eq!(harness.fake.launches(), vec!["local-exit-isolation"]);
        assert_eq!(harness.fake.shutdown_count(), 0);

        harness.shutdown().await;
    }

    #[tokio::test]
    async fn local_exit_disable_observes_cleaning_until_runtime_confirms_cleanup() {
        let harness = TestHarness::new("local-exit-drain").await;
        start_fake_network(&harness, "local-exit-drain").await;
        set_supported_exit_capabilities(&harness).await;
        harness
            .runtime
            .update_routing(routing_command(
                "local-exit-drain",
                NodeRoutingMode::GlobalProxy,
                Some("office"),
                true,
            ))
            .await
            .expect("enable network exit role");
        harness
            .runtime
            .set_local_exit_capability(true)
            .await
            .expect("enable local exit");
        harness
            .fake
            .set_local_exit_drain_responses("local-exit-drain", 3);
        let disable_calls_before = harness
            .fake
            .local_exit_calls()
            .into_iter()
            .filter(|(network, enabled)| network == "local-exit-drain" && !enabled)
            .count();

        let runtime = harness.runtime.clone();
        let disable = tokio::spawn(async move { runtime.set_local_exit_capability(false).await });
        timeout(Duration::from_secs(2), async {
            loop {
                if harness.state.snapshot().await.local_exit.stage
                    == LocalExitCapabilityStage::Cleaning
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("draining runtime must remain visible as Cleaning");

        let snapshot = disable
            .await
            .expect("disable task must not panic")
            .expect("disable local exit after cleanup");
        assert!(!snapshot.owner_exit_enabled);
        assert!(!snapshot.healthy);
        assert_eq!(snapshot.stage, LocalExitCapabilityStage::Disabled);
        assert_eq!(
            harness.state.snapshot().await.local_exit.stage,
            LocalExitCapabilityStage::Disabled
        );
        assert_eq!(
            harness.fake.runtime_local_exit_owner("local-exit-drain"),
            Some(false)
        );
        assert_eq!(
            harness
                .fake
                .local_exit_calls()
                .into_iter()
                .filter(|(network, enabled)| network == "local-exit-drain" && !enabled)
                .count()
                - disable_calls_before,
            1,
            "cleanup polling must query status without repeating disable"
        );

        harness.shutdown().await;
    }

    #[tokio::test]
    async fn command_bus_local_exit_capability_uses_the_live_runtime_actor() {
        let harness = TestHarness::new("local-exit-command-bus").await;
        start_fake_network(&harness, "local-exit-command-bus").await;
        set_supported_exit_capabilities(&harness).await;
        harness
            .runtime
            .update_routing(routing_command(
                "local-exit-command-bus",
                NodeRoutingMode::GlobalProxy,
                Some("office"),
                true,
            ))
            .await
            .expect("enable network exit role");
        let bus = crate::node::spawn_command_bus_with_runtime(
            harness.state.clone(),
            MultiNetworkRegistry::new(),
            harness.events.clone(),
            harness.services.clone(),
            harness.runtime.clone(),
            harness.configs.clone(),
        );

        let result = bus
            .execute_test(NodeCommandRequest::new(
                725,
                NodeCommand::SetLocalExitCapability(NodeSetLocalExitCapabilityCommand {
                    enabled: true,
                }),
            ))
            .await
            .expect("local exit command result");
        assert!(result.is_success());
        let NodeCommandOutcome::LocalExitCapability(snapshot) = result.outcome else {
            panic!("unexpected local exit command outcome");
        };
        assert!(snapshot.owner_exit_enabled);
        assert!(snapshot.network_allowed);
        assert!(snapshot.healthy);
        assert_eq!(snapshot.stage, LocalExitCapabilityStage::Enabled);
        assert_eq!(
            harness.state.snapshot().await.local_exit.stage,
            LocalExitCapabilityStage::Enabled
        );
        assert!(harness.configs.node_local_exit_enabled().unwrap());
        assert_eq!(harness.fake.launches(), vec!["local-exit-command-bus"]);
        assert_eq!(harness.fake.shutdown_count(), 0);

        harness.shutdown().await;
    }

    #[tokio::test]
    async fn local_exit_client_counts_follow_runtime_authorization_aggregate() {
        let harness = TestHarness::new_with_timeouts(
            "local-exit-client-refresh",
            RuntimeTimeouts {
                local_exit_refresh: Duration::from_millis(20),
                ..RuntimeTimeouts::default()
            },
        )
        .await;
        let names = ["local-exit-clients-a", "local-exit-clients-b"];
        for network_name in names {
            harness
                .configs
                .create_network(network_name, "ws://127.0.0.1:6666", false)
                .expect("create test config");
        }
        let plan = NodeRuntimePlan::load(&harness.configs, NodeNetworkSelection::All)
            .expect("load test plan");
        harness
            .runtime
            .start_plan(plan)
            .await
            .expect("start fake networks");
        set_supported_exit_capabilities(&harness).await;
        for network_name in names {
            harness
                .runtime
                .update_routing(routing_command(
                    network_name,
                    NodeRoutingMode::GlobalProxy,
                    Some("office"),
                    true,
                ))
                .await
                .expect("enable network exit role");
        }

        let enabled = harness
            .runtime
            .set_local_exit_capability(true)
            .await
            .expect("enable local exit");
        assert_eq!((enabled.active_clients, enabled.total_clients), (0, 0));

        harness
            .fake
            .set_local_exit_clients("local-exit-clients-a", 1, 1);
        harness
            .fake
            .set_local_exit_clients("local-exit-clients-b", 2, 3);

        timeout(Duration::from_secs(2), async {
            loop {
                let local_exit = harness.state.snapshot().await.local_exit;
                if (local_exit.active_clients, local_exit.total_clients) == (3, 4) {
                    assert!(local_exit.owner_exit_enabled);
                    assert!(local_exit.healthy);
                    assert_eq!(local_exit.stage, LocalExitCapabilityStage::Enabled);
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("periodic refresh must aggregate active client counts");

        harness.shutdown().await;
    }

    #[tokio::test]
    async fn local_exit_toggle_operations_surface_authoritative_lifecycle_stages() {
        let harness = TestHarness::new("local-exit-operation").await;
        start_fake_network(&harness, "local-exit-operation").await;
        set_supported_exit_capabilities(&harness).await;
        harness
            .runtime
            .update_routing(routing_command(
                "local-exit-operation",
                NodeRoutingMode::GlobalProxy,
                Some("office"),
                true,
            ))
            .await
            .expect("enable network exit role");
        let bus = crate::node::spawn_command_bus_with_runtime(
            harness.state.clone(),
            MultiNetworkRegistry::new(),
            harness.events.clone(),
            harness.services.clone(),
            harness.runtime.clone(),
            harness.configs.clone(),
        );
        let operations = crate::node::OperationService::in_memory(
            bus,
            harness.state.clone(),
            harness.events.clone(),
        );
        let context = crate::node::NodeCommandContext::local_owner_for_snapshot(
            &harness.state.snapshot().await,
            "local-exit-operation",
        );

        let submit = |request_id, enabled, key| {
            operations.submit(
                context.clone(),
                NodeCommandRequest::new(
                    request_id,
                    NodeCommand::SetLocalExitCapability(NodeSetLocalExitCapabilityCommand {
                        enabled,
                    }),
                ),
                fuckipv6_common::domain::IdempotencyKey::from_public_key(key),
            )
        };
        let (enable, created) = submit(1300, true, b"local-exit-enable-operation")
            .await
            .expect("submit local exit enable operation");
        assert!(created);
        let enable = timeout(Duration::from_secs(2), async {
            loop {
                let snapshot = operations
                    .lookup(enable.operation.operation_id.as_str())
                    .expect("enable operation remains registered");
                if snapshot.operation.state.is_terminal() {
                    break snapshot;
                }
                sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("enable operation must finish");
        assert_eq!(
            enable.operation.kind,
            fuckipv6_common::domain::OperationKind::CapabilityEnable
        );
        assert_eq!(
            enable.operation.state,
            fuckipv6_common::domain::OperationState::Succeeded
        );
        let Some(result) = enable.result else {
            panic!("enable operation must expose its result");
        };
        let NodeCommandOutcome::LocalExitCapability(enabled) = result.outcome else {
            panic!("enable operation must return the local Exit lifecycle");
        };
        assert_eq!(enabled.stage, LocalExitCapabilityStage::Enabled);
        assert_eq!(
            harness.state.snapshot().await.local_exit.stage,
            LocalExitCapabilityStage::Enabled
        );

        let (disable, created) = submit(1301, false, b"local-exit-disable-operation")
            .await
            .expect("submit local exit disable operation");
        assert!(created);
        let disable = timeout(Duration::from_secs(2), async {
            loop {
                let snapshot = operations
                    .lookup(disable.operation.operation_id.as_str())
                    .expect("disable operation remains registered");
                if snapshot.operation.state.is_terminal() {
                    break snapshot;
                }
                sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("disable operation must finish");
        assert_eq!(
            disable.operation.kind,
            fuckipv6_common::domain::OperationKind::CapabilityDisable
        );
        assert_eq!(
            disable.operation.state,
            fuckipv6_common::domain::OperationState::Succeeded
        );
        let Some(result) = disable.result else {
            panic!("disable operation must expose its result");
        };
        let NodeCommandOutcome::LocalExitCapability(disabled) = result.outcome else {
            panic!("disable operation must return the local Exit lifecycle");
        };
        assert_eq!(disabled.stage, LocalExitCapabilityStage::Disabled);
        assert_eq!(
            harness.state.snapshot().await.local_exit.stage,
            LocalExitCapabilityStage::Disabled
        );

        harness.shutdown().await;
    }

    #[tokio::test]
    async fn local_exit_enable_without_network_authorization_waits_without_claiming_health() {
        let harness = TestHarness::new("local-exit-waiting").await;
        start_fake_network(&harness, "local-exit-waiting").await;
        set_supported_exit_capabilities(&harness).await;

        let snapshot = harness
            .runtime
            .set_local_exit_capability(true)
            .await
            .expect("enable local exit intent");
        assert!(snapshot.owner_exit_enabled);
        assert!(!snapshot.network_allowed);
        assert!(!snapshot.healthy);
        assert_eq!(
            snapshot.stage,
            LocalExitCapabilityStage::WaitingForNetworkAuthorization
        );
        assert_eq!(snapshot.problem, None);
        assert!(harness.configs.node_local_exit_enabled().unwrap());

        harness.shutdown().await;
    }

    #[tokio::test]
    async fn local_exit_restart_restores_owner_intent_but_not_network_authorization() {
        let harness = TestHarness::new("local-exit-restart").await;
        harness
            .configs
            .set_node_local_exit_enabled(true)
            .expect("persist owner intent");
        set_supported_exit_capabilities(&harness).await;
        start_fake_network(&harness, "local-exit-restart").await;

        let snapshot = harness.state.snapshot().await.local_exit;
        assert!(snapshot.owner_exit_enabled);
        assert!(!snapshot.network_allowed);
        assert_eq!(
            snapshot.stage,
            LocalExitCapabilityStage::WaitingForNetworkAuthorization
        );
        assert_eq!(
            harness.fake.runtime_local_exit_owner("local-exit-restart"),
            Some(true)
        );

        harness.shutdown().await;
    }

    #[tokio::test]
    async fn local_exit_platform_and_privilege_failures_are_structured() {
        let harness = TestHarness::new("local-exit-unsupported").await;
        harness
            .state
            .update(|snapshot| {
                snapshot.exit_node_capabilities =
                    ExitNodeCapabilities::for_platform(fuckipv6_common::ExitNodePlatform::Windows);
            })
            .await;
        let error = harness
            .runtime
            .set_local_exit_capability(true)
            .await
            .expect_err("unsupported local exit must fail");
        assert_eq!(error.problem_code(), NodeProblemCode::Unsupported);
        assert!(!error.retryable());
        let snapshot = harness.state.snapshot().await.local_exit;
        assert_eq!(snapshot.stage, LocalExitCapabilityStage::Failed);
        assert_eq!(
            snapshot.problem.as_ref().map(|problem| problem.code),
            Some(NodeProblemCode::Unsupported)
        );
        assert!(!harness.configs.node_local_exit_enabled().unwrap());
        harness.shutdown().await;

        let harness = TestHarness::new("local-exit-privilege").await;
        harness
            .state
            .update(|snapshot| {
                snapshot.exit_node_capabilities = ExitNodeCapabilities {
                    supported: false,
                    unsupported_reasons: vec![ExitNodeUnsupportedReason::PrivilegesRequired],
                    ..supported_exit_capabilities()
                };
            })
            .await;
        let error = harness
            .runtime
            .set_local_exit_capability(true)
            .await
            .expect_err("privileged local exit must fail");
        assert_eq!(error.problem_code(), NodeProblemCode::PermissionDenied);
        assert!(!error.retryable());
        assert_eq!(
            harness
                .state
                .snapshot()
                .await
                .local_exit
                .problem
                .as_ref()
                .map(|problem| problem.code),
            Some(NodeProblemCode::PermissionDenied)
        );
        harness.shutdown().await;
    }

    #[tokio::test]
    async fn local_exit_runtime_failure_rolls_back_and_remains_retryable() {
        let harness = TestHarness::new("local-exit-runtime-failure").await;
        start_fake_network(&harness, "local-exit-runtime-failure").await;
        set_supported_exit_capabilities(&harness).await;
        harness
            .fake
            .fail_next_local_exit("local-exit-runtime-failure");

        let error = harness
            .runtime
            .set_local_exit_capability(true)
            .await
            .expect_err("runtime local exit failure must be reported");
        assert_eq!(error.problem_code(), NodeProblemCode::Unavailable);
        assert!(error.retryable());
        assert!(!harness.configs.node_local_exit_enabled().unwrap());
        assert_eq!(
            harness
                .fake
                .runtime_local_exit_owner("local-exit-runtime-failure"),
            Some(false)
        );
        let snapshot = harness.state.snapshot().await.local_exit;
        assert!(!snapshot.owner_exit_enabled);
        assert_eq!(snapshot.stage, LocalExitCapabilityStage::Failed);
        assert!(snapshot
            .problem
            .as_ref()
            .is_some_and(|problem| problem.retryable));

        let retried = harness
            .runtime
            .set_local_exit_capability(true)
            .await
            .expect("retry after runtime failure");
        assert_eq!(
            retried.stage,
            LocalExitCapabilityStage::WaitingForNetworkAuthorization
        );
        harness.shutdown().await;
    }

    #[tokio::test]
    async fn local_exit_effective_unhealthy_surfaces_retryable_problem() {
        let harness = TestHarness::new("local-exit-unhealthy").await;
        start_fake_network(&harness, "local-exit-unhealthy").await;
        set_supported_exit_capabilities(&harness).await;
        harness
            .runtime
            .update_routing(routing_command(
                "local-exit-unhealthy",
                NodeRoutingMode::GlobalProxy,
                Some("office"),
                true,
            ))
            .await
            .expect("enable network exit role");
        harness
            .fake
            .set_local_exit_health("local-exit-unhealthy", false);

        let snapshot = harness
            .runtime
            .set_local_exit_capability(true)
            .await
            .expect("persist local exit owner intent");
        assert!(snapshot.owner_exit_enabled);
        assert!(snapshot.network_allowed);
        assert!(!snapshot.healthy);
        assert_eq!(snapshot.stage, LocalExitCapabilityStage::Failed);
        assert!(snapshot
            .problem
            .as_ref()
            .is_some_and(|problem| problem.retryable));
        assert!(harness.configs.node_local_exit_enabled().unwrap());

        harness.shutdown().await;
    }

    #[tokio::test]
    async fn local_exit_multi_network_failure_rolls_back_every_runtime() {
        let harness = TestHarness::new("local-exit-partial").await;
        for network_name in ["local-exit-a", "local-exit-b"] {
            harness
                .configs
                .create_network(network_name, "ws://127.0.0.1:6666", true)
                .expect("create test config");
        }
        let plan = NodeRuntimePlan::load(&harness.configs, NodeNetworkSelection::All)
            .expect("load test plan");
        let summary = harness
            .runtime
            .start_plan(plan)
            .await
            .expect("start fake networks");
        assert_eq!(summary.started.len(), 2);
        set_supported_exit_capabilities(&harness).await;
        harness.fake.fail_next_local_exit("local-exit-b");

        let error = harness
            .runtime
            .set_local_exit_capability(true)
            .await
            .expect_err("partial runtime failure must fail the operation");
        assert_eq!(error.problem_code(), NodeProblemCode::Unavailable);
        assert!(!harness.configs.node_local_exit_enabled().unwrap());
        assert_eq!(
            harness.fake.runtime_local_exit_owner("local-exit-a"),
            Some(false)
        );
        assert_eq!(
            harness.fake.runtime_local_exit_owner("local-exit-b"),
            Some(false)
        );
        let snapshot = harness.state.snapshot().await.local_exit;
        assert_eq!(snapshot.stage, LocalExitCapabilityStage::Failed);
        assert!(!snapshot.owner_exit_enabled);
        assert!(snapshot.problem.is_some());
        harness.shutdown().await;
    }

    #[tokio::test]
    async fn command_bus_hot_updates_routing_without_restarting_or_dropping_peers() {
        let harness = TestHarness::new("routing-success").await;
        start_fake_network(&harness, "routing-success").await;
        harness
            .state
            .update(|snapshot| {
                snapshot.exit_node_capabilities = fuckipv6_common::ExitNodeCapabilities {
                    schema_version: fuckipv6_common::EXIT_NODE_CAPABILITY_SCHEMA_VERSION,
                    platform: fuckipv6_common::ExitNodePlatform::Linux,
                    supported: true,
                    ipv4_forwarding: true,
                    ipv6_forwarding: true,
                    nat: true,
                    cleanup: true,
                    unsupported_reasons: Vec::new(),
                };
                let network = snapshot
                    .networks
                    .iter_mut()
                    .find(|network| network.name == "routing-success")
                    .expect("running network");
                network.peer_count = 1;
                snapshot.peers.push(peer_snapshot("routing-success"));
            })
            .await;
        let mut events = harness.events.subscribe();
        let bus = crate::node::spawn_command_bus_with_runtime(
            harness.state.clone(),
            MultiNetworkRegistry::new(),
            harness.events.clone(),
            harness.services.clone(),
            harness.runtime.clone(),
            harness.configs.clone(),
        );

        let result = bus
            .execute_test(NodeCommandRequest::new(
                600,
                NodeCommand::UpdateRouting(routing_command(
                    "routing-success",
                    NodeRoutingMode::GlobalProxy,
                    Some("office"),
                    true,
                )),
            ))
            .await
            .expect("routing command result");
        assert!(result.is_success());
        match result.outcome {
            NodeCommandOutcome::Completed { message } => {
                assert!(message.contains("GlobalProxy"));
                assert!(message.contains("office"));
            }
            outcome => panic!("unexpected outcome: {outcome:?}"),
        }

        assert_eq!(
            harness.fake.routing_calls(),
            vec![RoutingCall {
                network_name: "routing-success".into(),
                mode: NodeRoutingMode::GlobalProxy,
                exit_node: Some("office".into()),
                is_exit_node: true,
            }]
        );
        let config_path = harness
            .configs
            .get_network_config("routing-success")
            .expect("network config path");
        let persisted = ClientConfig::from_file(config_path).expect("persisted routing config");
        assert_eq!(
            persisted.routing.mode,
            fuckipv6_common::RoutingMode::GlobalProxy
        );
        assert_eq!(persisted.routing.exit_node.as_deref(), Some("office"));
        assert!(persisted.exit_node.enabled);

        let snapshot = harness.state.snapshot().await;
        let configuration = snapshot
            .configuration
            .networks
            .iter()
            .find(|network| network.network_name == "routing-success")
            .expect("shared routing config");
        assert_eq!(configuration.routing_mode, NodeRoutingMode::GlobalProxy);
        assert_eq!(configuration.exit_node.as_deref(), Some("office"));
        assert!(configuration.exit_node_enabled);
        assert_eq!(
            snapshot
                .peers
                .iter()
                .filter(|peer| peer.network_name == "routing-success")
                .count(),
            1
        );
        assert_eq!(
            snapshot
                .networks
                .iter()
                .find(|network| network.name == "routing-success")
                .expect("running network")
                .peer_count,
            1
        );
        assert_eq!(
            routing_changed_events(&mut events),
            vec![(
                "routing-success".into(),
                NodeRoutingMode::GlobalProxy,
                Some("office".into()),
                true,
            )]
        );
        assert_eq!(harness.fake.launches(), vec!["routing-success"]);
        assert_eq!(harness.fake.shutdown_count(), 0);

        harness.shutdown().await;
    }

    #[tokio::test]
    async fn command_bus_hot_updates_exit_node_role_without_restarting_or_dropping_peers() {
        let harness = TestHarness::new("role-success").await;
        start_fake_network(&harness, "role-success").await;
        harness
            .state
            .update(|snapshot| {
                snapshot.exit_node_capabilities = fuckipv6_common::ExitNodeCapabilities {
                    schema_version: fuckipv6_common::EXIT_NODE_CAPABILITY_SCHEMA_VERSION,
                    platform: fuckipv6_common::ExitNodePlatform::Linux,
                    supported: true,
                    ipv4_forwarding: true,
                    ipv6_forwarding: true,
                    nat: true,
                    cleanup: true,
                    unsupported_reasons: Vec::new(),
                };
                let network = snapshot
                    .networks
                    .iter_mut()
                    .find(|network| network.name == "role-success")
                    .expect("running network");
                network.peer_count = 1;
                snapshot.peers.push(peer_snapshot("role-success"));
            })
            .await;
        let mut events = harness.events.subscribe();
        let bus = crate::node::spawn_command_bus_with_runtime(
            harness.state.clone(),
            MultiNetworkRegistry::new(),
            harness.events.clone(),
            harness.services.clone(),
            harness.runtime.clone(),
            harness.configs.clone(),
        );

        let result = bus
            .execute_test(NodeCommandRequest::new(
                700,
                NodeCommand::SetNetworkRole(role_command(
                    "role-success",
                    NodeNetworkRole::ExitNode,
                    true,
                )),
            ))
            .await
            .expect("role command result");
        assert!(result.is_success());
        match result.outcome {
            NodeCommandOutcome::Completed { message } => {
                assert!(message.contains("ExitNode"));
                assert!(message.contains("enabled=true"));
            }
            outcome => panic!("unexpected outcome: {outcome:?}"),
        }

        assert_eq!(
            harness.fake.routing_calls(),
            vec![RoutingCall {
                network_name: "role-success".into(),
                mode: NodeRoutingMode::MeshOnly,
                exit_node: None,
                is_exit_node: true,
            }]
        );
        assert_eq!(harness.fake.runtime_exit_node("role-success"), Some(true));
        let config_path = harness
            .configs
            .get_network_config("role-success")
            .expect("network config path");
        let persisted = ClientConfig::from_file(config_path).expect("persisted role config");
        assert!(persisted.exit_node.enabled);

        let snapshot = harness.state.snapshot().await;
        let network = snapshot
            .networks
            .iter()
            .find(|network| network.name == "role-success")
            .expect("running network");
        assert_eq!(
            network.roles,
            vec![NodeNetworkRole::Member, NodeNetworkRole::ExitNode]
        );
        let configuration = snapshot
            .configuration
            .networks
            .iter()
            .find(|network| network.network_name == "role-success")
            .expect("shared role config");
        assert!(configuration.exit_node_enabled);
        assert_eq!(snapshot.peers, vec![peer_snapshot("role-success")]);
        assert_eq!(
            role_changed_events(&mut events),
            vec![(
                "role-success".into(),
                NodeNetworkRole::ExitNode,
                true,
                vec![NodeNetworkRole::Member, NodeNetworkRole::ExitNode],
            )]
        );
        assert_eq!(harness.fake.launches(), vec!["role-success"]);
        assert_eq!(harness.fake.shutdown_count(), 0);

        harness.shutdown().await;
    }

    #[tokio::test]
    async fn persisted_exit_node_role_is_restored_when_the_network_restarts() {
        let harness = TestHarness::new("role-restart").await;
        start_fake_network(&harness, "role-restart").await;
        harness
            .runtime
            .set_network_role(role_command(
                "role-restart",
                NodeNetworkRole::ExitNode,
                true,
            ))
            .await
            .expect("enable persisted exit role");

        let config_path = harness
            .configs
            .get_network_config("role-restart")
            .expect("network config path");
        assert!(
            ClientConfig::from_file(config_path)
                .expect("persisted role config")
                .exit_node
                .enabled
        );

        harness
            .runtime
            .stop(vec!["role-restart".into()], false)
            .await
            .expect("stop network before restart");
        assert_eq!(harness.fake.launches(), vec!["role-restart"]);
        assert_eq!(harness.fake.shutdown_count(), 1);

        let plan = NodeRuntimePlan::load(
            &harness.configs,
            NodeNetworkSelection::Named(vec!["role-restart".into()]),
        )
        .expect("reload persisted network plan");
        let summary = harness
            .runtime
            .start_plan(plan)
            .await
            .expect("restart network from persisted config");
        assert_eq!(summary.started, vec!["role-restart"]);
        assert_eq!(
            harness.fake.launches(),
            vec!["role-restart", "role-restart"]
        );
        assert_eq!(harness.fake.shutdown_count(), 1);

        let snapshot = harness.state.snapshot().await;
        assert_eq!(
            snapshot
                .networks
                .iter()
                .find(|network| network.name == "role-restart")
                .expect("restarted network")
                .roles,
            vec![NodeNetworkRole::Member, NodeNetworkRole::ExitNode]
        );
        assert!(
            snapshot
                .configuration
                .networks
                .iter()
                .find(|network| network.network_name == "role-restart")
                .expect("restored role config")
                .exit_node_enabled
        );

        harness.shutdown().await;
    }

    #[test]
    fn host_permission_failures_keep_network_specific_recovery() {
        let problem = runtime_control_problem(&RuntimeControlError::NetworkSetup {
            stage: NetworkSetupStage::PermissionDenied,
            message: NetworkSetupStage::PermissionDenied
                .fallback_message()
                .into(),
        });

        assert_eq!(problem.code, NodeProblemCode::PermissionDenied);
        assert_eq!(problem.category, NodeErrorCategory::Authorization);
        assert_eq!(problem.message_key, "error.network.permission_denied");
        assert!(!problem.retryable);
        assert_eq!(
            problem
                .recovery
                .as_deref()
                .map(|recovery| recovery.hint_key.as_str()),
            Some("recovery.request_access")
        );
    }

    #[test]
    fn relay_lifecycle_errors_have_stable_problem_contract() {
        let cases = [
            (
                "relay.start.platform_unsupported",
                NodeProblemCode::Unsupported,
                false,
            ),
            (
                "relay.start.permission_denied",
                NodeProblemCode::PermissionDenied,
                false,
            ),
            (
                "relay.start.address_in_use",
                NodeProblemCode::Unavailable,
                true,
            ),
            (
                "relay.start.network_unreachable",
                NodeProblemCode::Unavailable,
                true,
            ),
            (
                "relay.stop.cleanup_incomplete",
                NodeProblemCode::Unavailable,
                true,
            ),
        ];

        for (reason_code, expected_code, expected_retryable) in cases {
            let error = RuntimeControlError::RelayLifecycleFailed {
                reason_code,
                message: "Relay lifecycle failed".into(),
            };
            assert_eq!(error.problem_code(), expected_code);
            assert_eq!(error.retryable(), expected_retryable);
            assert_eq!(error.conflict_reason(), Some(reason_code));
        }
    }

    #[test]
    fn join_request_control_errors_map_to_stable_problem_contract() {
        let cases = [
            (
                JoinRequestControlError::Disconnected,
                NodeProblemCode::Unavailable,
                "join_request.error.unavailable",
                true,
            ),
            (
                JoinRequestControlError::Unavailable,
                NodeProblemCode::Unavailable,
                "join_request.error.unavailable",
                true,
            ),
            (
                JoinRequestControlError::TimedOut,
                NodeProblemCode::TimedOut,
                "join_request.error.timeout",
                true,
            ),
            (
                JoinRequestControlError::Rejected(401),
                NodeProblemCode::AuthenticationRequired,
                "join_request.error.unauthorized",
                false,
            ),
            (
                JoinRequestControlError::Rejected(403),
                NodeProblemCode::PermissionDenied,
                "join_request.error.forbidden",
                false,
            ),
            (
                JoinRequestControlError::Rejected(404),
                NodeProblemCode::NotFound,
                "join_request.error.not_found",
                false,
            ),
            (
                JoinRequestControlError::Rejected(429),
                NodeProblemCode::Unavailable,
                "join_request.error.rate_limited",
                true,
            ),
            (
                JoinRequestControlError::Rejected(503),
                NodeProblemCode::Unavailable,
                "join_request.error.overloaded",
                true,
            ),
            (
                JoinRequestControlError::InvalidResponse,
                NodeProblemCode::InvalidRequest,
                "join_request.error.unavailable",
                false,
            ),
        ];

        for (control_error, code, message_key, retryable) in cases {
            let error = map_join_request_error(control_error);
            assert_eq!(error.problem_code(), code);
            assert_eq!(error.message_key(), message_key);
            assert_eq!(error.retryable(), retryable);
        }
    }

    #[tokio::test]
    async fn review_join_requests_fails_closed_without_an_active_data_plane_session() {
        use fuckipv6_common::domain::JoinRequestReviewAction;

        let harness = TestHarness::new("join-review-no-session").await;
        let network_id = NetworkId::from_public_key(b"join-review-no-session");
        let error = harness
            .runtime
            .review_join_requests(
                network_id,
                vec![JoinRequestBatchReviewItem {
                    request_id: JoinRequestId::from_public_key(b"missing-request"),
                    expected_revision: 1,
                    action: JoinRequestReviewAction::Approve,
                    note: None,
                }],
            )
            .await
            .expect_err("review must not create a second data-plane registration");

        match error {
            RuntimeControlError::JoinRequest {
                code,
                message_key,
                message,
            } => {
                assert_eq!(code, NodeProblemCode::Unavailable);
                assert_eq!(message_key, "join_request.error.unavailable");
                assert!(message.contains("权威数据面连接"));
            }
            error => panic!("unexpected review error: {error}"),
        }

        harness.shutdown().await;
    }

    #[tokio::test]
    async fn invalid_and_unavailable_role_changes_have_no_side_effects() {
        let harness = TestHarness::new("role-rejections").await;
        start_fake_network(&harness, "role-rejections").await;
        let mut events = harness.events.subscribe();
        let bus = crate::node::spawn_command_bus_with_runtime(
            harness.state.clone(),
            MultiNetworkRegistry::new(),
            harness.events.clone(),
            harness.services.clone(),
            harness.runtime.clone(),
            harness.configs.clone(),
        );
        let invalid = bus
            .execute_test(NodeCommandRequest::new(
                710,
                NodeCommand::SetNetworkRole(role_command(
                    "role-rejections",
                    NodeNetworkRole::Member,
                    false,
                )),
            ))
            .await
            .expect_err("disabling the baseline Member role must fail validation");
        match invalid {
            NodeCommandBusError::Invalid(problem) => {
                assert_eq!(problem.code, NodeProblemCode::InvalidRequest);
                assert!(!problem.retryable);
                assert_eq!(problem.network_name.as_deref(), Some("role-rejections"));
            }
            error => panic!("unexpected command bus error: {error:?}"),
        }

        let cases = [
            (
                role_command("role-rejections", NodeNetworkRole::Member, true),
                NodeProblemCode::Conflict,
                false,
            ),
            (
                role_command("role-rejections", NodeNetworkRole::Discovery, true),
                NodeProblemCode::Unavailable,
                true,
            ),
        ];
        for (offset, (command, expected_code, expected_retryable)) in cases.into_iter().enumerate()
        {
            let result = bus
                .execute_test(NodeCommandRequest::new(
                    711 + offset as u64,
                    NodeCommand::SetNetworkRole(command),
                ))
                .await
                .expect("role rejection result");
            match result.outcome {
                NodeCommandOutcome::Rejected(problem) => {
                    assert_eq!(problem.code, expected_code);
                    assert_eq!(problem.retryable, expected_retryable);
                    assert_eq!(problem.network_name.as_deref(), Some("role-rejections"));
                }
                outcome => panic!("unexpected outcome: {outcome:?}"),
            }
        }

        assert!(harness.fake.routing_calls().is_empty());
        assert_eq!(harness.fake.launches(), vec!["role-rejections"]);
        let snapshot = harness.state.snapshot().await;
        assert_eq!(
            snapshot
                .networks
                .iter()
                .find(|network| network.name == "role-rejections")
                .expect("running network")
                .roles,
            vec![NodeNetworkRole::Member]
        );
        let config_path = harness
            .configs
            .get_network_config("role-rejections")
            .expect("network config path");
        assert!(
            !ClientConfig::from_file(config_path)
                .expect("unchanged role config")
                .exit_node
                .enabled
        );
        assert!(role_changed_events(&mut events).is_empty());
        assert_eq!(harness.fake.shutdown_count(), 0);

        harness.shutdown().await;
    }

    #[tokio::test]
    async fn concurrent_role_updates_are_serialized_and_last_write_wins() {
        let harness = TestHarness::new("role-concurrent").await;
        start_fake_network(&harness, "role-concurrent").await;
        harness.fake.set_routing_delay(Duration::from_millis(80));

        let first_runtime = harness.runtime.clone();
        let first = tokio::spawn(async move {
            first_runtime
                .set_network_role(role_command(
                    "role-concurrent",
                    NodeNetworkRole::ExitNode,
                    true,
                ))
                .await
        });
        wait_for_routing_calls(&harness.fake, 1).await;

        harness
            .runtime
            .set_network_role(role_command(
                "role-concurrent",
                NodeNetworkRole::ExitNode,
                false,
            ))
            .await
            .expect("second role update");
        first
            .await
            .expect("first role task")
            .expect("first role update");

        assert_eq!(harness.fake.max_routing_concurrency(), 1);
        assert_eq!(
            harness
                .fake
                .routing_calls()
                .into_iter()
                .map(|call| call.is_exit_node)
                .collect::<Vec<_>>(),
            vec![true, false]
        );
        let snapshot = harness.state.snapshot().await;
        assert_eq!(
            snapshot
                .networks
                .iter()
                .find(|network| network.name == "role-concurrent")
                .expect("running network")
                .roles,
            vec![NodeNetworkRole::Member]
        );
        assert!(
            !snapshot
                .configuration
                .networks
                .iter()
                .find(|network| network.network_name == "role-concurrent")
                .expect("final role config")
                .exit_node_enabled
        );
        assert_eq!(harness.fake.launches(), vec!["role-concurrent"]);
        assert_eq!(harness.fake.shutdown_count(), 0);

        harness.shutdown().await;
    }

    #[tokio::test]
    async fn role_runtime_failure_rolls_back_runtime_file_and_snapshot() {
        let harness = TestHarness::new("role-runtime-rollback").await;
        start_fake_network(&harness, "role-runtime").await;
        harness
            .state
            .update(|snapshot| {
                let network = snapshot
                    .networks
                    .iter_mut()
                    .find(|network| network.name == "role-runtime")
                    .expect("running network");
                network.peer_count = 1;
                snapshot.peers.push(peer_snapshot("role-runtime"));
            })
            .await;
        let before = harness.state.snapshot().await;
        let config_path = harness
            .configs
            .get_network_config("role-runtime")
            .expect("network config path");
        harness.fake.fail_next_routing("role-runtime");
        let mut events = harness.events.subscribe();

        let error = harness
            .runtime
            .set_network_role(role_command(
                "role-runtime",
                NodeNetworkRole::ExitNode,
                true,
            ))
            .await
            .expect_err("runtime rejection must propagate");
        assert!(matches!(
            error,
            RuntimeControlError::Persistence(ref message)
                if message.contains("运行态与持久化配置已回滚")
        ));

        let after = harness.state.snapshot().await;
        assert_eq!(after.peers, before.peers);
        assert_eq!(
            after
                .networks
                .iter()
                .find(|network| network.name == "role-runtime")
                .expect("running network")
                .roles,
            vec![NodeNetworkRole::Member]
        );
        assert_eq!(harness.fake.runtime_exit_node("role-runtime"), Some(false));
        assert!(
            !ClientConfig::from_file(config_path)
                .expect("rolled back role config")
                .exit_node
                .enabled
        );
        assert!(role_changed_events(&mut events).is_empty());
        assert_eq!(harness.fake.launches(), vec!["role-runtime"]);
        assert_eq!(harness.fake.shutdown_count(), 0);

        harness.shutdown().await;
    }

    #[tokio::test]
    async fn role_persistence_failure_rolls_back_runtime_and_snapshot() {
        let harness = TestHarness::new("role-persistence-rollback").await;
        start_fake_network(&harness, "role-persistence").await;
        let before = harness.state.snapshot().await;
        let config_path = harness
            .configs
            .get_network_config("role-persistence")
            .expect("network config path");
        harness
            .fake
            .fail_next_routing_persistence("role-persistence");
        let mut events = harness.events.subscribe();

        let error = harness
            .runtime
            .set_network_role(role_command(
                "role-persistence",
                NodeNetworkRole::ExitNode,
                true,
            ))
            .await
            .expect_err("persistence rejection must propagate");
        assert!(matches!(
            error,
            RuntimeControlError::Persistence(ref message)
                if message.contains("运行态与持久化配置已回滚")
        ));

        let after = harness.state.snapshot().await;
        assert_eq!(after.peers, before.peers);
        assert!(
            !after
                .configuration
                .networks
                .iter()
                .find(|network| network.network_name == "role-persistence")
                .expect("shared role config")
                .exit_node_enabled
        );
        assert_eq!(
            after
                .networks
                .iter()
                .find(|network| network.name == "role-persistence")
                .expect("running network")
                .roles,
            vec![NodeNetworkRole::Member]
        );
        assert_eq!(
            harness.fake.runtime_exit_node("role-persistence"),
            Some(false)
        );
        assert!(
            !ClientConfig::from_file(config_path)
                .expect("unchanged role config")
                .exit_node
                .enabled
        );
        assert!(role_changed_events(&mut events).is_empty());
        assert_eq!(harness.fake.launches(), vec!["role-persistence"]);
        assert_eq!(harness.fake.shutdown_count(), 0);

        harness.shutdown().await;
    }

    #[tokio::test]
    async fn role_snapshot_failure_rolls_back_runtime_file_and_shared_state() {
        let harness = TestHarness::new("role-snapshot-rollback").await;
        start_fake_network(&harness, "role-snapshot").await;
        harness
            .state
            .update(|snapshot| {
                let network = snapshot
                    .networks
                    .iter_mut()
                    .find(|network| network.name == "role-snapshot")
                    .expect("running network");
                network.peer_count = 1;
                snapshot.peers.push(peer_snapshot("role-snapshot"));
            })
            .await;
        let before = harness.state.snapshot().await;
        let before_config = before
            .configuration
            .networks
            .iter()
            .find(|network| network.network_name == "role-snapshot")
            .expect("initial role config")
            .clone();
        let config_path = harness
            .configs
            .get_network_config("role-snapshot")
            .expect("network config path");
        harness.fake.set_routing_delay(Duration::from_millis(80));
        let mut events = harness.events.subscribe();
        let update_runtime = harness.runtime.clone();
        let update = tokio::spawn(async move {
            update_runtime
                .set_network_role(role_command(
                    "role-snapshot",
                    NodeNetworkRole::ExitNode,
                    true,
                ))
                .await
        });
        wait_for_routing_calls(&harness.fake, 1).await;

        harness
            .state
            .update(|snapshot| {
                snapshot
                    .configuration
                    .networks
                    .retain(|network| network.network_name != "role-snapshot");
            })
            .await;
        let error = update
            .await
            .expect("role task")
            .expect_err("snapshot publish failure must reject the role update");
        assert!(matches!(
            error,
            RuntimeControlError::Persistence(ref message)
                if message.contains("角色共享快照发布失败")
        ));

        assert_eq!(harness.fake.runtime_exit_node("role-snapshot"), Some(false));
        assert!(
            !ClientConfig::from_file(config_path)
                .expect("rolled back role config")
                .exit_node
                .enabled
        );
        let after = harness.state.snapshot().await;
        assert_eq!(
            after
                .configuration
                .networks
                .iter()
                .find(|network| network.network_name == "role-snapshot")
                .expect("restored role config"),
            &before_config
        );
        assert_eq!(
            after
                .networks
                .iter()
                .find(|network| network.name == "role-snapshot")
                .expect("running network")
                .roles,
            vec![NodeNetworkRole::Member]
        );
        assert_eq!(after.peers, before.peers);
        assert!(role_changed_events(&mut events).is_empty());
        assert_eq!(harness.fake.launches(), vec!["role-snapshot"]);
        assert_eq!(harness.fake.shutdown_count(), 0);

        harness.shutdown().await;
    }

    #[tokio::test]
    async fn stopping_network_after_role_update_releases_embedded_service() {
        let harness = TestHarness::new("role-stop-service").await;
        start_fake_network(&harness, "role-stop").await;
        harness
            .services
            .start(
                EmbeddedServiceStartRequest::new("role-stop", 0, 0).with_external_host("127.0.0.1"),
            )
            .await
            .expect("start role-owned embedded service");
        harness
            .runtime
            .set_network_role(role_command("role-stop", NodeNetworkRole::ExitNode, true))
            .await
            .expect("enable exit role");

        let stopped = harness
            .runtime
            .stop(vec!["role-stop".into()], false)
            .await
            .expect("stop network and role-owned service");
        assert_eq!(stopped.stopped, vec!["role-stop"]);
        assert!(harness
            .services
            .list()
            .await
            .expect("list embedded services")
            .iter()
            .all(|service| service.network_name != "role-stop"));
        assert_eq!(harness.fake.shutdown_count(), 1);
        assert_eq!(harness.fake.launches(), vec!["role-stop"]);

        harness.shutdown().await;
    }

    #[tokio::test]
    async fn relay_role_toggles_independently_and_preserves_other_network_state() {
        let harness = TestHarness::new("relay-role-toggle").await;
        let network_name = "relay-role";
        start_fake_network(&harness, network_name).await;
        let started = harness
            .services
            .start(
                EmbeddedServiceStartRequest::new(network_name, 0, 0)
                    .with_external_host("127.0.0.1")
                    .with_relay_enabled(false),
            )
            .await
            .expect("start service with relay disabled");
        harness
            .configs
            .set_embedded_service_metadata(
                network_name,
                started.signaling_addr.port(),
                None,
                false,
                Some("127.0.0.1".into()),
                None,
            )
            .expect("persist embedded service metadata");
        record_running_embedded_service_for_test(&harness, &started).await;
        let before = harness.state.snapshot().await;
        let before_config = before
            .configuration
            .networks
            .iter()
            .find(|network| network.network_name == network_name)
            .cloned()
            .expect("network configuration");
        let signaling_addr = started.signaling_addr;

        let mut relay_events = harness.events.subscribe();
        let mut role_events = harness.events.subscribe();
        let enabled = harness
            .runtime
            .set_network_role(role_command(network_name, NodeNetworkRole::Relay, true))
            .await
            .expect("enable relay role");
        let relay_addr = harness
            .state
            .snapshot()
            .await
            .embedded_services
            .iter()
            .find(|service| service.network_name == network_name)
            .and_then(|service| service.relay_addr)
            .expect("running relay address");
        assert_eq!(
            enabled.roles,
            vec![NodeNetworkRole::Member, NodeNetworkRole::Relay]
        );
        assert_eq!(
            relay_changed_events(&mut relay_events, network_name),
            vec![
                NodeEmbeddedServiceStatus::Starting,
                NodeEmbeddedServiceStatus::Running,
            ]
        );
        assert_eq!(
            role_changed_events(&mut role_events),
            vec![(
                network_name.into(),
                NodeNetworkRole::Relay,
                true,
                vec![NodeNetworkRole::Member, NodeNetworkRole::Relay],
            )]
        );

        let running = harness.state.snapshot().await;
        let service = running
            .embedded_services
            .iter()
            .find(|service| service.network_name == network_name)
            .expect("embedded service");
        assert_eq!(service.status, NodeEmbeddedServiceStatus::Running);
        assert_eq!(service.relay_status, NodeEmbeddedServiceStatus::Running);
        assert_eq!(service.signaling_addr, Some(signaling_addr));
        assert_eq!(
            running
                .configuration
                .networks
                .iter()
                .find(|network| network.network_name == network_name),
            Some(&before_config),
            "enabling Relay must not rewrite Exit routing configuration"
        );
        let persisted = harness
            .configs
            .list_networks()
            .expect("load network metadata")
            .into_iter()
            .find(|network| network.name == network_name)
            .expect("persisted network");
        assert!(persisted.relay_enabled);
        assert_eq!(persisted.relay_port, Some(relay_addr.port()));

        let mut relay_events = harness.events.subscribe();
        let mut role_events = harness.events.subscribe();
        let disabled = harness
            .runtime
            .set_network_role(role_command(network_name, NodeNetworkRole::Relay, false))
            .await
            .expect("disable relay role");
        assert_eq!(disabled.roles, vec![NodeNetworkRole::Member]);
        assert_eq!(
            relay_changed_events(&mut relay_events, network_name),
            vec![
                NodeEmbeddedServiceStatus::Stopping,
                NodeEmbeddedServiceStatus::Stopped,
            ]
        );
        assert_eq!(
            role_changed_events(&mut role_events),
            vec![(
                network_name.into(),
                NodeNetworkRole::Relay,
                false,
                vec![NodeNetworkRole::Member],
            )]
        );

        let stopped = harness.state.snapshot().await;
        let service = stopped
            .embedded_services
            .iter()
            .find(|service| service.network_name == network_name)
            .expect("embedded service remains");
        assert_eq!(service.status, NodeEmbeddedServiceStatus::Running);
        assert_eq!(service.relay_status, NodeEmbeddedServiceStatus::Stopped);
        assert_eq!(service.signaling_addr, Some(signaling_addr));
        assert!(service.relay_addr.is_none());
        assert_eq!(
            stopped
                .configuration
                .networks
                .iter()
                .find(|network| network.network_name == network_name),
            Some(&before_config),
            "disabling Relay must not stop or rewrite Coordinator/Exit state"
        );
        tokio::net::TcpListener::bind(("127.0.0.1", signaling_addr.port()))
            .await
            .expect_err("signaling listener must remain active after Relay stops");
        tokio::net::UdpSocket::bind(("127.0.0.1", relay_addr.port()))
            .await
            .expect("relay listener must be released after Relay stops");
        let persisted = harness
            .configs
            .list_networks()
            .expect("load network metadata")
            .into_iter()
            .find(|network| network.name == network_name)
            .expect("persisted network");
        assert!(!persisted.relay_enabled);
        assert_eq!(
            persisted.relay_port,
            Some(relay_addr.port()),
            "disabling Relay must retain the last bound port"
        );

        harness.shutdown().await;
    }

    #[tokio::test]
    async fn failed_relay_start_is_explained_persisted_and_retryable() {
        let harness = TestHarness::new("relay-start-failure").await;
        let network_name = "relay-start-failure";
        start_fake_network(&harness, network_name).await;
        let occupied = tokio::net::UdpSocket::bind(("127.0.0.1", 0))
            .await
            .expect("occupy Relay UDP port");
        let relay_port = occupied.local_addr().expect("occupied address").port();
        let started = harness
            .services
            .start(
                EmbeddedServiceStartRequest::new(network_name, 0, relay_port)
                    .with_external_host("127.0.0.1")
                    .with_relay_enabled(false),
            )
            .await
            .expect("start signaling with Relay disabled");
        harness
            .configs
            .set_embedded_service_metadata(
                network_name,
                started.signaling_addr.port(),
                Some(relay_port),
                false,
                Some("127.0.0.1".into()),
                None,
            )
            .expect("persist embedded service metadata");
        record_running_embedded_service_for_test(&harness, &started).await;

        let stale_authorization = RelayAuthorizationAuditSnapshot {
            network_id: NetworkId::from_public_key(b"stale-relay-auth"),
            provider_node_id: fuckipv6_common::domain::NodeId::from_public_key(b"relay-provider"),
            policy_revision: 7,
            policy_updated_at_unix_ms: 1_700_000_000_000,
            existing_session_policy: RelayExistingSessionPolicy::Terminate,
            decisions: Vec::new(),
            truncated: false,
        };
        let stale_governance = RelayGovernanceAuditSnapshot {
            dropped_events: 1,
            ..Default::default()
        };
        harness
            .state
            .update(|snapshot| {
                let service = snapshot
                    .embedded_services
                    .iter_mut()
                    .find(|service| service.network_name == network_name)
                    .expect("embedded service snapshot");
                service.relay_authorization = Some(stale_authorization);
                service.relay_governance = Some(stale_governance);
            })
            .await;

        let mut relay_events = harness.events.subscribe();
        let error = harness
            .runtime
            .set_network_role(role_command(network_name, NodeNetworkRole::Relay, true))
            .await
            .expect_err("occupied UDP port must reject Relay enable");
        let RuntimeControlError::RelayLifecycleFailed { reason_code, .. } = error else {
            panic!("expected typed Relay lifecycle failure");
        };
        assert_eq!(reason_code, "relay.start.address_in_use");
        assert_eq!(
            relay_changed_events(&mut relay_events, network_name),
            vec![
                NodeEmbeddedServiceStatus::Starting,
                NodeEmbeddedServiceStatus::Failed,
            ]
        );

        let service = harness
            .state
            .snapshot()
            .await
            .embedded_services
            .into_iter()
            .find(|service| service.network_name == network_name)
            .expect("failed embedded service");
        assert_eq!(service.relay_status, NodeEmbeddedServiceStatus::Failed);
        assert!(service.relay_addr.is_none());
        let health = service.relay_health.expect("failed Relay health");
        assert_eq!(health.lifecycle, RelayLifecycleState::Failed);
        assert_eq!(health.reason_code.as_deref(), Some(reason_code));
        assert!(!health.listener_bound);
        assert!(service.relay_authorization.is_none());
        assert!(service.relay_governance.is_none());
        let manager_service = harness
            .services
            .list()
            .await
            .expect("list embedded services")
            .into_iter()
            .find(|service| service.network_name == network_name)
            .expect("manager keeps the failed service");
        assert!(manager_service.relay_addr.is_none());
        assert_eq!(
            manager_service
                .relay_health
                .and_then(|health| health.reason_code),
            Some(reason_code.to_string())
        );
        assert!(
            !harness
                .configs
                .list_networks()
                .expect("load network metadata")
                .into_iter()
                .find(|network| network.name == network_name)
                .expect("persisted network")
                .relay_enabled
        );

        drop(occupied);
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                match harness
                    .runtime
                    .set_network_role(role_command(network_name, NodeNetworkRole::Relay, true))
                    .await
                {
                    Ok(_) => break,
                    Err(RuntimeControlError::RelayLifecycleFailed {
                        reason_code: "relay.start.address_in_use",
                        ..
                    }) => {
                        tokio::time::sleep(Duration::from_millis(10)).await;
                    }
                    Err(error) => panic!("unexpected Relay retry failure: {error}"),
                }
            }
        })
        .await
        .expect("Relay retry timed out");
        let recovered = harness
            .state
            .snapshot()
            .await
            .embedded_services
            .into_iter()
            .find(|service| service.network_name == network_name)
            .expect("recovered embedded service");
        assert_eq!(recovered.relay_status, NodeEmbeddedServiceStatus::Running);
        assert_eq!(
            recovered.relay_addr.map(|addr| addr.port()),
            Some(relay_port)
        );
        assert_eq!(
            recovered
                .relay_health
                .as_ref()
                .map(|health| health.lifecycle),
            Some(RelayLifecycleState::Running)
        );
        assert!(
            harness
                .configs
                .list_networks()
                .expect("load recovered network metadata")
                .into_iter()
                .find(|network| network.name == network_name)
                .expect("persisted recovered network")
                .relay_enabled
        );

        harness.shutdown().await;
        tokio::net::UdpSocket::bind(("127.0.0.1", relay_port))
            .await
            .expect("runtime shutdown must release the Relay port");
    }

    #[tokio::test]
    async fn initial_relay_restore_failure_keeps_signaling_and_retries_on_next_start() {
        let harness = TestHarness::new("relay-restore-failure").await;
        let network_name = "relay-restore-failure";
        harness
            .configs
            .create_network(network_name, "ws://127.0.0.1:6666", true)
            .expect("create persisted network");
        let occupied = tokio::net::UdpSocket::bind(("127.0.0.1", 0))
            .await
            .expect("occupy Relay UDP port");
        let relay_port = occupied.local_addr().expect("occupied address").port();
        harness
            .configs
            .set_embedded_service_metadata(
                network_name,
                0,
                Some(relay_port),
                true,
                Some("127.0.0.1".into()),
                None,
            )
            .expect("persist embedded service metadata");

        let first = harness
            .runtime
            .start_node()
            .await
            .expect("Relay failure must be reported without aborting node startup");
        assert_eq!(first.failed.len(), 1);
        assert!(first.failed[0]
            .message
            .contains("relay.start.address_in_use"));

        let failed_service = harness
            .state
            .snapshot()
            .await
            .embedded_services
            .into_iter()
            .find(|service| service.network_name == network_name)
            .expect("signaling must remain observable after Relay restore failure");
        assert_eq!(failed_service.status, NodeEmbeddedServiceStatus::Running);
        assert_eq!(
            failed_service.relay_status,
            NodeEmbeddedServiceStatus::Failed
        );
        let signaling_addr = failed_service
            .signaling_addr
            .expect("signaling address after Relay restore failure");
        assert!(failed_service.relay_addr.is_none());
        let health = failed_service.relay_health.expect("failed Relay health");
        assert_eq!(health.lifecycle, RelayLifecycleState::Failed);
        assert_eq!(
            health.reason_code.as_deref(),
            Some("relay.start.address_in_use")
        );
        tokio::net::TcpListener::bind(("127.0.0.1", signaling_addr.port()))
            .await
            .expect_err("signaling listener must survive Relay restore failure");

        drop(occupied);
        let retried = tokio::time::timeout(Duration::from_secs(2), harness.runtime.start_node())
            .await
            .expect("Retry after Relay port release timed out")
            .expect("retry node startup");
        assert_eq!(retried.started, vec![network_name]);
        let recovered = harness
            .state
            .snapshot()
            .await
            .embedded_services
            .into_iter()
            .find(|service| service.network_name == network_name)
            .expect("recovered embedded service");
        assert_eq!(recovered.relay_status, NodeEmbeddedServiceStatus::Running);
        assert_eq!(
            recovered.relay_addr.map(|addr| addr.port()),
            Some(relay_port)
        );
        assert_eq!(
            recovered
                .relay_health
                .as_ref()
                .map(|health| health.lifecycle),
            Some(RelayLifecycleState::Running)
        );

        harness.shutdown().await;
        tokio::net::UdpSocket::bind(("127.0.0.1", relay_port))
            .await
            .expect("shutdown must release the recovered Relay port");
    }

    #[tokio::test]
    async fn create_cleans_up_signaling_when_initial_relay_start_fails() {
        let harness = TestHarness::new("relay-create-failure").await;
        let occupied = tokio::net::UdpSocket::bind(("127.0.0.1", 0))
            .await
            .expect("occupy Relay UDP port");
        let relay_port = occupied.local_addr().expect("occupied address").port();
        let mut command = create_command("relay-create-failure", false);
        command.relay_port = relay_port;
        command.external_host = Some("127.0.0.1".into());

        let error = harness
            .runtime
            .create(command)
            .await
            .expect_err("occupied Relay port must fail network creation");
        let RuntimeControlError::RelayLifecycleFailed { reason_code, .. } = error else {
            panic!("expected typed Relay lifecycle failure");
        };
        assert_eq!(reason_code, "relay.start.address_in_use");
        assert!(
            harness
                .services
                .list()
                .await
                .expect("list embedded services")
                .is_empty(),
            "failed create must not leave an orphan signaling service"
        );
        assert!(
            harness.state.snapshot().await.embedded_services.is_empty(),
            "rolled-back create must not retain embedded service state"
        );
        assert!(!harness.configs.network_exists("relay-create-failure"));

        drop(occupied);
        tokio::net::UdpSocket::bind(("127.0.0.1", relay_port))
            .await
            .expect("failed create must release its signaling-only resources");
        harness.shutdown().await;
    }

    #[tokio::test]
    async fn disabled_relay_intent_restores_on_network_restart_and_reuses_port() {
        let harness = TestHarness::new("relay-role-restart").await;
        let network_name = "relay-restart";
        start_fake_network(&harness, network_name).await;
        let started = harness
            .services
            .start(
                EmbeddedServiceStartRequest::new(network_name, 0, 0)
                    .with_external_host("127.0.0.1"),
            )
            .await
            .expect("start embedded service");
        let signaling_port = started.signaling_addr.port();
        let relay_port = started.relay_addr.expect("initial relay").port();
        harness
            .configs
            .set_embedded_service_metadata(
                network_name,
                signaling_port,
                Some(relay_port),
                true,
                Some("127.0.0.1".into()),
                None,
            )
            .expect("persist embedded service metadata");
        record_running_embedded_service_for_test(&harness, &started).await;

        harness
            .runtime
            .set_network_role(role_command(network_name, NodeNetworkRole::Relay, false))
            .await
            .expect("disable relay before restart");
        harness
            .runtime
            .stop(vec![network_name.into()], false)
            .await
            .expect("stop network and embedded service");

        harness
            .runtime
            .start_node()
            .await
            .expect("restart node services from owner intent");
        let restored = harness.state.snapshot().await;
        let service = restored
            .embedded_services
            .iter()
            .find(|service| service.network_name == network_name)
            .expect("restored embedded service");
        assert_eq!(service.status, NodeEmbeddedServiceStatus::Running);
        assert_eq!(service.relay_status, NodeEmbeddedServiceStatus::Stopped);
        assert_eq!(
            service.signaling_addr.map(|addr| addr.port()),
            Some(signaling_port)
        );
        assert!(service.relay_addr.is_none());

        harness
            .runtime
            .set_network_role(role_command(network_name, NodeNetworkRole::Relay, true))
            .await
            .expect("re-enable relay after restart");
        let service = harness
            .state
            .snapshot()
            .await
            .embedded_services
            .into_iter()
            .find(|service| service.network_name == network_name)
            .expect("embedded service after re-enable");
        assert_eq!(service.relay_status, NodeEmbeddedServiceStatus::Running);
        assert_eq!(
            service.relay_addr.map(|addr| addr.port()),
            Some(relay_port),
            "re-enable must restore the last persisted Relay port"
        );

        harness.shutdown().await;
    }

    #[tokio::test]
    async fn runtime_routing_failure_leaves_file_snapshot_and_peers_unchanged() {
        let harness = TestHarness::new("routing-failure").await;
        start_fake_network(&harness, "routing-failure").await;
        harness
            .state
            .update(|snapshot| {
                let network = snapshot
                    .networks
                    .iter_mut()
                    .find(|network| network.name == "routing-failure")
                    .expect("running network");
                network.peer_count = 1;
                snapshot.peers.push(peer_snapshot("routing-failure"));
            })
            .await;
        let before = harness.state.snapshot().await;
        let before_config = before
            .configuration
            .networks
            .iter()
            .find(|network| network.network_name == "routing-failure")
            .expect("initial shared routing config")
            .clone();
        let config_path = harness
            .configs
            .get_network_config("routing-failure")
            .expect("network config path");
        let before_file = ClientConfig::from_file(&config_path).expect("initial config");
        harness.fake.fail_routing("routing-failure");
        let mut events = harness.events.subscribe();

        let error = harness
            .runtime
            .update_routing(routing_command(
                "routing-failure",
                NodeRoutingMode::BypassMainland,
                Some("office"),
                true,
            ))
            .await
            .expect_err("runtime rejection must propagate");
        assert!(matches!(
            error,
            RuntimeControlError::Persistence(ref message)
                if message.contains("旧配置回滚未能确认")
        ));

        let after = harness.state.snapshot().await;
        assert_eq!(
            after
                .configuration
                .networks
                .iter()
                .find(|network| network.network_name == "routing-failure")
                .expect("unchanged shared routing config"),
            &before_config
        );
        let after_file = ClientConfig::from_file(&config_path).expect("unchanged config");
        assert_eq!(after_file.routing.mode, before_file.routing.mode);
        assert_eq!(after_file.routing.exit_node, before_file.routing.exit_node);
        assert_eq!(after_file.exit_node.enabled, before_file.exit_node.enabled);
        assert!(routing_changed_events(&mut events).is_empty());
        assert_eq!(after.peers, before.peers);
        assert_eq!(harness.fake.routing_calls().len(), 2);
        assert_eq!(harness.fake.launches(), vec!["routing-failure"]);
        assert_eq!(harness.fake.shutdown_count(), 0);

        harness.shutdown().await;
    }

    #[tokio::test]
    async fn concurrent_routing_updates_are_serialized_and_last_write_wins() {
        let harness = TestHarness::new("routing-concurrent").await;
        start_fake_network(&harness, "routing-concurrent").await;
        harness.fake.set_routing_delay(Duration::from_millis(80));

        let first_runtime = harness.runtime.clone();
        let first = tokio::spawn(async move {
            first_runtime
                .update_routing(routing_command(
                    "routing-concurrent",
                    NodeRoutingMode::BypassMainland,
                    Some("first"),
                    false,
                ))
                .await
        });
        wait_for_routing_calls(&harness.fake, 1).await;

        let second = harness
            .runtime
            .update_routing(routing_command(
                "routing-concurrent",
                NodeRoutingMode::GlobalProxy,
                Some("second"),
                true,
            ))
            .await
            .expect("second routing update");
        assert_eq!(second.mode, "GlobalProxy");
        first
            .await
            .expect("first routing task")
            .expect("first routing update");

        assert_eq!(harness.fake.max_routing_concurrency(), 1);
        let calls = harness.fake.routing_calls();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].mode, NodeRoutingMode::BypassMainland);
        assert_eq!(calls[1].mode, NodeRoutingMode::GlobalProxy);
        let snapshot = harness.state.snapshot().await;
        let configuration = snapshot
            .configuration
            .networks
            .iter()
            .find(|network| network.network_name == "routing-concurrent")
            .expect("final shared routing config");
        assert_eq!(configuration.routing_mode, NodeRoutingMode::GlobalProxy);
        assert_eq!(configuration.exit_node.as_deref(), Some("second"));
        assert!(configuration.exit_node_enabled);
        assert_eq!(harness.fake.launches(), vec!["routing-concurrent"]);

        harness.shutdown().await;
    }

    #[tokio::test]
    async fn timed_out_update_queues_old_config_rollback_after_late_application() {
        let harness = TestHarness::new_with_routing_timeout(
            "routing-timeout-rollback",
            Duration::from_millis(80),
        )
        .await;
        start_fake_network(&harness, "routing-timeout").await;
        harness
            .state
            .update(|snapshot| {
                let network = snapshot
                    .networks
                    .iter_mut()
                    .find(|network| network.name == "routing-timeout")
                    .expect("running network");
                network.peer_count = 1;
                snapshot.peers.push(peer_snapshot("routing-timeout"));
            })
            .await;
        let before = harness.state.snapshot().await;
        let before_config = before
            .configuration
            .networks
            .iter()
            .find(|network| network.network_name == "routing-timeout")
            .expect("initial shared routing config")
            .clone();
        let config_path = harness
            .configs
            .get_network_config("routing-timeout")
            .expect("network config path");
        harness.fake.set_routing_delay(Duration::from_millis(120));
        let mut events = harness.events.subscribe();

        let update_runtime = harness.runtime.clone();
        let update = tokio::spawn(async move {
            update_runtime
                .update_routing(routing_command(
                    "routing-timeout",
                    NodeRoutingMode::GlobalProxy,
                    Some("office"),
                    true,
                ))
                .await
        });
        wait_for_routing_calls(&harness.fake, 1).await;
        // Only the first application is slow. The queued compensation command
        // must quickly restore the old config after that late application ends.
        harness.fake.set_routing_delay(Duration::ZERO);
        let error = update
            .await
            .expect("timed out routing task")
            .expect_err("timed out update must not report success");
        assert!(matches!(
            error,
            RuntimeControlError::Persistence(ref message)
                if message.contains("超时") && message.contains("已回滚")
        ));

        let calls = harness.fake.routing_calls();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].mode, NodeRoutingMode::GlobalProxy);
        assert_eq!(calls[1].mode, before_config.routing_mode);
        assert_eq!(calls[1].exit_node, before_config.exit_node);
        assert_eq!(calls[1].is_exit_node, before_config.exit_node_enabled);
        assert_eq!(harness.fake.max_routing_concurrency(), 1);

        let persisted = ClientConfig::from_file(config_path).expect("rolled back config");
        assert_eq!(
            persisted.routing.mode,
            fuckipv6_common::RoutingMode::MeshOnly
        );
        assert_eq!(persisted.routing.exit_node, before_config.exit_node);
        assert_eq!(persisted.exit_node.enabled, before_config.exit_node_enabled);
        let after = harness.state.snapshot().await;
        assert_eq!(
            after
                .configuration
                .networks
                .iter()
                .find(|network| network.network_name == "routing-timeout")
                .expect("restored shared routing config"),
            &before_config
        );
        assert_eq!(after.peers, before.peers);
        assert!(routing_changed_events(&mut events).is_empty());
        assert_eq!(harness.fake.launches(), vec!["routing-timeout"]);
        assert_eq!(harness.fake.shutdown_count(), 0);

        harness.shutdown().await;
    }

    #[tokio::test]
    async fn snapshot_publish_failure_rolls_back_runtime_file_and_shared_state() {
        let harness = TestHarness::new("routing-snapshot-rollback").await;
        start_fake_network(&harness, "routing-snapshot").await;
        harness
            .state
            .update(|snapshot| {
                let network = snapshot
                    .networks
                    .iter_mut()
                    .find(|network| network.name == "routing-snapshot")
                    .expect("running network");
                network.peer_count = 1;
                snapshot.peers.push(peer_snapshot("routing-snapshot"));
            })
            .await;
        let before = harness.state.snapshot().await;
        let before_config = before
            .configuration
            .networks
            .iter()
            .find(|network| network.network_name == "routing-snapshot")
            .expect("initial shared routing config")
            .clone();
        harness.fake.set_routing_delay(Duration::from_millis(80));
        let mut events = harness.events.subscribe();
        let routing_runtime = harness.runtime.clone();
        let update = tokio::spawn(async move {
            routing_runtime
                .update_routing(routing_command(
                    "routing-snapshot",
                    NodeRoutingMode::GlobalProxy,
                    Some("office"),
                    true,
                ))
                .await
        });
        wait_for_routing_calls(&harness.fake, 1).await;

        harness
            .state
            .update(|snapshot| {
                snapshot
                    .configuration
                    .networks
                    .retain(|network| network.network_name != "routing-snapshot");
            })
            .await;
        let error = update
            .await
            .expect("routing task")
            .expect_err("snapshot publish failure must reject the update");
        assert!(matches!(
            error,
            RuntimeControlError::Persistence(ref message)
                if message.contains("共享快照发布失败")
        ));

        let calls = harness.fake.routing_calls();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].mode, NodeRoutingMode::GlobalProxy);
        assert_eq!(calls[1].mode, before_config.routing_mode);
        assert_eq!(calls[1].exit_node, before_config.exit_node);
        assert_eq!(calls[1].is_exit_node, before_config.exit_node_enabled);
        let config_path = harness
            .configs
            .get_network_config("routing-snapshot")
            .expect("network config path");
        let persisted = ClientConfig::from_file(config_path).expect("rolled back config");
        assert_eq!(
            persisted.routing.mode,
            fuckipv6_common::RoutingMode::MeshOnly
        );
        assert_eq!(persisted.routing.exit_node, before_config.exit_node);
        assert_eq!(persisted.exit_node.enabled, before_config.exit_node_enabled);
        let after = harness.state.snapshot().await;
        assert_eq!(
            after
                .configuration
                .networks
                .iter()
                .find(|network| network.network_name == "routing-snapshot")
                .expect("restored shared routing config"),
            &before_config
        );
        assert_eq!(after.peers, before.peers);
        assert!(routing_changed_events(&mut events).is_empty());
        assert_eq!(harness.fake.launches(), vec!["routing-snapshot"]);
        assert_eq!(harness.fake.shutdown_count(), 0);

        harness.shutdown().await;
    }

    #[tokio::test]
    async fn invalid_routing_policy_publishes_invalid_without_touching_runtime() {
        let harness = TestHarness::new("routing-invalid").await;
        start_fake_network(&harness, "routing-invalid").await;

        let before = harness
            .state
            .snapshot()
            .await
            .routing_summaries
            .into_iter()
            .find(|summary| summary.network_name == "routing-invalid")
            .expect("current summary after startup");
        assert_eq!(before.status, NodeRoutingSummaryStatus::Current);
        assert_eq!(before.last_applied_revision, Some(1));
        let calls_before = harness.fake.routing_calls().len();

        let error = harness
            .runtime
            .update_routing(routing_command(
                "routing-invalid",
                NodeRoutingMode::GlobalProxy,
                Some("  "),
                false,
            ))
            .await
            .expect_err("blank exit must reject");
        assert!(matches!(error, RuntimeControlError::Invalid(_)));
        assert_eq!(harness.fake.routing_calls().len(), calls_before);

        let after = harness
            .state
            .snapshot()
            .await
            .routing_summaries
            .into_iter()
            .find(|summary| summary.network_name == "routing-invalid")
            .expect("invalid summary");
        assert_eq!(after.status, NodeRoutingSummaryStatus::Invalid);
        assert_eq!(after.last_applied_revision, before.last_applied_revision);
        assert_eq!(
            after.last_applied_at_unix_ms,
            before.last_applied_at_unix_ms
        );
        assert_eq!(
            after
                .problem
                .as_ref()
                .map(|problem| problem.message_key.as_str()),
            Some("runtime.routing.problem.invalid")
        );

        harness.shutdown().await;
    }

    #[tokio::test]
    async fn routing_update_for_a_non_running_network_does_not_touch_the_runtime() {
        let harness = TestHarness::new("routing-missing").await;

        let error = harness
            .runtime
            .update_routing(routing_command(
                "missing",
                NodeRoutingMode::GlobalProxy,
                None,
                false,
            ))
            .await
            .expect_err("non-running network must be rejected");
        assert!(matches!(error, RuntimeControlError::NotFound(_)));
        assert!(harness.fake.routing_calls().is_empty());
        assert!(harness.state.snapshot().await.routing_summaries.is_empty());

        harness.shutdown().await;
    }

    #[tokio::test]
    async fn start_plan_reports_partial_failure_and_retries_only_failed_networks() {
        let harness = TestHarness::new("start-plan").await;
        harness
            .configs
            .create_network("home", "ws://home.example:6666", true)
            .expect("create home");
        harness
            .configs
            .create_network("office", "ws://office.example:6666", false)
            .expect("create office");
        harness.fake.fail("office");
        let plan = NodeRuntimePlan::load(
            &harness.configs,
            NodeNetworkSelection::Named(vec!["home".into(), "office".into()]),
        )
        .expect("load plan");

        let first = harness
            .runtime
            .start_plan(plan.clone())
            .await
            .expect("start first plan");
        assert_eq!(first.requested, 2);
        assert_eq!(first.started, vec!["home"]);
        assert_eq!(first.failed.len(), 1);
        assert_eq!(first.failed[0].network_name, "office");

        let snapshot = harness.state.snapshot().await;
        assert_eq!(snapshot.lifecycle.active_networks, 1);
        assert_eq!(snapshot.lifecycle.failed_networks, vec!["office"]);
        assert!(snapshot.networks.iter().any(|network| {
            network.name == "home" && network.status == NodeNetworkStatus::Online
        }));
        assert!(snapshot.networks.iter().any(|network| {
            network.name == "office" && network.status == NodeNetworkStatus::Failed
        }));

        harness.fake.recover("office");
        let second = harness
            .runtime
            .start_plan(plan)
            .await
            .expect("retry failed network");
        assert_eq!(second.started, vec!["office"]);
        assert_eq!(second.already_running, vec!["home"]);
        assert!(second.failed.is_empty());
        assert_eq!(harness.fake.launches(), vec!["home", "office"]);

        let snapshot = harness.state.snapshot().await;
        assert_eq!(snapshot.lifecycle.active_networks, 2);
        assert!(snapshot.lifecycle.failed_networks.is_empty());
        assert_eq!(harness.lifecycle.snapshot().await.active_networks, 2);

        harness.shutdown().await;
    }

    #[tokio::test]
    async fn start_node_without_networks_reaches_running() {
        let harness = TestHarness::new("start-node-empty").await;

        let summary = harness
            .runtime
            .start_node()
            .await
            .expect("start empty node");
        assert_eq!(summary.requested, 0);
        assert!(summary.started.is_empty());
        assert!(summary.failed.is_empty());

        let lifecycle = harness.lifecycle.snapshot().await;
        assert_eq!(lifecycle.phase, NodePhase::Running);
        assert_eq!(lifecycle.configured_networks, 0);
        assert_eq!(lifecycle.active_networks, 0);
        assert!(lifecycle.failed_networks.is_empty());

        harness.shutdown().await;
    }

    #[tokio::test]
    async fn start_node_only_restores_the_default_network() {
        let harness = TestHarness::new("start-node-default").await;
        harness
            .configs
            .create_network("home", "ws://home.example:6666", true)
            .expect("create home");
        harness
            .configs
            .create_network("office", "ws://office.example:6666", false)
            .expect("create office");
        harness.fake.fail("office");

        let summary = harness.runtime.start_node().await.expect("start node");
        assert_eq!(summary.requested, 1);
        assert_eq!(summary.started, vec!["home"]);
        assert!(summary.failed.is_empty());
        assert!(harness.fake.launches().iter().all(|name| name != "office"));

        harness.shutdown().await;
    }

    #[tokio::test]
    async fn start_plan_with_partial_failure_stays_running_and_reports_failed_network() {
        let harness = TestHarness::new("start-node-partial").await;
        harness
            .configs
            .create_network("home", "ws://home.example:6666", true)
            .expect("create home");
        harness
            .configs
            .create_network("office", "ws://office.example:6666", false)
            .expect("create office");
        harness.fake.fail("office");
        let plan = NodeRuntimePlan::load(
            &harness.configs,
            NodeNetworkSelection::Named(vec!["home".into(), "office".into()]),
        )
        .expect("load plan");

        let summary = harness.runtime.start_plan(plan).await.expect("start plan");
        assert_eq!(summary.requested, 2);
        assert_eq!(summary.started, vec!["home"]);
        assert_eq!(summary.failed.len(), 1);
        assert_eq!(summary.failed[0].network_name, "office");

        let lifecycle = harness.lifecycle.snapshot().await;
        assert_eq!(lifecycle.phase, NodePhase::Running);
        assert_eq!(lifecycle.active_networks, 1);
        assert_eq!(lifecycle.failed_networks, vec!["office"]);

        harness.shutdown().await;
    }

    #[tokio::test]
    async fn start_node_with_only_failures_reaches_failed_with_network_context() {
        let harness = TestHarness::new("start-node-failed").await;
        harness
            .configs
            .create_network("broken", "ws://broken.example:6666", true)
            .expect("create broken");
        harness.fake.fail("broken");

        let summary = harness.runtime.start_node().await.expect("start node");
        assert_eq!(summary.requested, 1);
        assert!(summary.started.is_empty());
        assert_eq!(summary.failed.len(), 1);
        assert_eq!(summary.failed[0].network_name, "broken");

        let lifecycle = harness.lifecycle.snapshot().await;
        assert_eq!(lifecycle.phase, NodePhase::Failed);
        assert_eq!(lifecycle.configured_networks, 1);
        assert_eq!(lifecycle.active_networks, 0);
        assert_eq!(lifecycle.failed_networks, vec!["broken"]);

        harness.shutdown().await;
    }

    #[tokio::test]
    async fn create_exposes_running_embedded_service_without_secrets_or_paths() {
        let harness = TestHarness::new("service-running").await;
        let mut events = harness.events.subscribe();

        let created = harness
            .runtime
            .create(create_command("local", false))
            .await
            .expect("create detached network");
        assert!(created.ticket.starts_with(NetworkTicket::URI_SCHEME));
        assert_eq!(
            embedded_service_events(&mut events, "local"),
            vec![
                NodeEmbeddedServiceStatus::Starting,
                NodeEmbeddedServiceStatus::Running,
            ]
        );

        let snapshot = harness.state.snapshot().await;
        let service = snapshot
            .embedded_services
            .iter()
            .find(|service| service.network_name == "local")
            .expect("embedded service snapshot");
        assert_eq!(service.status, NodeEmbeddedServiceStatus::Running);
        let signaling_addr = service.signaling_addr.expect("signaling address");
        let relay_addr = service.relay_addr.expect("relay address");
        let persisted = harness
            .configs
            .list_networks()
            .expect("load persisted networks")
            .into_iter()
            .find(|network| network.name == "local")
            .expect("persisted network metadata");
        assert!(persisted.owns_embedded_services);
        assert_eq!(persisted.signaling_port, Some(signaling_addr.port()));
        assert_eq!(persisted.relay_port, Some(relay_addr.port()));
        assert!(persisted.relay_enabled);
        assert_eq!(persisted.external_host, None);
        assert_eq!(persisted.ip_pool, None);
        assert!(snapshot
            .configuration
            .networks
            .iter()
            .any(|network| network.network_name == "local"));
        assert!(snapshot.invariant_violations().is_empty());

        let encoded = serde_json::to_string(&snapshot).expect("serialize node snapshot");
        assert!(!encoded.contains("ticket"));
        assert!(!encoded.contains("private_key"));
        assert!(!encoded.contains("secret_key"));
        assert!(!encoded.contains("config_path"));

        harness.shutdown().await;
    }

    #[tokio::test]
    async fn stop_then_start_node_restores_owned_embedded_service_endpoints() {
        let harness = TestHarness::new("node-lifecycle-owned").await;
        harness
            .runtime
            .create(create_command("owned", false))
            .await
            .expect("create detached network");

        let before = harness.services.list().await.expect("list services");
        assert_eq!(before.len(), 1);
        let persisted_before = harness
            .configs
            .list_networks()
            .expect("load networks before stop")
            .into_iter()
            .find(|network| network.name == "owned")
            .expect("owned network metadata");
        assert_eq!(
            persisted_before.signaling_port,
            Some(before[0].signaling_addr.port())
        );
        assert_eq!(
            persisted_before.relay_port,
            before[0].relay_addr.map(|addr| addr.port())
        );

        let stopped = harness
            .runtime
            .stop_node()
            .await
            .expect("stop node data plane");
        assert_eq!(stopped.stopped, vec!["owned"]);
        assert!(stopped.failed.is_empty());
        assert!(stopped.is_success());
        assert!(harness
            .services
            .list()
            .await
            .expect("list stopped services")
            .is_empty());

        let started = harness.runtime.start_node().await.expect("start node");
        assert_eq!(started.started, vec!["owned"]);
        let after = harness
            .services
            .list()
            .await
            .expect("list restored services");
        assert_eq!(after.len(), 1);
        assert_eq!(after[0].signaling_addr, before[0].signaling_addr);
        assert_eq!(after[0].relay_addr, before[0].relay_addr);
        assert_eq!(after[0].ticket, before[0].ticket);

        let persisted_after = harness
            .configs
            .list_networks()
            .expect("load networks after restart")
            .into_iter()
            .find(|network| network.name == "owned")
            .expect("restored network metadata");
        assert_eq!(
            persisted_after.signaling_port,
            Some(after[0].signaling_addr.port())
        );
        assert_eq!(
            persisted_after.relay_port,
            after[0].relay_addr.map(|addr| addr.port())
        );

        harness.shutdown().await;
    }

    #[tokio::test]
    async fn stop_then_start_selection_restores_owned_embedded_service_endpoints() {
        let harness = TestHarness::new("selection-lifecycle-owned").await;
        harness
            .runtime
            .create(create_command("selection-owned", false))
            .await
            .expect("create detached network");

        let before = harness.services.list().await.expect("list services");
        assert_eq!(before.len(), 1);

        harness
            .runtime
            .stop(vec!["selection-owned".into()], false)
            .await
            .expect("stop network and owned service");
        assert!(harness
            .services
            .list()
            .await
            .expect("list stopped services")
            .is_empty());

        let started = harness
            .runtime
            .start_selection(NodeNetworkSelection::Named(vec!["selection-owned".into()]))
            .await
            .expect("start selected network");
        assert_eq!(started.started, vec!["selection-owned"]);
        assert!(started.failed.is_empty());

        let after = harness
            .services
            .list()
            .await
            .expect("list restored services");
        assert_eq!(after.len(), 1);
        assert_eq!(after[0].signaling_addr, before[0].signaling_addr);
        assert_eq!(after[0].relay_addr, before[0].relay_addr);
        assert_eq!(after[0].ticket, before[0].ticket);
        assert_eq!(
            harness.state.snapshot().await.networks[0].status,
            NodeNetworkStatus::Online
        );

        harness.shutdown().await;
    }

    #[tokio::test]
    async fn embedded_service_start_failure_is_visible_in_snapshot_and_events() {
        let harness = TestHarness::new("service-failure").await;
        let mut events = harness.events.subscribe();
        let mut command = create_command("broken", false);
        command.signaling_port = 12345;
        command.relay_port = 12345;

        let error = harness
            .runtime
            .create(command)
            .await
            .expect_err("invalid embedded service request must fail");
        assert!(matches!(error, RuntimeControlError::Invalid(_)));
        assert_eq!(
            embedded_service_events(&mut events, "broken"),
            vec![
                NodeEmbeddedServiceStatus::Starting,
                NodeEmbeddedServiceStatus::Failed,
            ]
        );

        let snapshot = harness.state.snapshot().await;
        let service = snapshot
            .embedded_services
            .iter()
            .find(|service| service.network_name == "broken")
            .expect("failed embedded service snapshot");
        assert_eq!(service.status, NodeEmbeddedServiceStatus::Failed);
        assert!(service.signaling_addr.is_none());
        assert!(service.relay_addr.is_none());
        assert!(!harness.configs.network_exists("broken"));

        harness.shutdown().await;
    }

    #[tokio::test]
    async fn stop_network_releases_runtime_and_owned_service() {
        let harness = TestHarness::new("stop-owned").await;
        let created = harness
            .runtime
            .create(create_command("owned", true))
            .await
            .expect("create connected network");
        assert!(created.config_saved);
        assert!(created.started);
        assert_eq!(
            harness.services.list().await.expect("list services").len(),
            1
        );
        harness
            .state
            .update(|snapshot| {
                let network = snapshot
                    .networks
                    .iter_mut()
                    .find(|network| network.name == "owned")
                    .expect("running network state");
                network.peer_count = 1;
                snapshot.interfaces.push(NodeInterfaceSnapshot {
                    index: u32::MAX,
                    name: network.tun_name.clone(),
                    kind: NodeInterfaceKind::Tun,
                    ownership: NodeInterfaceOwnership::Product,
                    status: NodeInterfaceStatus::Up,
                    mtu: 1_420,
                    addresses: Vec::new(),
                    default_ipv4: false,
                    default_ipv6: false,
                });
                snapshot.peers.push(peer_snapshot("owned"));
            })
            .await;
        assert!(harness
            .state
            .snapshot()
            .await
            .invariant_violations()
            .is_empty());
        let mut events = harness.events.subscribe();

        let stopped = harness
            .runtime
            .stop(vec!["owned".into()], false)
            .await
            .expect("stop network");
        assert_eq!(stopped.stopped, vec!["owned"]);
        assert_eq!(harness.fake.shutdown_count(), 1);
        assert!(harness
            .services
            .list()
            .await
            .expect("list services")
            .is_empty());
        assert!(harness.configs.network_exists("owned"));

        let snapshot = harness.state.snapshot().await;
        assert!(snapshot.networks.iter().any(|network| {
            network.name == "owned"
                && network.status == NodeNetworkStatus::Offline
                && network.peer_count == 0
        }));
        assert!(snapshot.peers.is_empty());
        harness
            .state
            .refresh_interface_capture(Default::default(), Some(&BTreeMap::new()))
            .await;
        let snapshot = harness.state.snapshot().await;
        assert!(snapshot.invariant_violations().is_empty());
        let service = snapshot
            .embedded_services
            .iter()
            .find(|service| service.network_name == "owned")
            .expect("stopped embedded service snapshot");
        assert_eq!(service.status, NodeEmbeddedServiceStatus::Stopped);
        assert!(service.signaling_addr.is_none());
        assert!(service.relay_addr.is_none());
        assert_eq!(
            embedded_service_events(&mut events, "owned"),
            vec![
                NodeEmbeddedServiceStatus::Stopping,
                NodeEmbeddedServiceStatus::Stopped,
            ]
        );
        assert_eq!(snapshot.lifecycle.active_networks, 0);
        assert_eq!(snapshot.lifecycle.configured_networks, 1);

        harness.shutdown().await;
    }

    #[tokio::test]
    async fn create_without_auto_connect_keeps_service_until_network_stop() {
        let harness = TestHarness::new("create-detached").await;
        let created = harness
            .runtime
            .create(create_command("detached", false))
            .await
            .expect("create detached network");
        assert!(created.config_saved);
        assert!(!created.started);
        assert!(created.ticket.starts_with(NetworkTicket::URI_SCHEME));
        assert_eq!(
            harness.services.list().await.expect("list services").len(),
            1
        );
        let snapshot = harness.state.snapshot().await;
        let network = snapshot
            .networks
            .iter()
            .find(|network| network.name == "detached")
            .expect("detached network is immediately listed");
        assert_eq!(network.status, NodeNetworkStatus::Offline);
        assert_eq!(network.peer_count, 0);
        assert!(snapshot
            .configuration
            .networks
            .iter()
            .any(|network| network.network_name == "detached"));
        let membership = snapshot
            .memberships
            .iter()
            .find(|membership| membership.network_name == "detached")
            .expect("owner membership is projected immediately");
        assert_eq!(
            membership.state,
            fuckipv6_common::domain::MembershipState::Accepted
        );
        assert_eq!(membership.role, MembershipRole::Owner);
        assert!(snapshot.invariant_violations().is_empty());

        harness
            .runtime
            .stop(vec!["detached".into()], false)
            .await
            .expect("stop detached service");
        assert!(harness
            .services
            .list()
            .await
            .expect("list services")
            .is_empty());
        assert!(harness.configs.network_exists("detached"));

        harness.shutdown().await;
    }

    #[tokio::test]
    async fn runtime_launch_receives_authoritative_network_id() {
        let harness = TestHarness::new("launch-authoritative-network-id").await;
        harness
            .configs
            .create_network("identified", "ws://127.0.0.1:6666", true)
            .expect("create identified config");
        let network_id = NetworkId::from_public_key(b"runtime-authoritative-network");
        harness
            .configs
            .set_network_id_metadata("identified", network_id.as_str())
            .expect("persist authoritative network id");
        let plan = NodeRuntimePlan::load(
            &harness.configs,
            NodeNetworkSelection::Named(vec!["identified".into()]),
        )
        .expect("load identified plan");

        let summary = harness
            .runtime
            .start_plan(plan)
            .await
            .expect("start identified network");

        assert_eq!(summary.started, vec!["identified"]);
        assert_eq!(
            harness.fake.launched_network_ids(),
            vec![Some(network_id.to_string())]
        );
        harness.shutdown().await;
    }

    #[tokio::test]
    async fn duplicate_network_name_is_a_structured_conflict_and_creates_nothing() {
        let harness = TestHarness::new("create-duplicate-name").await;
        harness
            .runtime
            .create(create_command("duplicate", false))
            .await
            .expect("create first network");
        let created_network_id = harness
            .configs
            .load_network_record("duplicate")
            .expect("load created network")
            .network_id;

        let error = harness
            .runtime
            .create(create_command("duplicate", true))
            .await
            .expect_err("duplicate name must fail");
        assert!(matches!(
            error,
            RuntimeControlError::CreationConflict {
                reason: "duplicate_name",
                ..
            }
        ));
        assert_eq!(
            harness
                .configs
                .load_network_record("duplicate")
                .expect("load original network")
                .network_id,
            created_network_id
        );
        let listed = harness
            .configs
            .list_networks()
            .expect("list networks")
            .into_iter()
            .filter(|network| network.name == "duplicate")
            .collect::<Vec<_>>();
        assert_eq!(listed.len(), 1);
        assert_eq!(
            listed[0].network_id.as_deref(),
            Some(created_network_id.as_str())
        );
        assert_eq!(
            harness.services.list().await.expect("list services").len(),
            1
        );

        let snapshot = harness.state.snapshot().await;
        assert_eq!(
            snapshot
                .networks
                .iter()
                .filter(|network| network.name == "duplicate")
                .count(),
            1
        );
        assert!(snapshot.invariant_violations().is_empty());

        harness.shutdown().await;
    }

    #[tokio::test]
    async fn shutdown_stops_every_embedded_service_and_publishes_events() {
        let harness = TestHarness::new("shutdown-services").await;
        harness
            .runtime
            .create(create_command("alpha", false))
            .await
            .expect("create alpha");
        harness
            .runtime
            .create(create_command("beta", false))
            .await
            .expect("create beta");
        let mut alpha_events = harness.events.subscribe();
        let mut beta_events = harness.events.subscribe();

        let summary = harness.runtime.shutdown().await.expect("shutdown node");
        assert_eq!(summary.stopped, vec!["alpha", "beta"]);

        let snapshot = harness.state.snapshot().await;
        assert_eq!(snapshot.embedded_services.len(), 2);
        for service in &snapshot.embedded_services {
            assert_eq!(service.status, NodeEmbeddedServiceStatus::Stopped);
            assert!(service.signaling_addr.is_none());
            assert!(service.relay_addr.is_none());
        }
        assert_eq!(
            embedded_service_events(&mut alpha_events, "alpha"),
            vec![
                NodeEmbeddedServiceStatus::Stopping,
                NodeEmbeddedServiceStatus::Stopped,
            ]
        );
        assert_eq!(
            embedded_service_events(&mut beta_events, "beta"),
            vec![
                NodeEmbeddedServiceStatus::Stopping,
                NodeEmbeddedServiceStatus::Stopped,
            ]
        );

        let _ = std::fs::remove_dir_all(&harness.config_dir);
    }

    #[tokio::test]
    async fn create_failure_rolls_back_config_service_and_state() {
        let harness = TestHarness::new("create-rollback").await;
        harness.fake.fail("broken");

        let error = harness
            .runtime
            .create(create_command("broken", true))
            .await
            .expect_err("launch must fail");
        assert!(matches!(error, RuntimeControlError::Launch(_)));
        assert!(!harness.configs.network_exists("broken"));
        assert!(harness
            .services
            .list()
            .await
            .expect("list services")
            .is_empty());

        let snapshot = harness.state.snapshot().await;
        assert!(!snapshot
            .networks
            .iter()
            .any(|network| network.name == "broken"));
        assert!(!snapshot
            .configuration
            .networks
            .iter()
            .any(|network| network.network_name == "broken"));
        assert!(!snapshot
            .embedded_services
            .iter()
            .any(|service| service.network_name == "broken"));
        assert!(!snapshot
            .peers
            .iter()
            .any(|peer| peer.network_name == "broken"));
        assert!(!snapshot
            .memberships
            .iter()
            .any(|membership| membership.network_name == "broken"));
        assert!(harness
            .configs
            .joined_membership_for_network_name("broken")
            .expect("load rolled back membership")
            .is_none());
        assert!(snapshot.invariant_violations().is_empty());

        harness.shutdown().await;
    }

    #[tokio::test]
    async fn owner_created_network_can_authorize_routing_governance() {
        let harness = TestHarness::new("owner-routing-authorization").await;
        harness
            .runtime
            .create(create_command("owner-routing", true))
            .await
            .expect("create connected owner network");

        let snapshot = harness.state.snapshot().await;
        let membership = snapshot
            .memberships
            .iter()
            .find(|membership| membership.network_name == "owner-routing")
            .expect("owner membership");
        assert_eq!(membership.role, MembershipRole::Owner);
        assert_eq!(
            membership.state,
            fuckipv6_common::domain::MembershipState::Active
        );

        let bus = crate::node::spawn_command_bus_with_runtime(
            harness.state.clone(),
            MultiNetworkRegistry::new(),
            harness.events.clone(),
            harness.services.clone(),
            harness.runtime.clone(),
            harness.configs.clone(),
        );
        let context = crate::node::NodeCommandContext::local_owner_for_snapshot(
            &snapshot,
            "owner-routing-test",
        );
        let result = bus
            .execute(
                context,
                NodeCommandRequest::new(
                    601,
                    NodeCommand::UpdateRouting(routing_command(
                        "owner-routing",
                        NodeRoutingMode::GlobalProxy,
                        Some("exit-node"),
                        false,
                    )),
                ),
            )
            .await
            .expect("routing command result");
        assert!(
            result.is_success(),
            "routing command was rejected: {result:?}"
        );

        harness.shutdown().await;
    }

    #[tokio::test]
    async fn join_ticket_parses_network_and_persists_coordinator() {
        let harness = TestHarness::new("join-ticket").await;
        let ticket = NetworkTicket::new([7u8; 32], "127.0.0.1:6666", "invited").encode();

        let joined = harness
            .runtime
            .join(NodeJoinNetworkCommand {
                target: ticket,
                network_name: None,
                start: false,
                discovery_record: None,
                network_id: None,
                preview_digest: None,
                network_policy_revision: None,
                privacy_policy_revision: None,
                terms_hash: None,
                acknowledged_at_unix_ms: None,
                exit_contribution_acknowledged: None,
                signed_join_preview: None,
            })
            .await
            .expect("join ticket");
        assert_eq!(joined.network_name, "invited");
        assert_eq!(joined.coordinator_url, "ws://127.0.0.1:6666");
        assert!(joined.config_saved);
        assert!(!joined.started);

        let config_path = harness
            .configs
            .get_network_config("invited")
            .expect("persisted network config");
        let config = ClientConfig::from_file(config_path).expect("load persisted config");
        assert_eq!(config.coordinator_url, "ws://127.0.0.1:6666");

        harness.shutdown().await;
    }

    #[tokio::test]
    async fn repeated_invitation_join_binds_network_id_without_public_key() {
        let harness = TestHarness::new("repeat-invitation-join").await;
        let network_id = NetworkId::from_public_key(b"invitation-only-network");
        harness
            .configs
            .create_network("invited", "ws://127.0.0.1:6666", true)
            .expect("create legacy config without identity metadata");

        let joined = harness
            .runtime
            .join(NodeJoinNetworkCommand {
                target: "ws://127.0.0.1:6666".into(),
                network_name: Some("invited".into()),
                start: false,
                discovery_record: None,
                network_id: Some(network_id.to_string()),
                preview_digest: None,
                network_policy_revision: None,
                privacy_policy_revision: None,
                terms_hash: None,
                acknowledged_at_unix_ms: None,
                exit_contribution_acknowledged: None,
                signed_join_preview: None,
            })
            .await
            .expect("repeat invitation join");

        assert!(!joined.config_saved);
        assert!(!joined.started);
        let network = harness
            .configs
            .list_networks()
            .expect("list networks")
            .into_iter()
            .find(|network| network.name == "invited")
            .expect("existing network");
        assert_eq!(network.network_id.as_deref(), Some(network_id.as_str()));
        assert!(network.public_key.is_none());

        harness.shutdown().await;
    }

    #[tokio::test]
    async fn restart_reloads_persisted_config_before_relaunch() {
        let harness = TestHarness::new("restart-reload").await;
        harness
            .configs
            .create_network("reload", "ws://127.0.0.1:6666", true)
            .expect("create test config");
        let selection = NodeNetworkSelection::Named(vec!["reload".into()]);
        harness
            .runtime
            .start_selection(selection.clone())
            .await
            .expect("start original runtime");

        let config_path = harness
            .configs
            .get_network_config("reload")
            .expect("persisted network config");
        let mut config = ClientConfig::from_file(&config_path).expect("load config");
        let original_tun_name = config.tun.name.clone();
        config.tun.name = "restart-reloaded".into();
        config.save_to_file(&config_path).expect("rewrite config");

        let summary = harness
            .runtime
            .restart(vec!["reload".into()], false)
            .await
            .expect("restart network");
        assert_eq!(summary.requested, 1);
        assert_eq!(summary.restarted, vec!["reload"]);
        assert!(summary.failed.is_empty());
        assert!(summary.cleanup.residual_networks.is_empty());
        assert!(summary.cleanup.configuration_retained);
        assert_eq!(
            harness.fake.launched_tun_names(),
            vec![
                ("reload".into(), original_tun_name),
                ("reload".into(), "restart-reloaded".into()),
            ]
        );
        assert_eq!(harness.fake.shutdown_count(), 1);

        harness.shutdown().await;
    }

    #[tokio::test]
    async fn rebuild_tun_fails_closed_without_mutating_the_live_session() {
        let harness = TestHarness::new("rebuild-tun-blocked").await;
        start_fake_network(&harness, "blocked-rebuild").await;
        harness
            .state
            .update(|snapshot| {
                snapshot.tun_capabilities = TunCapabilities {
                    schema_version: TUN_CAPABILITY_SCHEMA_VERSION,
                    platform: TunPlatform::Linux,
                    supported: true,
                    can_rebuild: false,
                    reasons: vec![TunCapabilityReason::PermissionDenied],
                };
            })
            .await;
        let launches_before = harness.fake.launch_attempts();

        let error = harness
            .runtime
            .rebuild_tun("blocked-rebuild".into())
            .await
            .expect_err("unready TUN must reject rebuild");
        assert_eq!(error.message_key(), "error.network.permission_denied");
        assert_eq!(
            error.network_setup_stage(),
            Some(NetworkSetupStage::PermissionDenied)
        );
        assert_eq!(harness.fake.launch_attempts(), launches_before);
        assert_eq!(harness.fake.shutdown_count(), 0);
        let snapshot = harness.state.snapshot().await;
        assert_eq!(snapshot.networks[0].status, NodeNetworkStatus::Online);
        assert_eq!(harness.registry.get_all().await.len(), 1);

        harness.shutdown().await;
    }

    #[tokio::test]
    async fn rebuild_tun_restarts_the_same_network_when_capability_is_ready() {
        let harness = TestHarness::new("rebuild-tun-ready").await;
        harness
            .state
            .update(|snapshot| {
                snapshot.tun_capabilities = TunCapabilities {
                    schema_version: TUN_CAPABILITY_SCHEMA_VERSION,
                    platform: TunPlatform::Linux,
                    supported: true,
                    can_rebuild: true,
                    reasons: Vec::new(),
                };
            })
            .await;
        start_fake_network(&harness, "ready-rebuild").await;

        let summary = harness
            .runtime
            .rebuild_tun("ready-rebuild".into())
            .await
            .expect("rebuildable TUN should restart the network");
        assert_eq!(summary.requested, 1);
        assert_eq!(summary.restarted, vec!["ready-rebuild"]);
        assert!(summary.failed.is_empty());
        assert_eq!(harness.fake.shutdown_count(), 1);
        assert_eq!(
            harness.fake.launches(),
            vec!["ready-rebuild".to_string(), "ready-rebuild".to_string()]
        );
        let snapshot = harness.state.snapshot().await;
        assert_eq!(snapshot.networks[0].status, NodeNetworkStatus::Online);

        harness.shutdown().await;
    }

    #[tokio::test]
    async fn lifecycle_conflicts_cover_start_stop_restart_create_and_join() {
        let harness = TestHarness::new("lifecycle-conflict").await;
        harness
            .configs
            .create_network("inflight", "ws://127.0.0.1:6666", true)
            .expect("create test config");
        let selection = NodeNetworkSelection::Named(vec!["inflight".into()]);
        let plan = NodeRuntimePlan::load(&harness.configs, selection.clone()).expect("load plan");
        harness.fake.set_launch_delay(Duration::from_millis(250));
        let runtime = harness.runtime.clone();
        let start = tokio::spawn(async move { runtime.start_plan(plan).await });
        wait_for_launch_attempts(&harness.fake, 1).await;

        assert!(matches!(
            harness.runtime.stop(vec!["inflight".into()], false).await,
            Err(RuntimeControlError::Conflict(_))
        ));
        assert!(matches!(
            harness.runtime.restart(Vec::new(), true).await,
            Err(RuntimeControlError::Conflict(_))
        ));
        assert!(matches!(
            harness.runtime.start_node().await,
            Err(RuntimeControlError::Conflict(_))
        ));
        assert!(matches!(
            harness.runtime.stop_node().await,
            Err(RuntimeControlError::Conflict(_))
        ));
        assert!(matches!(
            harness.runtime.restart_node().await,
            Err(RuntimeControlError::Conflict(_))
        ));
        assert!(matches!(
            harness
                .runtime
                .create(create_command("blocked-create", false))
                .await,
            Err(RuntimeControlError::Conflict(_))
        ));
        let ticket = NetworkTicket::new([9u8; 32], "127.0.0.1:6666", "blocked-join").encode();
        assert!(matches!(
            harness
                .runtime
                .join(NodeJoinNetworkCommand {
                    target: ticket,
                    network_name: None,
                    start: false,
                    discovery_record: None,
                    network_id: None,
                    preview_digest: None,
                    network_policy_revision: None,
                    privacy_policy_revision: None,
                    terms_hash: None,
                    acknowledged_at_unix_ms: None,
                    exit_contribution_acknowledged: None,
                    signed_join_preview: None,
                })
                .await,
            Err(RuntimeControlError::Conflict(_))
        ));

        let summary = timeout(Duration::from_secs(2), start)
            .await
            .expect("start operation completed")
            .expect("start task")
            .expect("start plan");
        assert_eq!(summary.started, vec!["inflight"]);
        assert!(!harness.configs.network_exists("blocked-create"));
        assert!(!harness.configs.network_exists("blocked-join"));

        harness.shutdown().await;
    }

    #[tokio::test]
    async fn one_network_stop_failure_does_not_block_other_networks() {
        let harness = TestHarness::new_with_timeouts(
            "stop-isolation",
            RuntimeTimeouts {
                stop: Duration::from_millis(30),
                ..RuntimeTimeouts::default()
            },
        )
        .await;
        for name in ["fast-stop", "slow-stop"] {
            harness
                .configs
                .create_network(name, "ws://127.0.0.1:6666", false)
                .expect("create test config");
        }
        let plan = NodeRuntimePlan::load(
            &harness.configs,
            NodeNetworkSelection::Named(vec!["fast-stop".into(), "slow-stop".into()]),
        )
        .expect("load test plan");
        harness
            .runtime
            .start_plan(plan)
            .await
            .expect("start networks");
        harness
            .fake
            .set_shutdown_delay_for("slow-stop", Duration::from_millis(250));

        let summary = harness
            .runtime
            .stop(Vec::new(), true)
            .await
            .expect("partial stop returns a summary");
        assert_eq!(summary.stopped, vec!["fast-stop"]);
        assert_eq!(summary.failed.len(), 1);
        assert_eq!(summary.failed[0].network_name, "slow-stop");
        assert!(summary.failed[0].message.contains("stop/timeout"));
        let snapshot = harness.state.snapshot().await;
        assert_eq!(
            snapshot
                .networks
                .iter()
                .find(|network| network.name == "slow-stop")
                .expect("failed network")
                .status,
            NodeNetworkStatus::Failed
        );

        let lifecycle = harness.lifecycle.snapshot().await;
        assert_eq!(lifecycle.phase, NodePhase::Running);
        assert_eq!(lifecycle.active_networks, 1);
        assert_eq!(lifecycle.failed_networks, vec!["slow-stop"]);

        harness.shutdown().await;
    }

    #[tokio::test]
    async fn multi_network_stop_uses_one_shared_deadline() {
        let harness = TestHarness::new_with_timeouts(
            "stop-shared-deadline",
            RuntimeTimeouts {
                stop: Duration::from_millis(50),
                ..RuntimeTimeouts::default()
            },
        )
        .await;
        let names = (0..12)
            .map(|index| format!("parallel-{index:02}"))
            .collect::<Vec<_>>();
        for name in &names {
            harness
                .configs
                .create_network(name, "ws://127.0.0.1:6666", false)
                .expect("create test config");
        }
        let plan =
            NodeRuntimePlan::load(&harness.configs, NodeNetworkSelection::Named(names.clone()))
                .expect("load test plan");
        harness
            .runtime
            .start_plan(plan)
            .await
            .expect("start networks");
        harness.fake.set_shutdown_delay(Duration::from_secs(2));

        let started = Instant::now();
        let summary = timeout(Duration::from_secs(2), harness.runtime.stop(names, false))
            .await
            .expect("parallel stop must finish")
            .expect("stop returns a summary");
        assert!(summary.stopped.is_empty());
        assert_eq!(summary.failed.len(), 12);
        assert!(
            started.elapsed() < Duration::from_millis(400),
            "network shutdowns were serialized: {:?}",
            started.elapsed()
        );
        assert_eq!(summary.cleanup.runtime_outcomes.len(), 12);

        harness.shutdown().await;
    }

    #[tokio::test]
    async fn stop_timeout_keeps_residue_until_the_runtime_reports_cleanup() {
        let harness = TestHarness::new_with_timeouts(
            "stop-timeout",
            RuntimeTimeouts {
                stop: Duration::from_millis(20),
                ..RuntimeTimeouts::default()
            },
        )
        .await;
        start_fake_network(&harness, "slow-stop").await;
        harness.fake.set_shutdown_delay(Duration::from_millis(250));

        let summary = harness
            .runtime
            .stop(vec!["slow-stop".into()], false)
            .await
            .expect("slow stop must return a partial-failure summary");
        assert!(summary.stopped.is_empty());
        assert_eq!(summary.failed.len(), 1);
        assert_eq!(summary.failed[0].network_name, "slow-stop");
        assert!(summary.failed[0].message.contains("stop/timeout"));
        assert!(summary.failed[0].message.contains("清理结果未知"));
        assert_eq!(summary.cleanup.sessions_remaining, 1);
        assert_eq!(summary.cleanup.listeners_remaining, 1);
        assert_eq!(summary.cleanup.residual_networks, vec!["slow-stop"]);
        assert_eq!(harness.registry.get_all().await.len(), 1);
        let snapshot = harness.state.snapshot().await;
        let network = snapshot
            .networks
            .iter()
            .find(|network| network.name == "slow-stop")
            .expect("failed network state");
        assert_eq!(network.status, NodeNetworkStatus::Failed);

        let restarted = harness
            .runtime
            .restart(vec!["slow-stop".into()], false)
            .await
            .expect("restart waits for the real cleanup outcome");
        assert_eq!(restarted.restarted, vec!["slow-stop"]);
        assert!(restarted.failed.is_empty());
        assert!(restarted
            .cleanup
            .runtime_outcomes
            .get("slow-stop")
            .expect("runtime cleanup outcome")
            .is_clean());
        let snapshot = harness.state.snapshot().await;
        let network = snapshot
            .networks
            .iter()
            .find(|network| network.name == "slow-stop")
            .expect("restarted network state");
        assert_eq!(network.status, NodeNetworkStatus::Online);
        assert_eq!(harness.fake.launch_attempts(), 2);

        harness.fake.set_shutdown_delay(Duration::ZERO);
        let retried = harness
            .runtime
            .stop(vec!["slow-stop".into()], false)
            .await
            .expect("stop after restart cleans the replacement runtime");
        assert_eq!(retried.stopped, vec!["slow-stop"]);
        assert!(retried.failed.is_empty());
        assert!(harness.registry.get_all().await.is_empty());
        let snapshot = harness.state.snapshot().await;
        let network = snapshot
            .networks
            .iter()
            .find(|network| network.name == "slow-stop")
            .expect("offline network state");
        assert_eq!(network.status, NodeNetworkStatus::Offline);

        harness.shutdown().await;
    }

    #[tokio::test]
    async fn cleanup_failure_keeps_residue_and_blocks_restart_before_launch() {
        let harness = TestHarness::new("cleanup-failure").await;
        start_fake_network(&harness, "broken-cleanup").await;
        harness
            .state
            .update(|snapshot| {
                if let Some(network) = snapshot
                    .networks
                    .iter_mut()
                    .find(|network| network.name == "broken-cleanup")
                {
                    network.peer_count = 1;
                }
                snapshot.peers.push(peer_snapshot("broken-cleanup"));
            })
            .await;
        harness.fake.fail_cleanup("broken-cleanup");

        let summary = harness
            .runtime
            .stop(vec!["broken-cleanup".into()], false)
            .await
            .expect("cleanup failure returns a partial-failure summary");
        assert!(summary.stopped.is_empty());
        assert_eq!(summary.failed.len(), 1);
        let outcome = summary
            .cleanup
            .runtime_outcomes
            .get("broken-cleanup")
            .expect("runtime cleanup outcome");
        assert!(!outcome.is_clean());
        assert!(outcome.summary().contains("测试清理失败"));
        assert_eq!(summary.cleanup.peers_remaining, 1);
        assert_eq!(summary.cleanup.listeners_remaining, 1);
        assert_eq!(summary.cleanup.residual_networks, vec!["broken-cleanup"]);
        assert_eq!(harness.registry.get_all().await.len(), 1);
        let snapshot = harness.state.snapshot().await;
        let network = snapshot
            .networks
            .iter()
            .find(|network| network.name == "broken-cleanup")
            .expect("failed network state");
        assert_eq!(network.status, NodeNetworkStatus::Failed);
        assert_eq!(network.peer_count, 1);
        assert!(snapshot
            .peers
            .iter()
            .any(|peer| peer.network_name == "broken-cleanup"));

        let launches_before_restart = harness.fake.launch_attempts();
        let restart = harness
            .runtime
            .restart(vec!["broken-cleanup".into()], false)
            .await
            .expect("restart returns a cleanup-phase failure");
        assert!(restart.restarted.is_empty());
        assert_eq!(restart.failed.len(), 1);
        assert_eq!(restart.failed[0].phase, RestartPhase::Cleanup);
        assert!(restart.failed[0].message.contains("测试清理失败"));
        assert_eq!(harness.fake.launch_attempts(), launches_before_restart);
        assert_eq!(restart.cleanup.residual_networks, vec!["broken-cleanup"]);
        assert_eq!(harness.registry.get_all().await.len(), 1);

        harness.shutdown().await;
    }

    #[test]
    fn diagnostic_projection_uses_fixed_layers_and_all_five_states() {
        let clean_residue = NodeRuntimeResidueSnapshot::default();
        let mut snapshot = NodeSnapshot {
            lifecycle: crate::node::NodeLifecycleSnapshot {
                phase: NodePhase::Running,
                configured_networks: 1,
                active_networks: 1,
                failed_networks: Vec::new(),
            },
            ..NodeSnapshot::default()
        };

        let checks = build_diagnostic_checks(&snapshot, &clean_residue, 0);
        assert_eq!(
            checks.iter().map(|check| check.layer).collect::<Vec<_>>(),
            NodeDiagnosticLayer::ALL
        );
        assert_eq!(checks[0].state, NodeDiagnosticState::Passed);
        assert_eq!(checks[1].state, NodeDiagnosticState::NotTested);
        assert_eq!(checks[1].message_key, "diagnostic.dns.not_tested");
        assert!(checks.iter().all(|check| {
            check.started_at_unix_ms >= 1 && check.message_key.starts_with("diagnostic.")
        }));

        snapshot.lifecycle.phase = NodePhase::Failed;
        assert_eq!(
            diagnostic_result(NodeDiagnosticLayer::Network, &snapshot, &clean_residue).state,
            NodeDiagnosticState::Failed
        );

        snapshot.tun_capabilities.supported = false;
        assert_eq!(
            diagnostic_result(NodeDiagnosticLayer::Tun, &snapshot, &clean_residue).state,
            NodeDiagnosticState::Unsupported
        );

        snapshot.tun_capabilities = TunCapabilities {
            schema_version: TUN_CAPABILITY_SCHEMA_VERSION,
            platform: TunPlatform::current(),
            supported: true,
            can_rebuild: true,
            reasons: vec![TunCapabilityReason::PermissionDenied],
        };
        assert_eq!(
            diagnostic_result(NodeDiagnosticLayer::Tun, &snapshot, &clean_residue).state,
            NodeDiagnosticState::NoPermission
        );

        let stopped = NodeSnapshot::default();
        assert_eq!(
            diagnostic_result(NodeDiagnosticLayer::Network, &stopped, &clean_residue).state,
            NodeDiagnosticState::NotTested
        );
    }

    #[test]
    fn diagnostic_recovery_and_report_are_actionable_and_sanitized() {
        let checks = vec![
            NodeDiagnosticCheck {
                layer: NodeDiagnosticLayer::Network,
                state: NodeDiagnosticState::Failed,
                started_at_unix_ms: 10,
                duration_ms: 2,
                error_code: Some("diagnostic.network.lifecycle_failed".into()),
                message_key: "diagnostic.state.failed".into(),
                suggestion_key: Some("diagnostic.suggestion.restart_node".into()),
            },
            NodeDiagnosticCheck {
                layer: NodeDiagnosticLayer::Tun,
                state: NodeDiagnosticState::NoPermission,
                started_at_unix_ms: 11,
                duration_ms: 1,
                error_code: Some("diagnostic.tun.permission_denied".into()),
                message_key: "diagnostic.state.no_permission".into(),
                suggestion_key: Some("diagnostic.suggestion.grant_network_privileges".into()),
            },
        ];
        let residue = NodeRuntimeResidueSnapshot {
            sessions_remaining: 1,
            residual_networks: vec!["private-home-network".into()],
            ..NodeRuntimeResidueSnapshot::default()
        };
        let actions = diagnostic_recovery_actions(&checks, &residue);
        assert_eq!(
            actions
                .iter()
                .map(|action| action.action_id.as_str())
                .collect::<Vec<_>>(),
            vec!["restart_node", "rebuild_tun", "cleanup_residuals"]
        );
        assert!(actions.iter().all(|action| {
            action.confirmation_required && action.impact_key.starts_with("diagnostic.recovery.")
        }));

        let report = build_sanitized_diagnostic_report(
            "diag-10-00000001",
            10,
            12,
            false,
            &checks,
            &ExitHealthSnapshot::default(),
            &LeakDetectionSnapshot::default(),
            &actions,
            &residue,
        );
        assert!(report.contains("diag-10-00000001"));
        assert!(report.contains("network: failed"));
        assert!(report.contains("tun: no_permission"));
        assert!(report.contains("confirmation_required=true"));
        assert!(!report.contains("private-home-network"));
        assert!(!report.contains("/home/"));
        assert!(!report.contains("token"));
        assert!(!report.contains("private-key"));
    }

    #[test]
    fn exit_health_never_infers_dns_or_internet_from_a_healthy_process() {
        let snapshot = NodeSnapshot {
            local_exit: LocalExitCapabilitySnapshot {
                owner_exit_enabled: true,
                stage: LocalExitCapabilityStage::Enabled,
                healthy: true,
                network_allowed: true,
                ..LocalExitCapabilitySnapshot::default()
            },
            networks: vec![crate::node::NodeNetworkSnapshot {
                name: "exit-network".into(),
                tun_name: "fuckipv6-test".into(),
                status: NodeNetworkStatus::Online,
                assigned_ipv6: None,
                assigned_ipv4: None,
                p2p_listen_addr: None,
                uptime_secs: 1,
                roles: vec![NodeNetworkRole::ExitNode],
                peer_count: 0,
                traffic: NodeTrafficSnapshot::default(),
            }],
            ..NodeSnapshot::default()
        };

        let now = 1_700_000_000_000;
        let leak_detection = LeakDetectionSnapshot {
            generated_at_unix_ms: now,
            evidence: [
                LeakDetectionTarget::DnsResolver,
                LeakDetectionTarget::Ipv4ExternalTarget,
                LeakDetectionTarget::Ipv6ExternalTarget,
            ]
            .into_iter()
            .map(|target| LeakDetectionEvidence {
                target,
                state: LeakDetectionEvidenceState::NotTested,
                source: LeakDetectionSource::ControlledProbe,
                observed_at_unix_ms: now,
                duration_ms: 0,
                error_code: Some("exit.health.probe_not_executed".into()),
                isolation_action: None,
            })
            .collect(),
            ..LeakDetectionSnapshot::default()
        };

        let health = build_exit_health_snapshot(
            &snapshot,
            &leak_detection,
            1_700_000_000_000,
            1_700_000_001_000,
        );
        let assessment = health.assess(1_700_000_001_000);
        assert_eq!(
            assessment.state,
            fuckipv6_common::domain::ExitHealthState::Unknown
        );
        assert!(!assessment.default_candidate);
        assert!(assessment.affected_layers.contains(&ExitHealthLayer::Dns));
        assert!(assessment
            .affected_layers
            .contains(&ExitHealthLayer::InternetReachability));
        assert!(assessment
            .affected_targets
            .contains(&ExitHealthTarget::Ipv6Internet));
        assert_eq!(health.retest.phase, ExitHealthRetestPhase::Completed);
    }

    #[test]
    fn leak_evidence_is_projected_without_fabricating_a_local_exit_role() {
        let now = 1_700_000_000_000;
        let leak_detection = LeakDetectionSnapshot {
            generated_at_unix_ms: now,
            evidence: vec![
                LeakDetectionEvidence {
                    target: LeakDetectionTarget::DnsResolver,
                    state: LeakDetectionEvidenceState::Passed,
                    source: LeakDetectionSource::ManagedDoh,
                    observed_at_unix_ms: now,
                    duration_ms: 2,
                    error_code: None,
                    isolation_action: None,
                },
                LeakDetectionEvidence {
                    target: LeakDetectionTarget::BrowserApplicationDns,
                    state: LeakDetectionEvidenceState::NotTested,
                    source: LeakDetectionSource::ApplicationBehaviorProbe,
                    observed_at_unix_ms: now,
                    duration_ms: 0,
                    error_code: Some("leak_detection.probe_not_configured".into()),
                    isolation_action: None,
                },
                LeakDetectionEvidence {
                    target: LeakDetectionTarget::Ipv4DefaultRoute,
                    state: LeakDetectionEvidenceState::Failed,
                    source: LeakDetectionSource::LocalRoutingTable,
                    observed_at_unix_ms: now,
                    duration_ms: 4,
                    error_code: Some(
                        "leak_detection.ipv4_default_route_outside_protected_path".into(),
                    ),
                    isolation_action: None,
                },
                LeakDetectionEvidence {
                    target: LeakDetectionTarget::Ipv6Carrier,
                    state: LeakDetectionEvidenceState::Unsupported,
                    source: LeakDetectionSource::LinkCapability,
                    observed_at_unix_ms: now,
                    duration_ms: 1,
                    error_code: Some("leak_detection.ipv6_carrier_unavailable".into()),
                    isolation_action: Some(LeakDetectionIsolationAction::BlockedUnavailableIpv6),
                },
            ],
            ..LeakDetectionSnapshot::default()
        };

        let health =
            build_exit_health_snapshot(&NodeSnapshot::default(), &leak_detection, now - 1, now);
        assert!(!health
            .evidence
            .iter()
            .any(|evidence| evidence.layer == ExitHealthLayer::Process));
        assert_eq!(health.evidence.len(), 3);
        assert_eq!(
            health.evidence[0].target,
            Some(ExitHealthTarget::Controlled(
                "browser_application_dns".into()
            ))
        );
        assert_eq!(health.evidence[1].state, ExitHealthEvidenceState::Failed);
        assert_eq!(
            health.evidence[2].state,
            ExitHealthEvidenceState::Unsupported
        );
        let assessment = health.assess(now);
        assert_eq!(
            assessment.state,
            fuckipv6_common::domain::ExitHealthState::Unknown
        );
        assert!(assessment
            .affected_layers
            .contains(&ExitHealthLayer::Process));
        assert!(assessment
            .affected_layers
            .contains(&ExitHealthLayer::Listening));
        assert!(assessment
            .affected_layers
            .contains(&ExitHealthLayer::Network));
    }

    #[test]
    fn leak_evidence_aggregation_is_worst_first_then_latest_and_deterministic() {
        let now = 1_700_000_000_000;
        let evidence = |target, state, observed_at_unix_ms| LeakDetectionEvidence {
            target,
            state,
            source: LeakDetectionSource::ControlledProbe,
            observed_at_unix_ms,
            duration_ms: 1,
            error_code: (state == LeakDetectionEvidenceState::Failed)
                .then(|| "leak_detection.check_failed".into()),
            isolation_action: None,
        };
        let leak_detection = LeakDetectionSnapshot {
            generated_at_unix_ms: now,
            evidence: vec![
                evidence(
                    LeakDetectionTarget::DnsResolver,
                    LeakDetectionEvidenceState::Passed,
                    now,
                ),
                evidence(
                    LeakDetectionTarget::BrowserApplicationDns,
                    LeakDetectionEvidenceState::Passed,
                    now,
                ),
                evidence(
                    LeakDetectionTarget::Ipv4Carrier,
                    LeakDetectionEvidenceState::Passed,
                    now + 3,
                ),
                evidence(
                    LeakDetectionTarget::Ipv4SourceAddress,
                    LeakDetectionEvidenceState::Unsupported,
                    now + 2,
                ),
                evidence(
                    LeakDetectionTarget::Ipv4DefaultRoute,
                    LeakDetectionEvidenceState::NotTested,
                    now + 1,
                ),
                evidence(
                    LeakDetectionTarget::Ipv4ControlledLeakService,
                    LeakDetectionEvidenceState::Failed,
                    now,
                ),
                LeakDetectionEvidence {
                    isolation_action: Some(LeakDetectionIsolationAction::BlockedUnavailableIpv6),
                    ..evidence(
                        LeakDetectionTarget::Ipv6Carrier,
                        LeakDetectionEvidenceState::Unsupported,
                        now,
                    )
                },
            ],
            ..LeakDetectionSnapshot::default()
        };

        let projected = aggregate_leak_evidence_for_exit_health(&leak_detection);

        assert_eq!(projected.len(), 3);
        assert_eq!(
            projected[0].target,
            LeakDetectionTarget::BrowserApplicationDns,
            "equal-severity evidence uses the stable target key"
        );
        assert_eq!(
            projected[1].target,
            LeakDetectionTarget::Ipv4ControlledLeakService,
            "the worst state must win over newer passed or not-tested evidence"
        );
        assert_eq!(projected[1].state, LeakDetectionEvidenceState::Failed);
        assert_eq!(
            projected[2].isolation_action,
            Some(LeakDetectionIsolationAction::BlockedUnavailableIpv6),
            "isolated unsupported evidence must remain unsupported"
        );
    }

    #[test]
    fn leak_report_drops_non_token_error_text() {
        let now = 1_700_000_000_000;
        let leak_detection = LeakDetectionSnapshot {
            generated_at_unix_ms: now,
            evidence: vec![LeakDetectionEvidence {
                target: LeakDetectionTarget::Ipv4DefaultRoute,
                state: LeakDetectionEvidenceState::Failed,
                source: LeakDetectionSource::LocalRoutingTable,
                observed_at_unix_ms: now,
                duration_ms: 1,
                error_code: Some("http://resolver.example/1.2.3.4 /home/private".into()),
                isolation_action: None,
            }],
            ..LeakDetectionSnapshot::default()
        };
        let report = build_sanitized_diagnostic_report(
            "diag-sanitized",
            now - 1,
            now,
            false,
            &[],
            &ExitHealthSnapshot::default(),
            &leak_detection,
            &[],
            &NodeRuntimeResidueSnapshot::default(),
        );

        assert!(report.contains("leak_detection:"));
        assert!(report.contains("ipv4_default_route: state=failed"));
        assert!(!report.contains("resolver.example"));
        assert!(!report.contains("1.2.3.4"));
        assert!(!report.contains("/home/private"));
    }

    #[tokio::test]
    async fn diagnostic_timeout_and_cancellation_release_actor_state() {
        let (_timeout_sender, timeout_cancel) = watch::channel(false);
        let timed_out = run_diagnostic_collection(
            Duration::from_millis(1),
            timeout_cancel,
            std::future::pending::<Result<(), RuntimeControlError>>(),
        )
        .await;
        assert!(matches!(timed_out, Err(RuntimeControlError::Timeout(_))));

        let (cancel_sender, cancel) = watch::channel(false);
        let cancelled_run = run_diagnostic_collection(
            Duration::from_secs(1),
            cancel,
            std::future::pending::<Result<(), RuntimeControlError>>(),
        );
        tokio::pin!(cancelled_run);
        assert!(!cancelled_run.as_mut().now_or_never().is_some());
        cancel_sender.send_replace(true);
        assert!(matches!(
            cancelled_run.await,
            Err(RuntimeControlError::Cancelled(_))
        ));

        let cancelled = TestHarness::new("diagnostic-cancelled").await;
        let launches_before = cancelled.fake.launch_attempts();
        let (_cancel_sender, cancel) = watch::channel(true);
        assert!(matches!(
            cancelled.runtime.diagnostics_cancellable(cancel).await,
            Err(RuntimeControlError::Cancelled(_))
        ));
        assert_eq!(cancelled.fake.launch_attempts(), launches_before);
        assert!(cancelled.registry.get_all().await.is_empty());

        let diagnostics = cancelled
            .runtime
            .diagnostics()
            .await
            .expect("cancellation must leave diagnostics available");
        assert_eq!(diagnostics.checks.len(), NodeDiagnosticLayer::ALL.len());
        assert!(diagnostics.residue.is_clean());
        assert!(diagnostics.recovery_actions.iter().all(|action| {
            action.confirmation_required && action.impact_key.starts_with("diagnostic.recovery.")
        }));
        cancelled.shutdown().await;
    }

    #[tokio::test]
    async fn diagnostics_track_retained_owner_and_successful_cleanup_retry() {
        let harness = TestHarness::new("diagnostics-cleanup-retry").await;
        start_fake_network(&harness, "diagnostics-retry").await;
        harness.fake.fail_cleanup("diagnostics-retry");

        let stopped = harness
            .runtime
            .stop(vec!["diagnostics-retry".into()], false)
            .await
            .expect("cleanup failure returns a stop summary");
        assert_eq!(stopped.failed.len(), 1);

        let diagnostics = harness
            .runtime
            .diagnostics()
            .await
            .expect("read runtime diagnostics");
        assert_eq!(
            diagnostics.retained_owner_networks,
            vec!["diagnostics-retry"]
        );
        assert_eq!(
            diagnostics.cleanup_candidate_networks,
            vec!["diagnostics-retry"]
        );
        assert_eq!(
            diagnostics.residue.residual_networks,
            vec!["diagnostics-retry"]
        );
        assert_eq!(diagnostics.residue.cleanup_steps.len(), 1);
        assert_eq!(
            diagnostics.residue.cleanup_steps[0].status,
            NodeCleanupStepStatus::Failed
        );
        assert!(!diagnostics.residue.is_clean());

        let shutdowns_before_retry = harness.fake.shutdown_count();
        harness.fake.recover_cleanup("diagnostics-retry");
        let cleanup = harness
            .runtime
            .cleanup_residuals(vec!["diagnostics-retry".into()], false)
            .await
            .expect("retry retained runtime cleanup");
        assert_eq!(cleanup.stopped, vec!["diagnostics-retry"]);
        assert!(cleanup.failed.is_empty());
        assert!(cleanup.cleanup.is_clean_for_stop());
        assert!(harness.registry.get_all().await.is_empty());
        assert!(harness.fake.shutdown_count() > shutdowns_before_retry);

        let diagnostics = harness
            .runtime
            .diagnostics()
            .await
            .expect("read post-cleanup diagnostics");
        assert!(diagnostics.retained_owner_networks.is_empty());
        assert!(diagnostics.cleanup_candidate_networks.is_empty());
        assert!(diagnostics.residue.is_clean());
        assert_eq!(
            diagnostics.residue.cleanup_steps[0].status,
            NodeCleanupStepStatus::Removed
        );

        harness.shutdown().await;
    }

    #[tokio::test]
    async fn cleanup_never_bypasses_a_running_session_owner() {
        let harness = TestHarness::new("cleanup-running-session").await;
        start_fake_network(&harness, "running-cleanup").await;
        let shutdowns_before = harness.fake.shutdown_count();

        let error = harness
            .runtime
            .cleanup_residuals(vec!["running-cleanup".into()], false)
            .await
            .expect_err("running sessions must not be cleaned behind their owner");
        assert!(matches!(error, RuntimeControlError::Conflict(_)));
        assert_eq!(harness.registry.get_all().await.len(), 1);
        assert_eq!(harness.fake.shutdown_count(), shutdowns_before);
        assert!(harness
            .runtime
            .diagnostics()
            .await
            .expect("read running diagnostics")
            .residue
            .residual_networks
            .contains(&"running-cleanup".into()));

        harness.shutdown().await;
    }

    #[tokio::test]
    async fn cleanup_reports_an_unavailable_owner_without_dropping_the_candidate() {
        let harness = TestHarness::new("cleanup-owner-unavailable").await;
        start_fake_network(&harness, "lost-owner").await;
        harness.fake.fail_cleanup("lost-owner");
        harness.fake.lose_owner("lost-owner");

        harness
            .runtime
            .stop(vec!["lost-owner".into()], false)
            .await
            .expect("stop records failed cleanup");
        let diagnostics = harness
            .runtime
            .diagnostics()
            .await
            .expect("read lost-owner diagnostics");
        assert!(diagnostics.retained_owner_networks.is_empty());
        assert_eq!(diagnostics.cleanup_candidate_networks, vec!["lost-owner"]);

        let cleanup = harness
            .runtime
            .cleanup_residuals(vec!["lost-owner".into()], false)
            .await
            .expect("owner loss is a partial cleanup failure");
        assert!(cleanup.stopped.is_empty());
        assert_eq!(cleanup.failed.len(), 1);
        assert_eq!(
            cleanup.to_public_cleanup_result().failed_networks[0].reason,
            NodeCleanupFailureReason::OwnerUnavailable
        );
        assert!(harness
            .runtime
            .diagnostics()
            .await
            .expect("read remaining diagnostics")
            .cleanup_candidate_networks
            .contains(&"lost-owner".into()));

        harness.shutdown().await;
    }

    #[tokio::test]
    async fn cleanup_timeout_retains_the_owner_and_can_be_retried() {
        let harness = TestHarness::new_with_timeouts(
            "cleanup-timeout",
            RuntimeTimeouts {
                stop: Duration::from_millis(40),
                ..RuntimeTimeouts::default()
            },
        )
        .await;
        start_fake_network(&harness, "slow-cleanup").await;
        harness.fake.fail_cleanup("slow-cleanup");
        harness
            .runtime
            .stop(vec!["slow-cleanup".into()], false)
            .await
            .expect("initial stop records failed cleanup");

        harness
            .fake
            .set_residual_cleanup_delay("slow-cleanup", Duration::from_millis(200));
        let timed_out = harness
            .runtime
            .cleanup_residuals(vec!["slow-cleanup".into()], false)
            .await
            .expect("cleanup timeout is a partial failure");
        assert!(timed_out.stopped.is_empty());
        assert_eq!(timed_out.failed.len(), 1);
        assert_eq!(
            timed_out.to_public_cleanup_result().failed_networks[0].reason,
            NodeCleanupFailureReason::TimedOut
        );
        let diagnostics = harness
            .runtime
            .diagnostics()
            .await
            .expect("read timed-out diagnostics");
        assert_eq!(diagnostics.retained_owner_networks, vec!["slow-cleanup"]);
        assert!(diagnostics
            .residue
            .cleanup_steps
            .iter()
            .any(|step| step.status == NodeCleanupStepStatus::Deferred));

        harness
            .fake
            .set_residual_cleanup_delay("slow-cleanup", Duration::ZERO);
        harness.fake.recover_cleanup("slow-cleanup");
        let retried = harness
            .runtime
            .cleanup_residuals(vec!["slow-cleanup".into()], false)
            .await
            .expect("retry cleanup after timeout");
        assert_eq!(retried.stopped, vec!["slow-cleanup"]);
        assert!(retried.failed.is_empty());

        harness.shutdown().await;
    }

    #[tokio::test]
    async fn cleanup_all_returns_partial_success_and_retains_failed_owners() {
        let harness = TestHarness::new("cleanup-all-partial").await;
        start_fake_network(&harness, "cleanup-alpha").await;
        start_fake_network(&harness, "cleanup-beta").await;
        harness.fake.fail_cleanup("cleanup-alpha");
        harness.fake.fail_cleanup("cleanup-beta");
        harness
            .runtime
            .stop(Vec::new(), true)
            .await
            .expect("initial stop leaves two retained owners");
        harness.fake.recover_cleanup("cleanup-beta");

        let cleanup = harness
            .runtime
            .cleanup_residuals(Vec::new(), true)
            .await
            .expect("cleanup all returns partial success");
        assert_eq!(cleanup.stopped, vec!["cleanup-beta"]);
        assert_eq!(cleanup.failed.len(), 1);
        assert_eq!(cleanup.failed[0].network_name, "cleanup-alpha");
        let public = cleanup.to_public_cleanup_result();
        assert_eq!(public.cleaned_networks, vec!["cleanup-beta"]);
        assert_eq!(public.failed_networks.len(), 1);
        assert_eq!(
            public.failed_networks[0].reason,
            NodeCleanupFailureReason::Incomplete
        );

        let diagnostics = harness
            .runtime
            .diagnostics()
            .await
            .expect("read partial cleanup diagnostics");
        assert_eq!(diagnostics.retained_owner_networks, vec!["cleanup-alpha"]);
        assert_eq!(
            diagnostics.cleanup_candidate_networks,
            vec!["cleanup-alpha"]
        );
        assert_eq!(diagnostics.residue.residual_networks, vec!["cleanup-alpha"]);

        harness.shutdown().await;
    }

    #[tokio::test]
    async fn stop_node_with_cleanup_residue_reports_a_failed_lifecycle() {
        let harness = TestHarness::new("stop-node-cleanup-failure").await;
        start_fake_network(&harness, "broken-stop-node").await;
        harness.fake.fail_cleanup("broken-stop-node");

        let summary = harness
            .runtime
            .stop_node()
            .await
            .expect("cleanup failure returns a partial-failure summary");
        assert!(!summary.is_success());
        assert!(!summary.failed.is_empty());
        assert!(!summary.cleanup.residual_networks.is_empty());
        assert_eq!(harness.lifecycle.snapshot().await.phase, NodePhase::Failed);
        assert_eq!(harness.registry.get_all().await.len(), 1);

        harness.shutdown().await;
    }

    #[tokio::test]
    async fn start_timeout_records_failed_network_and_error() {
        let harness = TestHarness::new_with_timeouts(
            "start-timeout",
            RuntimeTimeouts {
                start: Duration::from_millis(20),
                ..RuntimeTimeouts::default()
            },
        )
        .await;
        harness
            .configs
            .create_network("slow-start", "ws://127.0.0.1:6666", true)
            .expect("create test config");
        harness.fake.set_launch_delay(Duration::from_millis(250));

        let summary = harness
            .runtime
            .start_selection(NodeNetworkSelection::Named(vec!["slow-start".into()]))
            .await
            .expect("start returns a per-network summary");
        assert!(summary.started.is_empty());
        assert_eq!(summary.failed.len(), 1);
        assert_eq!(summary.failed[0].network_name, "slow-start");
        assert!(summary.failed[0].message.contains("start/timeout"));

        let snapshot = harness.state.snapshot().await;
        let network = snapshot
            .networks
            .iter()
            .find(|network| network.name == "slow-start")
            .expect("failed network state");
        assert_eq!(network.status, NodeNetworkStatus::Failed);
        let problem = snapshot.last_error.as_ref().expect("start failure problem");
        assert_eq!(problem.network_name.as_deref(), Some("slow-start"));
        assert!(problem.message.contains("start/timeout"));
        assert!(harness.registry.get_all().await.is_empty());

        harness.shutdown().await;
    }

    #[tokio::test]
    async fn restart_launch_failure_is_reported_in_start_phase() {
        let harness = TestHarness::new("restart-launch-failure").await;
        start_fake_network(&harness, "restart-failure").await;
        harness.fake.fail("restart-failure");

        let summary = harness
            .runtime
            .restart(vec!["restart-failure".into()], false)
            .await
            .expect("restart returns a per-network summary");
        assert!(summary.restarted.is_empty());
        assert_eq!(summary.failed.len(), 1);
        assert_eq!(summary.failed[0].network_name, "restart-failure");
        assert_eq!(summary.failed[0].phase, RestartPhase::Start);
        assert!(summary.failed[0].message.contains("测试启动失败"));
        assert!(summary.cleanup.residual_networks.is_empty());
        let snapshot = harness.state.snapshot().await;
        assert_eq!(
            snapshot
                .networks
                .iter()
                .find(|network| network.name == "restart-failure")
                .expect("failed network state")
                .status,
            NodeNetworkStatus::Failed
        );

        harness.shutdown().await;
    }

    #[tokio::test]
    async fn restart_node_failure_converges_to_failed_instead_of_stopping() {
        let harness = TestHarness::new("restart-node-failed").await;
        harness
            .configs
            .create_network("restart-node", "ws://127.0.0.1:6666", false)
            .expect("create test config");
        let plan = NodeRuntimePlan::load(
            &harness.configs,
            NodeNetworkSelection::Named(vec!["restart-node".into()]),
        )
        .expect("load test plan");
        harness
            .runtime
            .start_plan(plan)
            .await
            .expect("start test network");

        // Closing the service actor forces the restart to fail after entering
        // the stopping phase, exercising lifecycle error convergence.
        harness
            .services
            .shutdown()
            .await
            .expect("close service manager");
        let error = harness
            .runtime
            .restart_node()
            .await
            .expect_err("restart must report the unavailable service manager");
        assert!(matches!(error, RuntimeControlError::ActorClosed));

        let lifecycle = harness.lifecycle.snapshot().await;
        assert_eq!(lifecycle.phase, NodePhase::Failed);
        assert_ne!(lifecycle.phase, NodePhase::Stopping);

        let _ = harness
            .runtime
            .stop(vec!["restart-node".into()], false)
            .await;
        harness.shutdown().await;
    }

    #[test]
    fn cleanup_residue_blocks_only_the_affected_network() {
        let report = RuntimeCleanupReport {
            residual_networks: vec!["alpha".into()],
            ..RuntimeCleanupReport::default()
        };
        assert!(report.blocks_restart("alpha"));
        assert!(!report.blocks_restart("beta"));
    }

    #[test]
    fn stop_success_requires_a_clean_cleanup_report() {
        let mut clean_outcome = RuntimeCleanupOutcome::default();
        clean_outcome.record("TUN 设备", CleanupStatus::Removed);
        clean_outcome.record("Exit 转发规则", CleanupStatus::NotOwned);
        let clean = StopSummary {
            stopped: vec!["alpha".into()],
            cleanup: RuntimeCleanupReport {
                runtime_outcomes: BTreeMap::from([("alpha".into(), clean_outcome)]),
                ..RuntimeCleanupReport::default()
            },
            ..StopSummary::default()
        };
        assert!(clean.is_success());

        let residue_reports = [
            RuntimeCleanupReport {
                sessions_remaining: 1,
                residual_networks: vec!["alpha".into()],
                ..RuntimeCleanupReport::default()
            },
            RuntimeCleanupReport {
                listeners_remaining: 1,
                residual_networks: vec!["alpha".into()],
                ..RuntimeCleanupReport::default()
            },
            RuntimeCleanupReport {
                peers_remaining: 1,
                residual_networks: vec!["alpha".into()],
                ..RuntimeCleanupReport::default()
            },
            RuntimeCleanupReport {
                embedded_services_remaining: 1,
                ..RuntimeCleanupReport::default()
            },
            RuntimeCleanupReport {
                residual_networks: vec!["alpha".into()],
                ..RuntimeCleanupReport::default()
            },
        ];
        for report in residue_reports {
            let summary = StopSummary {
                stopped: vec!["alpha".into()],
                cleanup: report,
                ..StopSummary::default()
            };
            assert!(!summary.is_success(), "residue must fail the stop");
        }

        let mut failed_outcome = RuntimeCleanupOutcome::default();
        failed_outcome.record("TUN 设备", CleanupStatus::Failed("删除失败".into()));
        let runtime_failure = StopSummary {
            stopped: vec!["alpha".into()],
            cleanup: RuntimeCleanupReport {
                runtime_outcomes: BTreeMap::from([("alpha".into(), failed_outcome)]),
                ..RuntimeCleanupReport::default()
            },
            ..StopSummary::default()
        };
        assert!(!runtime_failure.is_success());
    }

    #[test]
    fn stop_message_lists_every_cleanup_issue() {
        let mut outcome = RuntimeCleanupOutcome::default();
        outcome.record("TUN 设备", CleanupStatus::Failed("TUN 删除失败".into()));
        let summary = StopSummary {
            stopped: vec!["alpha".into()],
            cleanup: RuntimeCleanupReport {
                sessions_remaining: 1,
                listeners_remaining: 2,
                peers_remaining: 3,
                embedded_services_remaining: 4,
                runtime_outcomes: BTreeMap::from([("alpha".into(), outcome)]),
                residual_networks: vec!["alpha".into()],
                ..RuntimeCleanupReport::default()
            },
            ..StopSummary::default()
        };

        let message = summary.message();
        assert!(!summary.is_success());
        assert!(message.contains("残留网络: alpha"));
        assert!(message.contains("运行时任务残留 1 个"));
        assert!(message.contains("监听器残留 2 个"));
        assert!(message.contains("Peer 记录残留 3 个"));
        assert!(message.contains("内嵌服务残留 4 个"));
        assert!(message.contains("TUN 删除失败"));
    }

    #[tokio::test]
    async fn restart_retains_running_embedded_services_without_blocking() {
        let harness = TestHarness::new("restart-service").await;
        harness
            .runtime
            .create(create_command("service-network", true))
            .await
            .expect("create connected network with embedded service");
        let before = harness.services.list().await.expect("list services");
        assert_eq!(before.len(), 1);
        assert_eq!(
            harness.state.snapshot().await.embedded_services[0].status,
            NodeEmbeddedServiceStatus::Running
        );

        let summary = harness
            .runtime
            .restart(vec!["service-network".into()], false)
            .await
            .expect("restart connected network");
        assert_eq!(summary.restarted, vec!["service-network"]);
        assert!(summary.failed.is_empty());
        assert_eq!(summary.cleanup.embedded_services_remaining, 1);
        assert!(summary.cleanup.residual_networks.is_empty());
        let after = harness.services.list().await.expect("list services");
        assert_eq!(after.len(), 1);
        assert_eq!(after[0].signaling_addr, before[0].signaling_addr);
        assert_eq!(after[0].relay_addr, before[0].relay_addr);
        assert_eq!(after[0].ticket, before[0].ticket);
        assert_eq!(
            harness.state.snapshot().await.embedded_services[0].status,
            NodeEmbeddedServiceStatus::Running
        );
        assert_eq!(harness.fake.shutdown_count(), 1);
        assert_eq!(harness.fake.launches().len(), 2);

        harness.shutdown().await;
    }

    #[test]
    fn local_exit_runtime_stages_project_without_losing_cleanup_failure() {
        let status =
            |stage, owner_exit_enabled, effective, healthy, problem| ipc::LocalExitRuntimeStatus {
                owner_exit_enabled,
                network_allowed: true,
                effective,
                healthy,
                stage,
                problem,
                active_clients: 2,
                total_clients: 5,
            };

        let draining = local_exit_capability_from_statuses(
            false,
            &[status(
                ipc::LocalExitRuntimeStage::Draining,
                false,
                false,
                false,
                None,
            )],
            None,
        );
        assert_eq!(draining.stage, LocalExitCapabilityStage::Draining);
        assert_eq!(draining.active_clients, 2);
        assert_eq!(draining.total_clients, 5);

        let cleaning = local_exit_capability_from_statuses(
            false,
            &[status(
                ipc::LocalExitRuntimeStage::Cleaning,
                false,
                false,
                false,
                None,
            )],
            None,
        );
        assert_eq!(cleaning.stage, LocalExitCapabilityStage::Cleaning);

        let failure = NodeProblem::new(
            NodeProblemCode::Unavailable,
            "Exit Node 排空后的资源清理失败，保留排空栅栏并自动重试",
        )
        .retryable(true);
        let failed = local_exit_capability_from_statuses(
            false,
            &[status(
                ipc::LocalExitRuntimeStage::Failed,
                false,
                false,
                false,
                Some(failure.clone()),
            )],
            None,
        );
        assert_eq!(failed.stage, LocalExitCapabilityStage::Failed);
        assert_eq!(failed.problem, Some(failure));

        let exhausted_problem = runtime_control_problem(&RuntimeControlError::ResourceExhausted(
            "本机文件描述符耗尽".to_string(),
        ));
        assert_eq!(exhausted_problem.code, NodeProblemCode::ResourceExhausted);
        assert_eq!(exhausted_problem.category, NodeErrorCategory::Dependency);
        assert_eq!(
            exhausted_problem.message_key,
            "error.operation_resource_exhausted"
        );
        assert!(exhausted_problem.retryable);
        let exhausted =
            local_exit_capability_from_statuses(true, &[], Some(exhausted_problem.clone()));
        assert_eq!(exhausted.stage, LocalExitCapabilityStage::Failed);
        assert_eq!(exhausted.problem, Some(exhausted_problem));
    }

    #[tokio::test]
    async fn cancelling_start_caller_keeps_operation_gate_until_actor_finishes() {
        let harness = TestHarness::new_with_timeouts(
            "start-cancellation",
            RuntimeTimeouts {
                start: Duration::from_secs(1),
                ..RuntimeTimeouts::default()
            },
        )
        .await;
        harness
            .configs
            .create_network("cancelled", "ws://127.0.0.1:6666", true)
            .expect("create test config");
        let selection = NodeNetworkSelection::Named(vec!["cancelled".into()]);
        let plan = NodeRuntimePlan::load(&harness.configs, selection.clone()).expect("load plan");
        harness.fake.set_launch_delay(Duration::from_millis(200));
        let runtime = harness.runtime.clone();
        let caller = tokio::spawn(async move { runtime.start_plan(plan).await });
        wait_for_launch_attempts(&harness.fake, 1).await;
        caller.abort();
        let _ = caller.await;

        assert!(matches!(
            harness.runtime.start_selection(selection.clone()).await,
            Err(RuntimeControlError::Conflict(_))
        ));
        timeout(Duration::from_secs(2), async {
            loop {
                match harness.runtime.start_selection(selection.clone()).await {
                    Ok(summary) => {
                        assert_eq!(summary.already_running, vec!["cancelled"]);
                        assert!(summary.started.is_empty());
                        break;
                    }
                    Err(RuntimeControlError::Conflict(_)) => {
                        tokio::time::sleep(Duration::from_millis(10)).await;
                    }
                    Err(error) => panic!("unexpected start error: {error}"),
                }
            }
        })
        .await
        .expect("actor did not release the cancelled start operation");

        harness.shutdown().await;
    }
}
