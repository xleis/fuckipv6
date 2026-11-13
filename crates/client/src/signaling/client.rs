use anyhow::{Context, Result};
use bytes::Bytes;
use fuckipv6_common::domain::{
    ContentHash, CoordinatorCandidateConfig, CoordinatorCandidateDeclaration,
    CoordinatorCandidateStatus, CoordinatorElectionProposal, CoordinatorElectionVote,
    CoordinatorTerm, CoordinatorWriteCredential, InvitationCode, InvitationCreateRequest,
    InvitationId, InvitationPreview, InvitationRedeemRequest, InvitationRedemption,
    InvitationState, InvitationStatusView, IssuedInvitation, JoinRequest, JoinRequestId,
    JoinRequestState, MemberLifecycleChange, MemberLifecycleChangeRequest, MemberRemovalPreview,
    MemberRemovalPreviewRequest, NetworkId, NetworkJoinPreview, NetworkJoinPreviewRequest,
    NetworkMemberDirectory, NodeId,
};
use fuckipv6_common::{
    codec, validate_signaling_message_len, Identity, JoinRequestBatchReviewItem,
    JoinRequestReviewResult, NatType, PeerInfo, RelayEndpoint, SignalingMessage, VirtualIps,
    MAX_SIGNALING_MESSAGE_SIZE,
};
use futures::{SinkExt, StreamExt};
use std::net::{IpAddr, Ipv6Addr, SocketAddr};
use std::sync::{Arc, Mutex as StdMutex};
use tokio::sync::{mpsc, oneshot, watch, Mutex, RwLock};
use tokio::task::JoinHandle;
use tokio::time::{sleep, Duration};
use tokio_tungstenite::{
    connect_async_with_config,
    tungstenite::{protocol::WebSocketConfig, Message},
};

const INITIAL_RECONNECT_DELAY: Duration = Duration::from_secs(1);
const MAX_RECONNECT_DELAY: Duration = Duration::from_secs(30);
const CLOSE_TIMEOUT: Duration = Duration::from_secs(2);
const JOIN_REQUEST_RESPONSE_TIMEOUT: Duration = Duration::from_secs(15);
const INVITATION_RESPONSE_TIMEOUT: Duration = Duration::from_secs(15);
const NETWORK_JOIN_PREVIEW_RESPONSE_TIMEOUT: Duration = Duration::from_secs(15);
const COORDINATOR_CANDIDATE_RESPONSE_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug, thiserror::Error)]
pub(crate) enum SignalingCloseError {
    #[error("信令连接任务异常退出: {0}")]
    TaskFailed(#[source] tokio::task::JoinError),
    #[error("信令连接任务未在期限内退出，句柄已保留")]
    TimedOut,
}

impl SignalingCloseError {
    pub(crate) fn is_timed_out(&self) -> bool {
        matches!(self, Self::TimedOut)
    }
}

fn signaling_websocket_config() -> WebSocketConfig {
    WebSocketConfig::default()
        .max_message_size(Some(MAX_SIGNALING_MESSAGE_SIZE))
        .max_frame_size(Some(MAX_SIGNALING_MESSAGE_SIZE))
}

/// 信令客户端
pub struct SignalingClient {
    tx: mpsc::UnboundedSender<SignalingMessage>,
    event_rx: Arc<RwLock<mpsc::UnboundedReceiver<SignalingEvent>>>,
    assigned_ip_rx: watch::Receiver<Option<Ipv6Addr>>,
    assigned_virtual_ips_rx: watch::Receiver<Option<VirtualIps>>,
    relay_endpoint_rx: watch::Receiver<Option<RelayEndpoint>>,
    relay_endpoints_rx: watch::Receiver<Vec<RelayEndpoint>>,
    known_peers_rx: watch::Receiver<Vec<PeerInfo>>,
    shutdown_tx: watch::Sender<bool>,
    task: StdMutex<Option<JoinHandle<()>>>,
    join_requests: Arc<JoinRequestResponseHub>,
    join_request_gate: Arc<Mutex<()>>,
    invitations: Arc<InvitationResponseHub>,
    invitation_gate: Arc<Mutex<()>>,
    network_join_previews: Arc<NetworkJoinPreviewResponseHub>,
    network_join_preview_gate: Arc<Mutex<()>>,
    coordinator_candidates: Arc<CoordinatorCandidateResponseHub>,
    coordinator_candidate_gate: Arc<Mutex<()>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct JoinRequestStatusSnapshot {
    pub request_id: JoinRequestId,
    pub state: JoinRequestState,
    pub revision: u64,
    pub information_request: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum JoinRequestResponse {
    Status(JoinRequestStatusSnapshot),
    List(Vec<JoinRequest>),
    MemberDirectory(NetworkMemberDirectory),
    Review {
        network_id: NetworkId,
        reviews: Vec<JoinRequestReviewResult>,
    },
    RemovalPreview(MemberRemovalPreview),
    LifecycleChanged(MemberLifecycleChange),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub(crate) enum JoinRequestControlError {
    #[error("审批控制连接已断开")]
    Disconnected,
    #[error("审批控制请求超时")]
    TimedOut,
    #[error("审批控制请求被拒绝")]
    Rejected(u16),
    #[error("审批控制响应无效")]
    InvalidResponse,
    #[error("审批控制连接尚未就绪")]
    Unavailable,
}

impl JoinRequestControlError {
    pub(crate) fn message_key(self) -> &'static str {
        match self {
            Self::Disconnected | Self::Unavailable => "join_request.error.unavailable",
            Self::TimedOut => "join_request.error.timeout",
            Self::Rejected(401) => "join_request.error.unauthorized",
            Self::Rejected(403) => "join_request.error.forbidden",
            Self::Rejected(404) => "join_request.error.not_found",
            Self::Rejected(429) => "join_request.error.rate_limited",
            Self::Rejected(503) => "join_request.error.overloaded",
            Self::Rejected(_) | Self::InvalidResponse => "join_request.error.unavailable",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum PendingJoinRequest {
    Status(JoinRequestId),
    List(NetworkId),
    MemberDirectory(NetworkId),
    Review {
        network_id: NetworkId,
        request_ids: Vec<JoinRequestId>,
    },
    RemovalPreview(MemberRemovalPreviewRequest),
    LifecycleChange(MemberLifecycleChangeRequest),
}

struct PendingJoinRequestResponse {
    expected: PendingJoinRequest,
    sender: oneshot::Sender<std::result::Result<JoinRequestResponse, JoinRequestControlError>>,
}

#[derive(Default)]
struct JoinRequestResponseHub {
    pending: StdMutex<Option<PendingJoinRequestResponse>>,
}

impl JoinRequestResponseHub {
    fn register(
        &self,
        expected: PendingJoinRequest,
    ) -> std::result::Result<
        oneshot::Receiver<std::result::Result<JoinRequestResponse, JoinRequestControlError>>,
        JoinRequestControlError,
    > {
        let (sender, receiver) = oneshot::channel();
        let mut pending = self
            .pending
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if pending.is_some() {
            return Err(JoinRequestControlError::Unavailable);
        }
        *pending = Some(PendingJoinRequestResponse { expected, sender });
        Ok(receiver)
    }

    /// Route an approval response only when it belongs to the active request.
    /// Unknown approval responses are dropped rather than published through the
    /// general event stream, which may contain applicant prose.
    fn route(&self, message: &SignalingMessage) -> bool {
        let matches = match message {
            SignalingMessage::Error { .. } => self.has_pending(),
            SignalingMessage::JoinRequestStatus { request_id, .. } => self.matches(
                |expected| matches!(expected, PendingJoinRequest::Status(id) if id == request_id),
            ),
            SignalingMessage::JoinRequestListResponse { .. } => {
                self.matches(|expected| matches!(expected, PendingJoinRequest::List(_)))
            }
            SignalingMessage::MemberDirectoryListAck { directory } => self.matches(|expected| {
                matches!(
                    expected,
                    PendingJoinRequest::MemberDirectory(network_id)
                        if *network_id == directory.network_id
                )
            }),
            SignalingMessage::JoinRequestReviewAck {
                network_id,
                reviews,
            } => self.matches(|expected| {
                matches!(
                    expected,
                    PendingJoinRequest::Review {
                        network_id: expected_network,
                        request_ids,
                    } if expected_network == network_id
                        && reviews.len() == request_ids.len()
                        && reviews.iter().all(|review| request_ids.contains(&review.request_id))
                )
            }),
            SignalingMessage::MemberRemovalPreviewAck { preview } => self.matches(|expected| {
                matches!(
                    expected,
                    PendingJoinRequest::RemovalPreview(request)
                        if request.network_id == preview.network_id
                            && request.target_node_id == preview.target_node_id
                )
            }),
            SignalingMessage::MemberLifecycleChangeAck { change } => self.matches(|expected| {
                matches!(
                    expected,
                    PendingJoinRequest::LifecycleChange(request)
                        if request.network_id == change.network_id
                            && request.target_node_id == change.target_node_id
                            && request.action == change.action
                )
            }),
            _ => return false,
        };

        let is_approval_response = matches!(
            message,
            SignalingMessage::JoinRequestStatus { .. }
                | SignalingMessage::JoinRequestListResponse { .. }
                | SignalingMessage::MemberDirectoryListAck { .. }
                | SignalingMessage::JoinRequestReviewAck { .. }
                | SignalingMessage::MemberRemovalPreviewAck { .. }
                | SignalingMessage::MemberLifecycleChangeAck { .. }
        );
        if !matches {
            return is_approval_response;
        }

        let response = match message {
            SignalingMessage::JoinRequestStatus {
                request_id,
                state,
                revision,
                information_request,
            } => JoinRequestResponse::Status(JoinRequestStatusSnapshot {
                request_id: request_id.clone(),
                state: *state,
                revision: *revision,
                information_request: information_request.clone(),
            }),
            SignalingMessage::JoinRequestListResponse { requests } => {
                JoinRequestResponse::List(requests.clone())
            }
            SignalingMessage::MemberDirectoryListAck { directory } => {
                JoinRequestResponse::MemberDirectory(directory.clone())
            }
            SignalingMessage::JoinRequestReviewAck {
                network_id,
                reviews,
            } => JoinRequestResponse::Review {
                network_id: network_id.clone(),
                reviews: reviews.clone(),
            },
            SignalingMessage::MemberRemovalPreviewAck { preview } => {
                JoinRequestResponse::RemovalPreview(preview.clone())
            }
            SignalingMessage::MemberLifecycleChangeAck { change } => {
                JoinRequestResponse::LifecycleChanged(change.clone())
            }
            SignalingMessage::Error { code, .. } => {
                return self.finish(Err(JoinRequestControlError::Rejected(*code)));
            }
            _ => unreachable!("approval response match was checked above"),
        };
        self.finish(Ok(response))
    }

    fn has_pending(&self) -> bool {
        self.pending
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_some()
    }

    fn matches(&self, predicate: impl FnOnce(&PendingJoinRequest) -> bool) -> bool {
        self.pending
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_ref()
            .is_some_and(|pending| predicate(&pending.expected))
    }

    fn finish(
        &self,
        result: std::result::Result<JoinRequestResponse, JoinRequestControlError>,
    ) -> bool {
        let pending = self
            .pending
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        if let Some(pending) = pending {
            let _ = pending.sender.send(result);
        }
        true
    }

    fn fail_pending(&self, error: JoinRequestControlError) {
        let _ = self.finish(Err(error));
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub(crate) enum CoordinatorCandidateControlError {
    #[error("Coordinator 候选控制连接已断开")]
    Disconnected,
    #[error("Coordinator 候选控制请求超时")]
    TimedOut,
    #[error("Coordinator 候选控制请求被拒绝")]
    Rejected(u16),
    #[error("Coordinator 候选控制连接尚未就绪")]
    Unavailable,
}

impl CoordinatorCandidateControlError {
    pub(crate) fn message_key(self) -> &'static str {
        match self {
            Self::Disconnected | Self::Unavailable => "coordinator_candidate.error.unavailable",
            Self::TimedOut => "coordinator_candidate.error.timeout",
            Self::Rejected(401) => "coordinator_candidate.error.unauthorized",
            Self::Rejected(403) => "coordinator_candidate.error.forbidden",
            Self::Rejected(429) => "coordinator_candidate.error.rate_limited",
            Self::Rejected(503) => "coordinator_candidate.error.overloaded",
            Self::Rejected(_) => "coordinator_candidate.error.unavailable",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)] // The runtime takeover loop consumes these pending variants.
enum PendingCoordinatorCandidate {
    Config {
        network_id: NetworkId,
        revision: u64,
    },
    Declaration {
        network_id: NetworkId,
        config_revision: u64,
    },
    Status(NetworkId),
    TermInstall {
        network_id: NetworkId,
        term: u64,
        fencing_token: u64,
    },
    TermLeaseRenew {
        network_id: NetworkId,
        term: u64,
        fencing_token: u64,
        expires_at_unix_ms: u64,
    },
    ElectionPropose {
        network_id: NetworkId,
        proposal_id: ContentHash,
    },
    ElectionVote {
        network_id: NetworkId,
        proposal_id: ContentHash,
        vote: CoordinatorElectionVote,
    },
    ElectionTimeout {
        network_id: NetworkId,
        proposal_id: ContentHash,
    },
}

struct PendingCoordinatorCandidateResponse {
    expected: PendingCoordinatorCandidate,
    sender: oneshot::Sender<
        std::result::Result<CoordinatorCandidateStatus, CoordinatorCandidateControlError>,
    >,
}

#[derive(Default)]
struct CoordinatorCandidateResponseHub {
    pending: StdMutex<Option<PendingCoordinatorCandidateResponse>>,
}

impl CoordinatorCandidateResponseHub {
    fn register(
        &self,
        expected: PendingCoordinatorCandidate,
    ) -> std::result::Result<
        oneshot::Receiver<
            std::result::Result<CoordinatorCandidateStatus, CoordinatorCandidateControlError>,
        >,
        CoordinatorCandidateControlError,
    > {
        let (sender, receiver) = oneshot::channel();
        let mut pending = self
            .pending
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if pending.is_some() {
            return Err(CoordinatorCandidateControlError::Unavailable);
        }
        *pending = Some(PendingCoordinatorCandidateResponse { expected, sender });
        Ok(receiver)
    }

    fn route(&self, message: &SignalingMessage) -> bool {
        let matches = match message {
            SignalingMessage::Error { .. } => self.has_pending(),
            SignalingMessage::CoordinatorCandidateConfigAck { status } => {
                self.matches(|expected| match expected {
                    PendingCoordinatorCandidate::Config {
                        network_id,
                        revision,
                    } => {
                        status.network_id == *network_id
                            && status
                                .config
                                .as_ref()
                                .is_some_and(|config| config.revision == *revision)
                    }
                    _ => false,
                })
            }
            SignalingMessage::CoordinatorCandidateDeclarationAck { status } => {
                self.matches(|expected| match expected {
                    PendingCoordinatorCandidate::Declaration {
                        network_id,
                        config_revision,
                    } => {
                        status.network_id == *network_id
                            && status
                                .config
                                .as_ref()
                                .is_some_and(|config| config.revision == *config_revision)
                    }
                    _ => false,
                })
            }
            SignalingMessage::CoordinatorCandidateStatusAck { status } => self.matches(
                |expected| matches!(expected, PendingCoordinatorCandidate::Status(network_id) if status.network_id == *network_id),
            ),
            SignalingMessage::CoordinatorTermInstallAck { status } => {
                self.matches(|expected| match expected {
                    PendingCoordinatorCandidate::TermInstall {
                        network_id,
                        term,
                        fencing_token,
                    } => {
                        status.network_id == *network_id
                            && status.latest_term.as_ref().is_some_and(|latest| {
                                latest.term == *term
                                    && latest.fencing_token.0 == *fencing_token
                            })
                    }
                    _ => false,
                })
            }
            SignalingMessage::CoordinatorTermLeaseRenewAck { status } => {
                self.matches(|expected| match expected {
                    PendingCoordinatorCandidate::TermLeaseRenew {
                        network_id,
                        term,
                        fencing_token,
                        expires_at_unix_ms,
                    } => {
                        status.network_id == *network_id
                            && status.latest_term.as_ref().is_some_and(|latest| {
                                latest.term == *term
                                    && latest.fencing_token.0 == *fencing_token
                                    && latest.expires_at_unix_ms == *expires_at_unix_ms
                            })
                    }
                    _ => false,
                })
            }
            SignalingMessage::CoordinatorElectionProposeAck { status } => {
                self.matches(|expected| match expected {
                    PendingCoordinatorCandidate::ElectionPropose {
                        network_id,
                        proposal_id,
                    } => {
                        status.network_id == *network_id
                            && status.latest_election.as_ref().is_some_and(|election| {
                                election.proposal.proposal_id == *proposal_id
                            })
                    }
                    _ => false,
                })
            }
            SignalingMessage::CoordinatorElectionVoteAck { status } => {
                self.matches(|expected| match expected {
                    PendingCoordinatorCandidate::ElectionVote {
                        network_id,
                        proposal_id,
                        vote,
                    } => {
                        status.network_id == *network_id
                            && status.latest_election.as_ref().is_some_and(|election| {
                                election.proposal.proposal_id == *proposal_id
                                    && election.votes.contains(vote)
                            })
                    }
                    _ => false,
                })
            }
            SignalingMessage::CoordinatorElectionTimeoutAck { status } => {
                self.matches(|expected| match expected {
                    PendingCoordinatorCandidate::ElectionTimeout {
                        network_id,
                        proposal_id,
                    } => {
                        status.network_id == *network_id
                            && status.latest_election.as_ref().is_some_and(|election| {
                                election.proposal.proposal_id == *proposal_id
                            })
                    }
                    _ => false,
                })
            }
            _ => return false,
        };

        let is_candidate_response = matches!(
            message,
            SignalingMessage::CoordinatorCandidateConfigAck { .. }
                | SignalingMessage::CoordinatorCandidateDeclarationAck { .. }
                | SignalingMessage::CoordinatorCandidateStatusAck { .. }
                | SignalingMessage::CoordinatorTermInstallAck { .. }
                | SignalingMessage::CoordinatorTermLeaseRenewAck { .. }
                | SignalingMessage::CoordinatorElectionProposeAck { .. }
                | SignalingMessage::CoordinatorElectionVoteAck { .. }
                | SignalingMessage::CoordinatorElectionTimeoutAck { .. }
        );
        if !matches {
            return is_candidate_response;
        }

        let response = match message {
            SignalingMessage::CoordinatorCandidateConfigAck { status }
            | SignalingMessage::CoordinatorCandidateDeclarationAck { status }
            | SignalingMessage::CoordinatorCandidateStatusAck { status }
            | SignalingMessage::CoordinatorTermInstallAck { status }
            | SignalingMessage::CoordinatorTermLeaseRenewAck { status }
            | SignalingMessage::CoordinatorElectionProposeAck { status }
            | SignalingMessage::CoordinatorElectionVoteAck { status }
            | SignalingMessage::CoordinatorElectionTimeoutAck { status } => status.clone(),
            SignalingMessage::Error { code, .. } => {
                return self.finish(Err(CoordinatorCandidateControlError::Rejected(*code)));
            }
            _ => unreachable!("coordinator candidate response match was checked above"),
        };
        self.finish(Ok(response))
    }

    fn has_pending(&self) -> bool {
        self.pending
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_some()
    }

    fn matches(&self, predicate: impl FnOnce(&PendingCoordinatorCandidate) -> bool) -> bool {
        self.pending
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_ref()
            .is_some_and(|pending| predicate(&pending.expected))
    }

    fn finish(
        &self,
        result: std::result::Result<CoordinatorCandidateStatus, CoordinatorCandidateControlError>,
    ) -> bool {
        let pending = self
            .pending
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        if let Some(pending) = pending {
            let _ = pending.sender.send(result);
        }
        true
    }

    fn fail_pending(&self, error: CoordinatorCandidateControlError) {
        let _ = self.finish(Err(error));
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum InvitationControlError {
    Disconnected,
    TimedOut,
    Rejected(u16),
    InvalidResponse,
    Unavailable,
}

impl std::fmt::Display for InvitationControlError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Disconnected => "邀请控制连接已断开",
            Self::TimedOut => "邀请控制请求超时",
            Self::Rejected(401) => "邀请控制请求需要重新认证",
            Self::Rejected(403) => "邀请控制请求无权限",
            Self::Rejected(404) => "邀请不存在或不可见",
            Self::Rejected(409 | 412) => "邀请状态已变化，请刷新后重试",
            Self::Rejected(429) => "邀请控制请求过于频繁",
            Self::Rejected(503) => "协调服务器暂时不可用",
            Self::Rejected(_) => "邀请控制请求被拒绝",
            Self::InvalidResponse => "邀请控制响应无效",
            Self::Unavailable => "邀请控制连接尚未就绪",
        })
    }
}

impl std::error::Error for InvitationControlError {}

impl InvitationControlError {
    pub(crate) fn message_key(self) -> &'static str {
        match self {
            Self::Disconnected | Self::Unavailable => "invitation.error.unavailable",
            Self::TimedOut => "invitation.error.timeout",
            Self::Rejected(401) => "invitation.error.reauthentication_required",
            Self::Rejected(403) => "invitation.error.forbidden",
            Self::Rejected(404) => "invitation.error.not_found",
            Self::Rejected(409 | 412) => "invitation.error.preview_changed",
            Self::Rejected(429) => "invitation.error.rate_limited",
            Self::Rejected(503) => "invitation.error.overloaded",
            Self::Rejected(_) | Self::InvalidResponse => "invitation.error.unavailable",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum PendingInvitation {
    Create(NetworkId),
    List(NetworkId),
    Status {
        network_id: NetworkId,
        invitation_id: InvitationId,
    },
    Revoke {
        network_id: NetworkId,
        invitation_id: InvitationId,
    },
    Preview(NetworkId),
    Redeem {
        network_id: NetworkId,
        invitation_id: InvitationId,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum InvitationResponse {
    Created(IssuedInvitation),
    List(Vec<InvitationStatusView>),
    Status(InvitationStatusView),
    Revoked(InvitationStatusView),
    Preview(Box<InvitationPreview>),
    Redemption(Box<InvitationRedemption>),
}

struct PendingInvitationResponse {
    expected: PendingInvitation,
    sender: oneshot::Sender<std::result::Result<InvitationResponse, InvitationControlError>>,
}

#[derive(Default)]
struct InvitationResponseHub {
    pending: StdMutex<Option<PendingInvitationResponse>>,
}

impl InvitationResponseHub {
    fn register(
        &self,
        expected: PendingInvitation,
    ) -> std::result::Result<
        oneshot::Receiver<std::result::Result<InvitationResponse, InvitationControlError>>,
        InvitationControlError,
    > {
        let (sender, receiver) = oneshot::channel();
        let mut pending = self
            .pending
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if pending.is_some() {
            return Err(InvitationControlError::Unavailable);
        }
        *pending = Some(PendingInvitationResponse { expected, sender });
        Ok(receiver)
    }

    /// Correlate one invitation acknowledgement. All invitation acknowledgements
    /// are removed from the data-plane event stream; `InvitationChanged` remains
    /// available as a redacted projection notification.
    fn route(&self, message: &SignalingMessage) -> bool {
        let matches = match message {
            SignalingMessage::Error { .. } => self.has_pending(),
            SignalingMessage::InvitationCreateAck { issued } => self.matches(|expected| {
                matches!(
                    expected,
                    PendingInvitation::Create(network_id)
                        if network_id == &issued.status.invitation.network_id
                )
            }),
            SignalingMessage::InvitationListAck { .. } => {
                self.matches(|expected| matches!(expected, PendingInvitation::List(_)))
            }
            SignalingMessage::InvitationStatusAck { invitation } => self.matches(|expected| {
                matches!(
                    expected,
                    PendingInvitation::Status {
                        network_id,
                        invitation_id,
                    } if network_id == &invitation.invitation.network_id
                        && invitation_id == &invitation.invitation.invitation_id
                )
            }),
            SignalingMessage::InvitationRevokeAck { invitation } => self.matches(|expected| {
                matches!(
                    expected,
                    PendingInvitation::Revoke {
                        network_id,
                        invitation_id,
                    } if network_id == &invitation.invitation.network_id
                        && invitation_id == &invitation.invitation.invitation_id
                )
            }),
            SignalingMessage::InvitationPreviewAck { preview } => self.matches(|expected| {
                matches!(
                    expected,
                    PendingInvitation::Preview(network_id) if network_id == &preview.network_id
                )
            }),
            SignalingMessage::InvitationRedeemAck { redemption } => self.matches(|expected| {
                matches!(
                    expected,
                    PendingInvitation::Redeem {
                        network_id,
                        invitation_id,
                    } if network_id == &redemption.network_id
                        && invitation_id == &redemption.invitation_id
                )
            }),
            _ => return false,
        };

        let is_invitation_response = matches!(
            message,
            SignalingMessage::InvitationCreateAck { .. }
                | SignalingMessage::InvitationListAck { .. }
                | SignalingMessage::InvitationStatusAck { .. }
                | SignalingMessage::InvitationRevokeAck { .. }
                | SignalingMessage::InvitationPreviewAck { .. }
                | SignalingMessage::InvitationRedeemAck { .. }
        );
        if !matches {
            return is_invitation_response;
        }

        let response = match message {
            SignalingMessage::InvitationCreateAck { issued } => {
                InvitationResponse::Created(issued.clone())
            }
            SignalingMessage::InvitationListAck { invitations } => {
                InvitationResponse::List(invitations.clone())
            }
            SignalingMessage::InvitationStatusAck { invitation } => {
                InvitationResponse::Status(invitation.clone())
            }
            SignalingMessage::InvitationRevokeAck { invitation } => {
                InvitationResponse::Revoked(invitation.clone())
            }
            SignalingMessage::InvitationPreviewAck { preview } => {
                InvitationResponse::Preview(Box::new(preview.clone()))
            }
            SignalingMessage::InvitationRedeemAck { redemption } => {
                InvitationResponse::Redemption(Box::new(redemption.clone()))
            }
            SignalingMessage::Error { code, .. } => {
                return self.finish(Err(InvitationControlError::Rejected(*code)));
            }
            _ => unreachable!("invitation response match was checked above"),
        };
        self.finish(Ok(response))
    }

    fn has_pending(&self) -> bool {
        self.pending
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_some()
    }

    fn matches(&self, predicate: impl FnOnce(&PendingInvitation) -> bool) -> bool {
        self.pending
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_ref()
            .is_some_and(|pending| predicate(&pending.expected))
    }

    fn finish(
        &self,
        result: std::result::Result<InvitationResponse, InvitationControlError>,
    ) -> bool {
        let pending = self
            .pending
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        if let Some(pending) = pending {
            let _ = pending.sender.send(result);
        }
        true
    }

    fn fail_pending(&self, error: InvitationControlError) {
        let _ = self.finish(Err(error));
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub(crate) enum NetworkJoinPreviewControlError {
    #[error("加入预览连接已断开")]
    Disconnected,
    #[error("加入预览请求超时")]
    TimedOut,
    #[error("加入预览请求被拒绝")]
    Rejected(u16),
    #[error("加入预览连接尚未就绪")]
    Unavailable,
}

struct PendingNetworkJoinPreviewResponse {
    expected: NetworkJoinPreviewRequest,
    sender:
        oneshot::Sender<std::result::Result<NetworkJoinPreview, NetworkJoinPreviewControlError>>,
}

#[derive(Default)]
struct NetworkJoinPreviewResponseHub {
    pending: StdMutex<Option<PendingNetworkJoinPreviewResponse>>,
}

impl NetworkJoinPreviewResponseHub {
    fn register(
        &self,
        expected: NetworkJoinPreviewRequest,
    ) -> std::result::Result<
        oneshot::Receiver<std::result::Result<NetworkJoinPreview, NetworkJoinPreviewControlError>>,
        NetworkJoinPreviewControlError,
    > {
        let (sender, receiver) = oneshot::channel();
        let mut pending = self
            .pending
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if pending.is_some() {
            return Err(NetworkJoinPreviewControlError::Unavailable);
        }
        *pending = Some(PendingNetworkJoinPreviewResponse { expected, sender });
        Ok(receiver)
    }

    /// A preview acknowledgement never enters the data-plane event stream.
    /// Forged or unsolicited acknowledgements are dropped.
    fn route(&self, message: &SignalingMessage) -> bool {
        let matches = match message {
            SignalingMessage::Error { .. } => self.has_pending(),
            SignalingMessage::NetworkJoinPreviewAck { preview } => self.matches(|expected| {
                network_join_preview_ack_matches(expected, preview, current_timestamp_millis())
            }),
            _ => return false,
        };
        if !matches {
            return matches!(message, SignalingMessage::NetworkJoinPreviewAck { .. });
        }
        let result = match message {
            SignalingMessage::NetworkJoinPreviewAck { preview } => Ok(preview.clone()),
            SignalingMessage::Error { code, .. } => {
                Err(NetworkJoinPreviewControlError::Rejected(*code))
            }
            _ => unreachable!("network join preview response was checked above"),
        };
        self.finish(result)
    }

    fn has_pending(&self) -> bool {
        self.pending
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_some()
    }

    fn matches(&self, predicate: impl FnOnce(&NetworkJoinPreviewRequest) -> bool) -> bool {
        self.pending
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_ref()
            .is_some_and(|pending| predicate(&pending.expected))
    }

    fn finish(
        &self,
        result: std::result::Result<NetworkJoinPreview, NetworkJoinPreviewControlError>,
    ) -> bool {
        let pending = self
            .pending
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        if let Some(pending) = pending {
            let _ = pending.sender.send(result);
        }
        true
    }

    fn fail_pending(&self, error: NetworkJoinPreviewControlError) {
        let _ = self.finish(Err(error));
    }
}

fn network_join_preview_ack_matches(
    expected: &NetworkJoinPreviewRequest,
    preview: &NetworkJoinPreview,
    now_unix_ms: u64,
) -> bool {
    let expected_network_id = NetworkId::from_public_key(preview.public_key.as_str().as_bytes());
    let Ok(terms_hash) = preview.terms.content_hash() else {
        return false;
    };
    let Ok(preview_digest) = preview.compute_digest() else {
        return false;
    };

    NodeId::from_public_key(&expected.coordinator_public_key) == preview.owner
        && expected.network_name == preview.network_name
        && preview.network_id == expected_network_id
        && preview.terms_hash == terms_hash
        && preview.terms.validate().is_ok()
        && preview.policy_revision == preview.terms.network_policy_revision
        && preview.privacy_policy_revision == preview.terms.privacy_policy_revision
        && preview.preview_digest == preview_digest
        && preview.validate_freshness(now_unix_ms).is_ok()
        && preview.signature_is_valid(&expected.coordinator_public_key)
}

#[derive(Clone)]
struct RegistrationIdentity {
    client_id: String,
    public_key: Vec<u8>,
    private_identity: Arc<Identity>,
}

struct ConnectionConfig {
    url: String,
    registration: RegistrationIdentity,
    p2p_port: u16,
    nat_type: NatType,
    candidate_endpoints: Vec<SocketAddr>,
    initial_reconnect_delay: Duration,
    max_reconnect_delay: Duration,
}

struct ConnectionRuntime<'a> {
    outbound_rx: &'a mut mpsc::UnboundedReceiver<SignalingMessage>,
    pending_outbound: &'a mut Vec<SignalingMessage>,
    channels: ConnectionChannels<'a>,
    shutdown_rx: &'a mut watch::Receiver<bool>,
}

#[derive(Clone, Copy)]
struct ConnectionChannels<'a> {
    event_tx: &'a mpsc::UnboundedSender<SignalingEvent>,
    assigned_ip_tx: &'a watch::Sender<Option<Ipv6Addr>>,
    assigned_virtual_ips_tx: &'a watch::Sender<Option<VirtualIps>>,
    relay_endpoint_tx: &'a watch::Sender<Option<RelayEndpoint>>,
    relay_endpoints_tx: &'a watch::Sender<Vec<RelayEndpoint>>,
    known_peers_tx: &'a watch::Sender<Vec<PeerInfo>>,
    join_requests: &'a JoinRequestResponseHub,
    invitations: &'a InvitationResponseHub,
    network_join_previews: &'a NetworkJoinPreviewResponseHub,
    coordinator_candidates: &'a CoordinatorCandidateResponseHub,
}

struct ConnectionState {
    reconnect_delay: Duration,
    has_connected_once: bool,
    assigned_ips: Option<VirtualIps>,
}

fn normalize_ws_url(url: &str) -> String {
    let trimmed = url.trim();
    if trimmed.contains("://") {
        trimmed.to_string()
    } else if trimmed.contains(':') {
        format!("ws://{}", trimmed)
    } else {
        format!("ws://{}:6666", trimmed)
    }
}

impl SignalingClient {
    /// 连接到协调服务器，并在连接断开后自动重连。
    pub async fn connect(
        url: &str,
        identity: Identity,
        p2p_port: u16,
        nat_type: NatType,
    ) -> Result<Self> {
        Self::connect_with_candidates(url, identity, p2p_port, nat_type, Vec::new()).await
    }

    /// 连接到协调服务器并上报 ICE 候选端点，在连接断开后自动重连。
    pub async fn connect_with_candidates(
        url: &str,
        identity: Identity,
        p2p_port: u16,
        nat_type: NatType,
        candidate_endpoints: Vec<SocketAddr>,
    ) -> Result<Self> {
        let (outbound_tx, outbound_rx) = mpsc::unbounded_channel::<SignalingMessage>();
        let (event_tx, event_rx) = mpsc::unbounded_channel::<SignalingEvent>();
        let (assigned_ip_tx, assigned_ip_rx) = watch::channel(None);
        let (assigned_virtual_ips_tx, assigned_virtual_ips_rx) = watch::channel(None);
        let (relay_endpoint_tx, relay_endpoint_rx) = watch::channel(None);
        let (relay_endpoints_tx, relay_endpoints_rx) = watch::channel(Vec::new());
        let (known_peers_tx, known_peers_rx) = watch::channel(Vec::new());
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let join_requests = Arc::new(JoinRequestResponseHub::default());
        let invitations = Arc::new(InvitationResponseHub::default());
        let network_join_previews = Arc::new(NetworkJoinPreviewResponseHub::default());
        let coordinator_candidates = Arc::new(CoordinatorCandidateResponseHub::default());

        let registration = RegistrationIdentity {
            client_id: identity.client_id().to_string(),
            public_key: identity.public_key(),
            private_identity: Arc::new(identity),
        };

        let connection_config = ConnectionConfig {
            url: normalize_ws_url(url),
            registration,
            p2p_port,
            nat_type,
            candidate_endpoints,
            initial_reconnect_delay: INITIAL_RECONNECT_DELAY,
            max_reconnect_delay: MAX_RECONNECT_DELAY,
        };

        let task = tokio::spawn(connection_manager(
            connection_config,
            outbound_rx,
            event_tx,
            assigned_ip_tx,
            assigned_virtual_ips_tx,
            relay_endpoint_tx,
            relay_endpoints_tx,
            known_peers_tx,
            join_requests.clone(),
            invitations.clone(),
            network_join_previews.clone(),
            coordinator_candidates.clone(),
            shutdown_rx,
        ));

        let client = Self {
            tx: outbound_tx,
            event_rx: Arc::new(RwLock::new(event_rx)),
            assigned_ip_rx,
            assigned_virtual_ips_rx,
            relay_endpoint_rx,
            relay_endpoints_rx,
            known_peers_rx,
            shutdown_tx,
            task: StdMutex::new(Some(task)),
            join_requests,
            join_request_gate: Arc::new(Mutex::new(())),
            invitations,
            invitation_gate: Arc::new(Mutex::new(())),
            network_join_previews,
            network_join_preview_gate: Arc::new(Mutex::new(())),
            coordinator_candidates,
            coordinator_candidate_gate: Arc::new(Mutex::new(())),
        };

        client.wait_for_initial_registration().await?;
        Ok(client)
    }

    /// 获取分配的虚拟IP (IPv6)
    pub async fn assigned_ip(&self) -> Option<Ipv6Addr> {
        *self.assigned_ip_rx.borrow()
    }

    /// 获取分配的双栈虚拟IP
    pub async fn assigned_virtual_ips(&self) -> Option<VirtualIps> {
        self.assigned_virtual_ips_rx.borrow().clone()
    }

    pub async fn relay_endpoint(&self) -> Option<RelayEndpoint> {
        self.relay_endpoint_rx.borrow().clone()
    }

    pub async fn relay_endpoints(&self) -> Vec<RelayEndpoint> {
        self.relay_endpoints_rx.borrow().clone()
    }

    /// 获取信令层已知的在线节点快照。
    pub async fn known_peers(&self) -> Vec<PeerInfo> {
        self.known_peers_rx.borrow().clone()
    }

    /// 发送消息
    pub fn send(&self, msg: SignalingMessage) -> Result<()> {
        self.tx.send(msg)?;
        Ok(())
    }

    /// 发送心跳
    pub fn send_heartbeat(&self) -> Result<()> {
        self.send(SignalingMessage::heartbeat())
    }

    pub(crate) async fn submit_join_request(
        &self,
        request: JoinRequest,
    ) -> std::result::Result<JoinRequestStatusSnapshot, JoinRequestControlError> {
        let request_id = request.request_id.clone();
        match self
            .send_join_request_operation(
                SignalingMessage::JoinRequestSubmit {
                    request: Box::new(request),
                },
                PendingJoinRequest::Status(request_id),
            )
            .await?
        {
            JoinRequestResponse::Status(status) => Ok(status),
            _ => Err(JoinRequestControlError::InvalidResponse),
        }
    }

    pub(crate) async fn list_join_requests(
        &self,
        network_id: NetworkId,
    ) -> std::result::Result<Vec<JoinRequest>, JoinRequestControlError> {
        match self
            .send_join_request_operation(
                SignalingMessage::JoinRequestList {
                    network_id: network_id.clone(),
                },
                PendingJoinRequest::List(network_id),
            )
            .await?
        {
            JoinRequestResponse::List(requests) => Ok(requests),
            _ => Err(JoinRequestControlError::InvalidResponse),
        }
    }

    pub(crate) async fn list_network_members(
        &self,
        network_id: NetworkId,
    ) -> std::result::Result<NetworkMemberDirectory, JoinRequestControlError> {
        match self
            .send_join_request_operation(
                SignalingMessage::MemberDirectoryList {
                    network_id: network_id.clone(),
                },
                PendingJoinRequest::MemberDirectory(network_id),
            )
            .await?
        {
            JoinRequestResponse::MemberDirectory(directory) => Ok(directory),
            _ => Err(JoinRequestControlError::InvalidResponse),
        }
    }

    pub(crate) async fn review_join_requests(
        &self,
        network_id: NetworkId,
        reviews: Vec<JoinRequestBatchReviewItem>,
    ) -> std::result::Result<Vec<JoinRequestReviewResult>, JoinRequestControlError> {
        let request_ids = reviews
            .iter()
            .map(|review| review.request_id.clone())
            .collect::<Vec<_>>();
        let (message, expected) = if reviews.len() == 1 {
            let review = reviews.into_iter().next().expect("one review");
            (
                SignalingMessage::JoinRequestReview {
                    network_id: network_id.clone(),
                    request_id: review.request_id,
                    expected_revision: review.expected_revision,
                    action: review.action,
                    note: review.note,
                },
                PendingJoinRequest::Review {
                    network_id,
                    request_ids,
                },
            )
        } else {
            (
                SignalingMessage::JoinRequestBatchReview {
                    network_id: network_id.clone(),
                    reviews,
                },
                PendingJoinRequest::Review {
                    network_id,
                    request_ids,
                },
            )
        };
        match self.send_join_request_operation(message, expected).await? {
            JoinRequestResponse::Review { reviews, .. } => Ok(reviews),
            _ => Err(JoinRequestControlError::InvalidResponse),
        }
    }

    pub(crate) async fn preview_member_removal(
        &self,
        request: MemberRemovalPreviewRequest,
    ) -> std::result::Result<MemberRemovalPreview, JoinRequestControlError> {
        match self
            .send_join_request_operation(
                SignalingMessage::MemberRemovalPreview {
                    request: request.clone(),
                },
                PendingJoinRequest::RemovalPreview(request),
            )
            .await?
        {
            JoinRequestResponse::RemovalPreview(preview) => Ok(preview),
            _ => Err(JoinRequestControlError::InvalidResponse),
        }
    }

    pub(crate) async fn change_member_lifecycle(
        &self,
        request: MemberLifecycleChangeRequest,
    ) -> std::result::Result<MemberLifecycleChange, JoinRequestControlError> {
        match self
            .send_join_request_operation(
                SignalingMessage::MemberLifecycleChange {
                    request: request.clone(),
                },
                PendingJoinRequest::LifecycleChange(request),
            )
            .await?
        {
            JoinRequestResponse::LifecycleChanged(change) => Ok(change),
            _ => Err(JoinRequestControlError::InvalidResponse),
        }
    }

    pub(crate) async fn withdraw_join_request(
        &self,
        network_id: NetworkId,
        request_id: JoinRequestId,
    ) -> std::result::Result<JoinRequestStatusSnapshot, JoinRequestControlError> {
        match self
            .send_join_request_operation(
                SignalingMessage::JoinRequestWithdraw {
                    network_id,
                    request_id: request_id.clone(),
                },
                PendingJoinRequest::Status(request_id),
            )
            .await?
        {
            JoinRequestResponse::Status(status) => Ok(status),
            _ => Err(JoinRequestControlError::InvalidResponse),
        }
    }

    pub(crate) async fn resubmit_join_request(
        &self,
        network_id: NetworkId,
        request_id: JoinRequestId,
        expected_revision: u64,
        reason: String,
    ) -> std::result::Result<JoinRequestStatusSnapshot, JoinRequestControlError> {
        match self
            .send_join_request_operation(
                SignalingMessage::JoinRequestResubmit {
                    network_id,
                    request_id: request_id.clone(),
                    expected_revision,
                    reason,
                },
                PendingJoinRequest::Status(request_id),
            )
            .await?
        {
            JoinRequestResponse::Status(status) => Ok(status),
            _ => Err(JoinRequestControlError::InvalidResponse),
        }
    }

    async fn send_join_request_operation(
        &self,
        message: SignalingMessage,
        expected: PendingJoinRequest,
    ) -> std::result::Result<JoinRequestResponse, JoinRequestControlError> {
        let _gate = self.join_request_gate.lock().await;
        let receiver = self.join_requests.register(expected)?;
        if self.send(message).is_err() {
            self.join_requests
                .fail_pending(JoinRequestControlError::Disconnected);
            return Err(JoinRequestControlError::Unavailable);
        }
        match tokio::time::timeout(JOIN_REQUEST_RESPONSE_TIMEOUT, receiver).await {
            Ok(Ok(response)) => response,
            Ok(Err(_)) => Err(JoinRequestControlError::Disconnected),
            Err(_) => {
                self.join_requests
                    .fail_pending(JoinRequestControlError::TimedOut);
                Err(JoinRequestControlError::TimedOut)
            }
        }
    }

    pub(crate) async fn coordinator_candidate_status(
        &self,
        network_id: NetworkId,
    ) -> std::result::Result<CoordinatorCandidateStatus, CoordinatorCandidateControlError> {
        self.send_coordinator_candidate_operation(
            SignalingMessage::CoordinatorCandidateStatusRequest {
                network_id: network_id.clone(),
            },
            PendingCoordinatorCandidate::Status(network_id),
        )
        .await
    }

    pub(crate) async fn update_coordinator_candidates(
        &self,
        config: CoordinatorCandidateConfig,
        expected_revision: u64,
    ) -> std::result::Result<CoordinatorCandidateStatus, CoordinatorCandidateControlError> {
        let expected = PendingCoordinatorCandidate::Config {
            network_id: config.network_id.clone(),
            revision: config.revision,
        };
        self.send_coordinator_candidate_operation(
            SignalingMessage::CoordinatorCandidateConfigUpdate {
                expected_revision,
                config,
            },
            expected,
        )
        .await
    }

    pub(crate) async fn update_coordinator_candidates_authorized(
        &self,
        config: CoordinatorCandidateConfig,
        expected_revision: u64,
        credential: CoordinatorWriteCredential,
    ) -> std::result::Result<CoordinatorCandidateStatus, CoordinatorCandidateControlError> {
        let expected = PendingCoordinatorCandidate::Config {
            network_id: config.network_id.clone(),
            revision: config.revision,
        };
        self.send_coordinator_candidate_operation(
            SignalingMessage::CoordinatorCandidateConfigUpdateAuthorized {
                expected_revision,
                config,
                credential,
            },
            expected,
        )
        .await
    }

    pub(crate) async fn install_coordinator_term(
        &self,
        term: CoordinatorTerm,
    ) -> std::result::Result<CoordinatorCandidateStatus, CoordinatorCandidateControlError> {
        let expected = PendingCoordinatorCandidate::TermInstall {
            network_id: term.network_id.clone(),
            term: term.term,
            fencing_token: term.fencing_token.0,
        };
        self.send_coordinator_candidate_operation(
            SignalingMessage::CoordinatorTermInstall { term },
            expected,
        )
        .await
    }

    pub(crate) async fn renew_coordinator_term_lease(
        &self,
        credential: CoordinatorWriteCredential,
        new_expires_at_unix_ms: u64,
    ) -> std::result::Result<CoordinatorCandidateStatus, CoordinatorCandidateControlError> {
        let expected = PendingCoordinatorCandidate::TermLeaseRenew {
            network_id: credential.network_id.clone(),
            term: credential.term,
            fencing_token: credential.fencing_token.0,
            expires_at_unix_ms: new_expires_at_unix_ms,
        };
        self.send_coordinator_candidate_operation(
            SignalingMessage::CoordinatorTermLeaseRenew {
                credential,
                new_expires_at_unix_ms,
            },
            expected,
        )
        .await
    }

    #[allow(dead_code)] // Called by the runtime takeover loop.
    pub(crate) async fn propose_coordinator_election(
        &self,
        proposal: CoordinatorElectionProposal,
    ) -> std::result::Result<CoordinatorCandidateStatus, CoordinatorCandidateControlError> {
        let expected = PendingCoordinatorCandidate::ElectionPropose {
            network_id: proposal.network_id.clone(),
            proposal_id: proposal.proposal_id.clone(),
        };
        self.send_coordinator_candidate_operation(
            SignalingMessage::CoordinatorElectionPropose { proposal },
            expected,
        )
        .await
    }

    #[allow(dead_code)] // Called by the runtime takeover loop.
    pub(crate) async fn vote_coordinator_election(
        &self,
        proposal_id: ContentHash,
        vote: CoordinatorElectionVote,
    ) -> std::result::Result<CoordinatorCandidateStatus, CoordinatorCandidateControlError> {
        let expected = PendingCoordinatorCandidate::ElectionVote {
            network_id: vote.network_id.clone(),
            proposal_id: proposal_id.clone(),
            vote: vote.clone(),
        };
        self.send_coordinator_candidate_operation(
            SignalingMessage::CoordinatorElectionVote { proposal_id, vote },
            expected,
        )
        .await
    }

    #[allow(dead_code)] // Called by the runtime takeover loop.
    pub(crate) async fn timeout_coordinator_election(
        &self,
        network_id: NetworkId,
        proposal_id: ContentHash,
    ) -> std::result::Result<CoordinatorCandidateStatus, CoordinatorCandidateControlError> {
        let expected = PendingCoordinatorCandidate::ElectionTimeout {
            network_id: network_id.clone(),
            proposal_id: proposal_id.clone(),
        };
        self.send_coordinator_candidate_operation(
            SignalingMessage::CoordinatorElectionTimeout {
                network_id,
                proposal_id,
            },
            expected,
        )
        .await
    }

    pub(crate) async fn publish_coordinator_candidate_declaration(
        &self,
        declaration: CoordinatorCandidateDeclaration,
    ) -> std::result::Result<CoordinatorCandidateStatus, CoordinatorCandidateControlError> {
        let expected = PendingCoordinatorCandidate::Declaration {
            network_id: declaration.network_id.clone(),
            config_revision: declaration.candidate_config_revision,
        };
        self.send_coordinator_candidate_operation(
            SignalingMessage::CoordinatorCandidateDeclarationPublish { declaration },
            expected,
        )
        .await
    }

    async fn send_coordinator_candidate_operation(
        &self,
        message: SignalingMessage,
        expected: PendingCoordinatorCandidate,
    ) -> std::result::Result<CoordinatorCandidateStatus, CoordinatorCandidateControlError> {
        let _gate = self.coordinator_candidate_gate.lock().await;
        let receiver = self.coordinator_candidates.register(expected)?;
        if self.send(message).is_err() {
            self.coordinator_candidates
                .fail_pending(CoordinatorCandidateControlError::Disconnected);
            return Err(CoordinatorCandidateControlError::Unavailable);
        }
        match tokio::time::timeout(COORDINATOR_CANDIDATE_RESPONSE_TIMEOUT, receiver).await {
            Ok(Ok(response)) => response,
            Ok(Err(_)) => Err(CoordinatorCandidateControlError::Disconnected),
            Err(_) => {
                self.coordinator_candidates
                    .fail_pending(CoordinatorCandidateControlError::TimedOut);
                Err(CoordinatorCandidateControlError::TimedOut)
            }
        }
    }

    pub(crate) async fn create_invitation(
        &self,
        request: InvitationCreateRequest,
    ) -> std::result::Result<IssuedInvitation, InvitationControlError> {
        let network_id = request.network_id.clone();
        match self
            .send_invitation_operation(
                SignalingMessage::InvitationCreate { request },
                PendingInvitation::Create(network_id),
            )
            .await?
        {
            InvitationResponse::Created(issued) => Ok(issued),
            _ => Err(InvitationControlError::InvalidResponse),
        }
    }

    pub(crate) async fn list_invitations(
        &self,
        network_id: NetworkId,
    ) -> std::result::Result<Vec<InvitationStatusView>, InvitationControlError> {
        match self
            .send_invitation_operation(
                SignalingMessage::InvitationList {
                    network_id: network_id.clone(),
                },
                PendingInvitation::List(network_id),
            )
            .await?
        {
            InvitationResponse::List(invitations) => Ok(invitations),
            _ => Err(InvitationControlError::InvalidResponse),
        }
    }

    pub(crate) async fn get_invitation_status(
        &self,
        network_id: NetworkId,
        invitation_id: InvitationId,
    ) -> std::result::Result<InvitationStatusView, InvitationControlError> {
        match self
            .send_invitation_operation(
                SignalingMessage::InvitationStatus {
                    network_id: network_id.clone(),
                    invitation_id: invitation_id.clone(),
                },
                PendingInvitation::Status {
                    network_id,
                    invitation_id,
                },
            )
            .await?
        {
            InvitationResponse::Status(invitation) => Ok(invitation),
            _ => Err(InvitationControlError::InvalidResponse),
        }
    }

    pub(crate) async fn revoke_invitation(
        &self,
        network_id: NetworkId,
        invitation_id: InvitationId,
        expected_revision: u64,
    ) -> std::result::Result<InvitationStatusView, InvitationControlError> {
        match self
            .send_invitation_operation(
                SignalingMessage::InvitationRevoke {
                    network_id: network_id.clone(),
                    invitation_id: invitation_id.clone(),
                    expected_revision,
                },
                PendingInvitation::Revoke {
                    network_id,
                    invitation_id,
                },
            )
            .await?
        {
            InvitationResponse::Revoked(invitation) => Ok(invitation),
            _ => Err(InvitationControlError::InvalidResponse),
        }
    }

    pub(crate) async fn preview_invitation(
        &self,
        network_id: NetworkId,
        code: InvitationCode,
    ) -> std::result::Result<InvitationPreview, InvitationControlError> {
        match self
            .send_invitation_operation(
                SignalingMessage::InvitationPreview {
                    request: fuckipv6_common::domain::InvitationPreviewRequest {
                        network_id: network_id.clone(),
                        code,
                    },
                },
                PendingInvitation::Preview(network_id),
            )
            .await?
        {
            InvitationResponse::Preview(preview) => Ok(*preview),
            _ => Err(InvitationControlError::InvalidResponse),
        }
    }

    pub(crate) async fn redeem_invitation(
        &self,
        request: InvitationRedeemRequest,
    ) -> std::result::Result<InvitationRedemption, InvitationControlError> {
        let network_id = request.network_id.clone();
        let invitation_id = request.invitation_id.clone();
        match self
            .send_invitation_operation(
                SignalingMessage::InvitationRedeem { request },
                PendingInvitation::Redeem {
                    network_id,
                    invitation_id,
                },
            )
            .await?
        {
            InvitationResponse::Redemption(redemption) => Ok(*redemption),
            _ => Err(InvitationControlError::InvalidResponse),
        }
    }

    pub(crate) async fn preview_network_join(
        &self,
        request: NetworkJoinPreviewRequest,
    ) -> std::result::Result<NetworkJoinPreview, NetworkJoinPreviewControlError> {
        let _gate = self.network_join_preview_gate.lock().await;
        let receiver = self.network_join_previews.register(request.clone())?;
        if self
            .send(SignalingMessage::NetworkJoinPreview { request })
            .is_err()
        {
            self.network_join_previews
                .fail_pending(NetworkJoinPreviewControlError::Disconnected);
            return Err(NetworkJoinPreviewControlError::Unavailable);
        }
        match tokio::time::timeout(NETWORK_JOIN_PREVIEW_RESPONSE_TIMEOUT, receiver).await {
            Ok(Ok(response)) => response,
            Ok(Err(_)) => Err(NetworkJoinPreviewControlError::Disconnected),
            Err(_) => {
                self.network_join_previews
                    .fail_pending(NetworkJoinPreviewControlError::TimedOut);
                Err(NetworkJoinPreviewControlError::TimedOut)
            }
        }
    }

    async fn send_invitation_operation(
        &self,
        message: SignalingMessage,
        expected: PendingInvitation,
    ) -> std::result::Result<InvitationResponse, InvitationControlError> {
        let _gate = self.invitation_gate.lock().await;
        let receiver = self.invitations.register(expected)?;
        if self.send(message).is_err() {
            self.invitations
                .fail_pending(InvitationControlError::Disconnected);
            return Err(InvitationControlError::Unavailable);
        }
        match tokio::time::timeout(INVITATION_RESPONSE_TIMEOUT, receiver).await {
            Ok(Ok(response)) => response,
            Ok(Err(_)) => Err(InvitationControlError::Disconnected),
            Err(_) => {
                self.invitations
                    .fail_pending(InvitationControlError::TimedOut);
                Err(InvitationControlError::TimedOut)
            }
        }
    }

    /// 关闭信令连接，并等待连接任务退出。
    pub async fn close(&self) -> Result<()> {
        self.close_bounded(CLOSE_TIMEOUT).await?;
        Ok(())
    }

    pub(crate) async fn close_bounded(
        &self,
        total_timeout: Duration,
    ) -> std::result::Result<(), SignalingCloseError> {
        tracing::info!("正在关闭信令连接...");
        let _ = self.shutdown_tx.send(true);
        let mut task = {
            let mut slot = self.task.lock().expect("signaling task slot poisoned");
            slot.take()
        };
        let Some(mut task) = task.take() else {
            return Ok(());
        };
        match tokio::time::timeout(total_timeout, &mut task).await {
            Ok(Ok(())) => Ok(()),
            Ok(Err(error)) if error.is_cancelled() => Ok(()),
            Ok(Err(error)) => Err(SignalingCloseError::TaskFailed(error)),
            Err(_) => {
                task.abort();
                *self.task.lock().expect("signaling task slot poisoned") = Some(task);
                Err(SignalingCloseError::TimedOut)
            }
        }
    }

    /// 同步触发信令管理器关闭。Async callers should prefer [`close`](Self::close)
    /// so listener/task teardown can be observed.
    pub fn shutdown(&self) {
        let _ = self.shutdown_tx.send(true);
        if let Some(task) = self
            .task
            .lock()
            .expect("signaling task slot poisoned")
            .take()
        {
            task.abort();
        }
    }

    /// 接收事件
    pub async fn recv_event(&self) -> Option<SignalingEvent> {
        self.event_rx.write().await.recv().await
    }

    async fn wait_for_initial_registration(&self) -> Result<()> {
        let mut assigned_ip_rx = self.assigned_ip_rx.clone();
        tokio::time::timeout(Duration::from_secs(5), async move {
            loop {
                if assigned_ip_rx.borrow().is_some() {
                    return Ok(());
                }

                if assigned_ip_rx.changed().await.is_err() {
                    anyhow::bail!("信令连接管理器已停止");
                }
            }
        })
        .await
        .context("等待虚拟 IP 分配超时")?
    }
}

impl Drop for SignalingClient {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[allow(clippy::too_many_arguments)]
async fn connection_manager(
    config: ConnectionConfig,
    mut outbound_rx: mpsc::UnboundedReceiver<SignalingMessage>,
    event_tx: mpsc::UnboundedSender<SignalingEvent>,
    assigned_ip_tx: watch::Sender<Option<Ipv6Addr>>,
    assigned_virtual_ips_tx: watch::Sender<Option<VirtualIps>>,
    relay_endpoint_tx: watch::Sender<Option<RelayEndpoint>>,
    relay_endpoints_tx: watch::Sender<Vec<RelayEndpoint>>,
    known_peers_tx: watch::Sender<Vec<PeerInfo>>,
    join_requests: Arc<JoinRequestResponseHub>,
    invitations: Arc<InvitationResponseHub>,
    network_join_previews: Arc<NetworkJoinPreviewResponseHub>,
    coordinator_candidates: Arc<CoordinatorCandidateResponseHub>,
    mut shutdown_rx: watch::Receiver<bool>,
) {
    let channels = ConnectionChannels {
        event_tx: &event_tx,
        assigned_ip_tx: &assigned_ip_tx,
        assigned_virtual_ips_tx: &assigned_virtual_ips_tx,
        relay_endpoint_tx: &relay_endpoint_tx,
        relay_endpoints_tx: &relay_endpoints_tx,
        known_peers_tx: &known_peers_tx,
        join_requests: &join_requests,
        invitations: &invitations,
        network_join_previews: &network_join_previews,
        coordinator_candidates: &coordinator_candidates,
    };
    let mut pending_outbound = Vec::new();
    let mut state = ConnectionState {
        reconnect_delay: config.initial_reconnect_delay,
        has_connected_once: false,
        assigned_ips: None,
    };

    loop {
        if *shutdown_rx.borrow() {
            break;
        }

        tracing::info!("正在连接协调服务器: {}", config.url);
        match connect_async_with_config(&config.url, Some(signaling_websocket_config()), true).await
        {
            Ok((ws_stream, _)) => {
                handle_connected_signaling(
                    ws_stream,
                    &config,
                    &mut state,
                    &mut outbound_rx,
                    &mut pending_outbound,
                    channels,
                    &mut shutdown_rx,
                )
                .await;
            }
            Err(e) => {
                tracing::warn!("连接协调服务器失败: {:?}", e);
                notify_disconnected(channels, format!("{:?}", e));
            }
        }

        if *shutdown_rx.borrow() {
            break;
        }

        tokio::select! {
            _ = sleep(state.reconnect_delay) => {}
            _ = shutdown_rx.changed() => {
                if *shutdown_rx.borrow() {
                    break;
                }
            }
        }

        state.reconnect_delay = (state.reconnect_delay * 2).min(config.max_reconnect_delay);
    }
}

async fn handle_connected_signaling<S>(
    ws_stream: tokio_tungstenite::WebSocketStream<S>,
    config: &ConnectionConfig,
    state: &mut ConnectionState,
    outbound_rx: &mut mpsc::UnboundedReceiver<SignalingMessage>,
    pending_outbound: &mut Vec<SignalingMessage>,
    channels: ConnectionChannels<'_>,
    shutdown_rx: &mut watch::Receiver<bool>,
) where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    tracing::info!("WebSocket 信令链路已建立");
    state.reconnect_delay = config.initial_reconnect_delay;

    if state.has_connected_once {
        let _ = channels.event_tx.send(SignalingEvent::Reconnected);
    }
    state.has_connected_once = true;

    let mut runtime = ConnectionRuntime {
        outbound_rx,
        pending_outbound,
        channels,
        shutdown_rx,
    };

    if let Err(e) = run_connection(
        ws_stream,
        &config.registration,
        config.p2p_port,
        config.nat_type,
        &config.candidate_endpoints,
        &mut state.assigned_ips,
        &mut runtime,
    )
    .await
    {
        tracing::warn!("信令通信链路中断: {:?}", e);
        notify_disconnected(channels, format!("{:?}", e));
    }
}

fn notify_disconnected(channels: ConnectionChannels<'_>, reason: String) {
    channels
        .join_requests
        .fail_pending(JoinRequestControlError::Disconnected);
    channels
        .invitations
        .fail_pending(InvitationControlError::Disconnected);
    channels
        .network_join_previews
        .fail_pending(NetworkJoinPreviewControlError::Disconnected);
    channels
        .coordinator_candidates
        .fail_pending(CoordinatorCandidateControlError::Disconnected);
    let _ = channels.assigned_ip_tx.send(None);
    let _ = channels.relay_endpoint_tx.send(None);
    let _ = channels
        .event_tx
        .send(SignalingEvent::Disconnected { reason });
}

async fn run_connection<S>(
    ws_stream: tokio_tungstenite::WebSocketStream<S>,
    registration: &RegistrationIdentity,
    p2p_port: u16,
    nat_type: NatType,
    candidate_endpoints: &[SocketAddr],
    assigned_ips: &mut Option<VirtualIps>,
    runtime: &mut ConnectionRuntime<'_>,
) -> Result<()>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let (mut ws_sender, mut ws_receiver) = ws_stream.split();

    send_signaling_message(
        &mut ws_sender,
        build_register_message(
            registration,
            p2p_port,
            nat_type,
            candidate_endpoints,
            assigned_ips.as_ref(),
        ),
    )
    .await
    .context("failed to send initial signaling registration")?;
    flush_pending(&mut ws_sender, runtime.pending_outbound)
        .await
        .context("failed to flush pending signaling messages")?;

    loop {
        tokio::select! {
            _ = runtime.shutdown_rx.changed() => {
                if *runtime.shutdown_rx.borrow() {
                    let _ = ws_sender.send(Message::Close(None)).await;
                    return Ok(());
                }
            }

            maybe_msg = runtime.outbound_rx.recv() => {
                let Some(msg) = maybe_msg else {
                    return Ok(());
                };

                if let Err(e) = send_signaling_message(&mut ws_sender, msg.clone()).await {
                    runtime.pending_outbound.push(msg);
                    return Err(e);
                }
            }

            maybe_ws_msg = ws_receiver.next() => {
                let Some(ws_msg) = maybe_ws_msg else {
                    anyhow::bail!("coordinator closed signaling stream");
                };

                handle_ws_message(ws_msg, &mut ws_sender, runtime, assigned_ips).await?;
            }
        }
    }
}

async fn handle_ws_message<S>(
    ws_msg: Result<Message, tokio_tungstenite::tungstenite::Error>,
    ws_sender: &mut S,
    runtime: &ConnectionRuntime<'_>,
    assigned_ips: &mut Option<VirtualIps>,
) -> Result<()>
where
    S: SinkExt<Message> + Unpin,
    <S as futures::Sink<Message>>::Error: std::error::Error + Send + Sync + 'static,
{
    match ws_msg {
        Ok(Message::Binary(data)) => {
            validate_signaling_message_len(data.len())
                .context("signaling websocket frame exceeds the configured limit")?;
            let msg = codec::decode::<SignalingMessage>(&data)
                .context("failed to deserialize signaling message")?;
            handle_incoming_message(msg, runtime.channels, assigned_ips)?;
        }
        Ok(Message::Ping(data)) => {
            ws_sender
                .send(Message::Pong(data))
                .await
                .context("failed to send signaling pong")?;
        }
        Ok(Message::Close(_)) => {
            anyhow::bail!("coordinator closed signaling connection");
        }
        Err(e) => {
            return Err(e).context("signaling websocket receive failed");
        }
        _ => {}
    }

    Ok(())
}

async fn flush_pending<S>(
    ws_sender: &mut S,
    pending_outbound: &mut Vec<SignalingMessage>,
) -> Result<()>
where
    S: SinkExt<Message> + Unpin,
    <S as futures::Sink<Message>>::Error: std::error::Error + Send + Sync + 'static,
{
    let pending = std::mem::take(pending_outbound);
    for msg in pending {
        if let Err(e) = send_signaling_message(ws_sender, msg.clone()).await {
            pending_outbound.push(msg);
            return Err(e).context("failed to flush pending signaling message");
        }
    }

    Ok(())
}

async fn send_signaling_message<S>(ws_sender: &mut S, msg: SignalingMessage) -> Result<()>
where
    S: SinkExt<Message> + Unpin,
    <S as futures::Sink<Message>>::Error: std::error::Error + Send + Sync + 'static,
{
    let data = codec::encode(&msg).context("failed to serialize signaling message")?;
    validate_signaling_message_len(data.len())
        .context("serialized signaling message exceeds the configured limit")?;
    ws_sender
        .send(Message::Binary(data.into()))
        .await
        .context("failed to send signaling message")?;
    Ok(())
}

fn handle_incoming_message(
    msg: SignalingMessage,
    channels: ConnectionChannels<'_>,
    assigned_ips_state: &mut Option<VirtualIps>,
) -> Result<()> {
    let registered = if let SignalingMessage::RegisterAck {
        assigned_ip,
        assigned_ips,
        relay_endpoint,
        relay_endpoints,
        ..
    } = &msg
    {
        let ips = assigned_ips
            .clone()
            .unwrap_or_else(|| VirtualIps::from_ipv6(*assigned_ip));
        *assigned_ips_state = Some(ips.clone());
        let _ = channels.relay_endpoint_tx.send(relay_endpoint.clone());
        let all_relays =
            RelayEndpoint::combine_relay_endpoints(relay_endpoint.as_ref(), relay_endpoints);
        let _ = channels.relay_endpoints_tx.send(all_relays);
        tracing::info!("分配的虚拟 IP: 主 IPv6={}, 双栈={:?}", assigned_ip, ips);
        if let Some(endpoint) = relay_endpoint {
            tracing::info!("QUIC 中继服务端点: {}", endpoint.addr);
        }
        Some((*assigned_ip, ips))
    } else {
        None
    };

    update_known_peers(&msg, channels.known_peers_tx);

    if let Some((assigned_ip, ips)) = registered {
        let _ = channels.assigned_virtual_ips_tx.send(Some(ips));
        // `assigned_ip` is the public completion signal used by
        // `wait_for_initial_registration`; publish every other registration
        // field first so callers observe one coherent RegisterAck snapshot.
        let _ = channels.assigned_ip_tx.send(Some(assigned_ip));
    }

    if channels.join_requests.route(&msg) {
        return Ok(());
    }

    if channels.invitations.route(&msg) {
        return Ok(());
    }

    if channels.network_join_previews.route(&msg) {
        return Ok(());
    }

    if channels.coordinator_candidates.route(&msg) {
        return Ok(());
    }

    if let Some(event) = signaling_message_to_event(msg) {
        let _ = channels.event_tx.send(event);
    }

    Ok(())
}

fn update_known_peers(msg: &SignalingMessage, known_peers_tx: &watch::Sender<Vec<PeerInfo>>) {
    match msg {
        SignalingMessage::RegisterAck { peers, .. } => {
            let _ = known_peers_tx.send(peers.clone());
        }
        SignalingMessage::PeerOnline { peer }
        | SignalingMessage::ConnectInfo { peer }
        | SignalingMessage::ConnectNotify { peer } => {
            let mut peers = known_peers_tx.borrow().clone();
            if let Some(existing) = peers
                .iter_mut()
                .find(|known| known.client_id == peer.client_id)
            {
                *existing = peer.clone();
            } else {
                peers.push(peer.clone());
            }
            let _ = known_peers_tx.send(peers);
        }
        SignalingMessage::PeerOffline { client_id } => {
            let mut peers = known_peers_tx.borrow().clone();
            peers.retain(|peer| peer.client_id != *client_id);
            let _ = known_peers_tx.send(peers);
        }
        _ => {}
    }
}

fn build_register_message(
    registration: &RegistrationIdentity,
    p2p_port: u16,
    nat_type: NatType,
    candidate_endpoints: &[SocketAddr],
    requested_ips: Option<&VirtualIps>,
) -> SignalingMessage {
    let timestamp = current_timestamp_secs();
    let challenge = match requested_ips {
        Some(ips) => SignalingMessage::register_challenge_with_ips(
            &registration.client_id,
            &registration.public_key,
            Some(ips),
            p2p_port,
            timestamp,
        ),
        None => SignalingMessage::register_challenge(
            &registration.client_id,
            &registration.public_key,
            p2p_port,
            timestamp,
        ),
    };
    let signature = registration
        .private_identity
        .sign(&challenge)
        .to_bytes()
        .to_vec();

    match requested_ips {
        Some(ips) => SignalingMessage::register_with_ips(
            registration.client_id.clone(),
            registration.public_key.clone(),
            Some(ips.clone()),
            p2p_port,
            nat_type,
            candidate_endpoints.to_vec(),
            timestamp,
            signature,
        ),
        None => SignalingMessage::register_with_candidates(
            registration.client_id.clone(),
            registration.public_key.clone(),
            p2p_port,
            nat_type,
            candidate_endpoints.to_vec(),
            timestamp,
            signature,
        ),
    }
}

fn current_timestamp_secs() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};

    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn current_timestamp_millis() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};

    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

fn signaling_message_to_event(msg: SignalingMessage) -> Option<SignalingEvent> {
    match msg {
        SignalingMessage::RegisterAck {
            assigned_ip,
            assigned_ips,
            network_prefix,
            ipv4_prefix,
            peers,
            relay_endpoint,
            relay_endpoints,
        } => Some(SignalingEvent::Registered {
            assigned_ip,
            assigned_ips: assigned_ips.or_else(|| Some(VirtualIps::from_ipv6(assigned_ip))),
            network_prefix,
            ipv4_prefix,
            peers,
            relay_endpoint,
            relay_endpoints,
        }),

        SignalingMessage::PeerOnline { peer } => Some(SignalingEvent::PeerOnline { peer }),

        SignalingMessage::PeerOffline { client_id } => {
            Some(SignalingEvent::PeerOffline { client_id })
        }

        SignalingMessage::RelayPacket {
            src_ip,
            dst_ip,
            payload,
        } => Some(SignalingEvent::RelayPacket {
            src_ip,
            dst_ip,
            // Preserve the decode allocation's ownership through the event and
            // CLI data-plane handoff. Cloning shares the immutable allocation;
            // only a uniquely owned frame may later be converted in place for AEAD.
            payload,
        }),

        SignalingMessage::SecureControlPacket {
            src_ip,
            dst_ip,
            payload,
        } => Some(SignalingEvent::SecureControlPacket {
            src_ip,
            dst_ip,
            payload,
        }),

        SignalingMessage::ConnectInfo { peer } => Some(SignalingEvent::ConnectInfo { peer }),

        SignalingMessage::ConnectNotify { peer } => Some(SignalingEvent::ConnectNotify { peer }),

        SignalingMessage::HeartbeatAck { timestamp } => {
            Some(SignalingEvent::HeartbeatAck { timestamp })
        }

        SignalingMessage::Error { code, message } => Some(SignalingEvent::Error { code, message }),

        SignalingMessage::JoinRequestStatus {
            request_id,
            state,
            revision,
            information_request,
        } => Some(SignalingEvent::JoinRequestStatus {
            request_id,
            state,
            revision,
            information_request,
        }),

        SignalingMessage::JoinRequestListResponse { requests } => {
            Some(SignalingEvent::JoinRequestListResponse { requests })
        }

        SignalingMessage::JoinRequestReviewAck {
            network_id,
            reviews,
        } => Some(SignalingEvent::JoinRequestReviewAck {
            network_id,
            reviews,
        }),

        SignalingMessage::JoinRequestChanged {
            request_id,
            state,
            revision,
        } => Some(SignalingEvent::JoinRequestChanged {
            request_id,
            state,
            revision,
        }),

        SignalingMessage::InvitationChanged {
            invitation_id,
            state,
            revision,
        } => Some(SignalingEvent::InvitationChanged {
            invitation_id,
            state,
            revision,
        }),

        SignalingMessage::CoordinatorCandidateStatusChanged { network_id } => {
            Some(SignalingEvent::CoordinatorCandidateStatusChanged { network_id })
        }

        SignalingMessage::CoordinatorElectionChanged {
            network_id,
            proposal_id,
            stage,
        } => Some(SignalingEvent::CoordinatorElectionChanged {
            network_id,
            proposal_id,
            stage,
        }),

        SignalingMessage::MemberDirectoryList { .. }
        | SignalingMessage::MemberDirectoryListAck { .. }
        | SignalingMessage::JoinRequestSubmit { .. }
        | SignalingMessage::JoinRequestList { .. }
        | SignalingMessage::JoinRequestReview { .. }
        | SignalingMessage::JoinRequestBatchReview { .. }
        | SignalingMessage::JoinRequestWithdraw { .. }
        | SignalingMessage::JoinRequestResubmit { .. }
        | SignalingMessage::InvitationCreate { .. }
        | SignalingMessage::InvitationCreateAck { .. }
        | SignalingMessage::InvitationList { .. }
        | SignalingMessage::InvitationListAck { .. }
        | SignalingMessage::InvitationStatus { .. }
        | SignalingMessage::InvitationStatusAck { .. }
        | SignalingMessage::InvitationRevoke { .. }
        | SignalingMessage::InvitationRevokeAck { .. }
        | SignalingMessage::InvitationPreview { .. }
        | SignalingMessage::InvitationPreviewAck { .. }
        | SignalingMessage::InvitationRedeem { .. }
        | SignalingMessage::InvitationRedeemAck { .. }
        | SignalingMessage::NetworkJoinPreview { .. }
        | SignalingMessage::NetworkJoinPreviewAck { .. }
        | SignalingMessage::CoordinatorCandidateConfigUpdate { .. }
        | SignalingMessage::CoordinatorCandidateConfigAck { .. }
        | SignalingMessage::CoordinatorCandidateDeclarationPublish { .. }
        | SignalingMessage::CoordinatorCandidateDeclarationAck { .. }
        | SignalingMessage::CoordinatorCandidateStatusRequest { .. }
        | SignalingMessage::CoordinatorCandidateStatusAck { .. }
        | SignalingMessage::CoordinatorCandidateConfigUpdateAuthorized { .. }
        | SignalingMessage::CoordinatorTermInstall { .. }
        | SignalingMessage::CoordinatorTermInstallAck { .. }
        | SignalingMessage::CoordinatorTermLeaseRenew { .. }
        | SignalingMessage::CoordinatorTermLeaseRenewAck { .. }
        | SignalingMessage::CoordinatorElectionPropose { .. }
        | SignalingMessage::CoordinatorElectionProposeAck { .. }
        | SignalingMessage::CoordinatorElectionVote { .. }
        | SignalingMessage::CoordinatorElectionVoteAck { .. }
        | SignalingMessage::CoordinatorElectionTimeout { .. }
        | SignalingMessage::CoordinatorElectionTimeoutAck { .. } => None,

        _ => None,
    }
}

/// 信令事件
#[derive(Debug, Clone)]
pub enum SignalingEvent {
    /// 注册成功
    Registered {
        assigned_ip: Ipv6Addr,
        assigned_ips: Option<VirtualIps>,
        network_prefix: u8,
        ipv4_prefix: Option<u8>,
        peers: Vec<PeerInfo>,
        relay_endpoint: Option<RelayEndpoint>,
        relay_endpoints: Vec<RelayEndpoint>,
    },

    /// 新节点上线
    PeerOnline { peer: PeerInfo },

    /// 节点下线
    PeerOffline { client_id: String },

    /// 通过协调服务器中继收到的数据包 (支持 IPv4 / IPv6)
    RelayPacket {
        src_ip: IpAddr,
        dst_ip: IpAddr,
        payload: Bytes,
    },

    /// A Secure Session v2 handshake control frame relayed by the Coordinator.
    SecureControlPacket {
        src_ip: IpAddr,
        dst_ip: IpAddr,
        payload: Bytes,
    },

    /// 连接信息（用于建立P2P）
    ConnectInfo { peer: PeerInfo },

    /// 连接通知（对方想连接我）
    ConnectNotify { peer: PeerInfo },

    /// 心跳响应
    HeartbeatAck { timestamp: u64 },

    /// 信令连接已断开，会自动重连
    Disconnected { reason: String },

    /// 信令连接已重新建立并已重新发送注册消息
    Reconnected,

    /// Current state of a submitted or withdrawn approval request.
    JoinRequestStatus {
        request_id: JoinRequestId,
        state: JoinRequestState,
        revision: u64,
        information_request: Option<String>,
    },

    /// Authorized approval list. It contains request prose only for the
    /// applicant and current owner/administrators.
    JoinRequestListResponse { requests: Vec<JoinRequest> },

    /// Acknowledgement for one atomic single or batch review.
    JoinRequestReviewAck {
        network_id: NetworkId,
        reviews: Vec<JoinRequestReviewResult>,
    },

    /// Redacted notification suitable for refreshing all related views.
    JoinRequestChanged {
        request_id: JoinRequestId,
        state: JoinRequestState,
        revision: u64,
    },

    /// Redacted invitation projection notification. Plaintext codes and
    /// invitation records are deliberately excluded.
    InvitationChanged {
        invitation_id: InvitationId,
        state: InvitationState,
        revision: u64,
    },

    /// Candidate configuration, declarations or derived health changed. The
    /// event carries only the network scope; consumers must fetch authoritative
    /// status instead of inferring a Leader.
    CoordinatorCandidateStatusChanged { network_id: NetworkId },

    /// Candidate-only election transition. Runtime consumers must fetch
    /// authoritative status before signing a vote or activating Leader duties.
    CoordinatorElectionChanged {
        network_id: NetworkId,
        proposal_id: ContentHash,
        stage: fuckipv6_common::domain::CoordinatorElectionStage,
    },

    /// 错误
    Error { code: u16, message: String },
}

#[cfg(test)]
mod tests {
    use super::*;
    use fuckipv6_common::domain::{
        CoordinatorAuthority, CoordinatorCandidateConfigState, CoordinatorCandidateStatus,
        CoordinatorElectionSession, CoordinatorElectionVote, CoordinatorEmergencyState,
        CoordinatorLeaseState, CoordinatorPhase, CoordinatorTerm, ExitContributionMode,
        ExitContributionObligation, ExitContributionScope, FencingToken, MembershipRole,
        NetworkCreateProfile, NetworkJoinMode, NetworkRecord, NetworkVisibility, NodeId, PublicKey,
    };
    use fuckipv6_common::Identity;
    use futures::SinkExt;
    use std::sync::atomic::{AtomicBool, Ordering};
    use tokio::net::TcpListener;
    use tokio_tungstenite::accept_async;

    fn coordinator_term(term: u64, fencing_token: u64, expires_at_unix_ms: u64) -> CoordinatorTerm {
        let network_id = NetworkId::from_public_key(b"coordinator-ack-network");
        let identity = Identity::generate().unwrap();
        let leader = NodeId::from_public_key(&identity.public_key());
        let voter = identity;
        let vote = CoordinatorElectionVote::signed(
            network_id.clone(),
            1,
            term,
            leader.clone(),
            vec![leader.clone()],
            expires_at_unix_ms.saturating_sub(60_000),
            expires_at_unix_ms,
            &voter,
        )
        .unwrap();
        CoordinatorTerm {
            network_id,
            term,
            leader: leader.clone(),
            candidate_group: vec![leader.clone()],
            candidate_config_revision: 1,
            majority_proof: vec![leader],
            majority_votes: vec![vote],
            authority: CoordinatorAuthority::Full,
            phase: CoordinatorPhase::Stable,
            fencing_token: FencingToken(fencing_token),
            issued_at_unix_ms: expires_at_unix_ms.saturating_sub(60_000),
            expires_at_unix_ms,
        }
    }

    fn coordinator_status(
        term: &CoordinatorTerm,
        generated_at_unix_ms: u64,
    ) -> CoordinatorCandidateStatus {
        CoordinatorCandidateStatus {
            network_id: term.network_id.clone(),
            generated_at_unix_ms,
            config_state: CoordinatorCandidateConfigState::Missing,
            config: None,
            candidates: Vec::new(),
            current_leader: Some(term.leader.clone()),
            latest_term: Some(term.clone()),
            lease: term.lease_status_at(generated_at_unix_ms),
            emergency: CoordinatorEmergencyState::NoCandidatesConfigured,
            latest_election: None,
        }
    }

    fn coordinator_election_material() -> (CoordinatorElectionProposal, CoordinatorElectionVote) {
        let proposer = Identity::generate().unwrap();
        let voter = Identity::generate().unwrap();
        let candidate_group = vec![
            NodeId::from_public_key(&proposer.public_key()),
            NodeId::from_public_key(&voter.public_key()),
        ];
        let proposal = CoordinatorElectionProposal::signed(
            NetworkId::from_public_key(b"coordinator-election-response-network"),
            1,
            2,
            candidate_group.clone(),
            1_000,
            2_000,
            &proposer,
        )
        .unwrap();
        let vote = CoordinatorElectionVote::signed(
            proposal.network_id.clone(),
            proposal.candidate_config_revision,
            proposal.term,
            proposal.leader.clone(),
            candidate_group,
            1_000,
            2_000,
            &voter,
        )
        .unwrap();
        (proposal, vote)
    }

    fn coordinator_election_status(
        proposal: &CoordinatorElectionProposal,
        votes: Vec<CoordinatorElectionVote>,
        generated_at_unix_ms: u64,
    ) -> CoordinatorCandidateStatus {
        let mut session = CoordinatorElectionSession::started(proposal.clone(), 1_100)
            .expect("valid coordinator election session");
        session
            .begin_voting(1_100)
            .expect("coordinator election must enter voting");
        for vote in votes {
            session
                .record_vote(vote, 1_100)
                .expect("vote must belong to the election");
        }
        let mut status = coordinator_status(&coordinator_term(1, 10, 2_000), generated_at_unix_ms);
        status.network_id = proposal.network_id.clone();
        status.latest_election = Some(session);
        status
    }

    #[tokio::test]
    async fn close_timeout_preserves_task_owner_and_retry_succeeds() -> Result<()> {
        let (tx, _) = mpsc::unbounded_channel::<SignalingMessage>();
        let (_, event_rx) = mpsc::unbounded_channel::<SignalingEvent>();
        let (_, assigned_ip_rx) = watch::channel::<Option<Ipv6Addr>>(None);
        let (_, assigned_virtual_ips_rx) = watch::channel::<Option<VirtualIps>>(None);
        let (_, relay_endpoint_rx) = watch::channel::<Option<RelayEndpoint>>(None);
        let (_, relay_endpoints_rx) = watch::channel::<Vec<RelayEndpoint>>(Vec::new());
        let (_, known_peers_rx) = watch::channel::<Vec<PeerInfo>>(Vec::new());
        let (shutdown_tx, _) = watch::channel(false);

        let release = Arc::new(AtomicBool::new(false));
        let blocking_release = release.clone();
        let task = tokio::task::spawn_blocking(move || {
            while !blocking_release.load(Ordering::Acquire) {
                std::thread::sleep(Duration::from_millis(5));
            }
        });
        let client = SignalingClient {
            tx,
            event_rx: Arc::new(RwLock::new(event_rx)),
            assigned_ip_rx,
            assigned_virtual_ips_rx,
            relay_endpoint_rx,
            relay_endpoints_rx,
            known_peers_rx,
            shutdown_tx,
            task: StdMutex::new(Some(task)),
            join_requests: Arc::new(JoinRequestResponseHub::default()),
            join_request_gate: Arc::new(Mutex::new(())),
            invitations: Arc::new(InvitationResponseHub::default()),
            invitation_gate: Arc::new(Mutex::new(())),
            network_join_previews: Arc::new(NetworkJoinPreviewResponseHub::default()),
            network_join_preview_gate: Arc::new(Mutex::new(())),
            coordinator_candidates: Arc::new(CoordinatorCandidateResponseHub::default()),
            coordinator_candidate_gate: Arc::new(Mutex::new(())),
        };

        let error = client
            .close_bounded(Duration::from_millis(100))
            .await
            .expect_err("an uncooperative task must exceed the close deadline");
        assert!(error.is_timed_out());
        assert!(
            client
                .task
                .lock()
                .expect("signaling task slot poisoned")
                .is_some(),
            "a timed-out close must retain the task owner"
        );

        release.store(true, Ordering::Release);
        client.close().await?;
        assert!(client
            .task
            .lock()
            .expect("signaling task slot poisoned")
            .is_none());
        Ok(())
    }

    #[test]
    fn register_message_has_valid_signature() -> Result<()> {
        let identity = Identity::generate()?;
        let registration = RegistrationIdentity {
            client_id: identity.client_id().to_string(),
            public_key: identity.public_key(),
            private_identity: Arc::new(identity),
        };
        let msg = build_register_message(&registration, 51820, NatType::PortRestricted, &[], None);

        let SignalingMessage::Register {
            client_id,
            public_key,
            p2p_port,
            nat_type,
            timestamp,
            signature,
            ..
        } = msg
        else {
            panic!("expected register message");
        };

        let challenge =
            SignalingMessage::register_challenge(&client_id, &public_key, p2p_port, timestamp);
        assert!(Identity::verify(&public_key, &challenge, &signature));
        assert_eq!(nat_type, NatType::PortRestricted);
        Ok(())
    }

    #[test]
    fn relay_packet_event_preserves_ciphertext_allocation() {
        let payload = Bytes::from(vec![0x5a; 1420 + 64]);
        let allocation = payload.as_ptr();
        let event = signaling_message_to_event(SignalingMessage::RelayPacket {
            src_ip: "fd00::1".parse().expect("source IP"),
            dst_ip: "fd00::2".parse().expect("destination IP"),
            payload: payload.clone(),
        })
        .expect("relay packet event");

        let SignalingEvent::RelayPacket {
            payload: event_payload,
            ..
        } = event
        else {
            panic!("expected relay packet event");
        };
        assert_eq!(event_payload.as_ptr(), allocation);
        assert_eq!(event_payload, payload);
    }

    #[tokio::test]
    async fn join_request_responses_are_correlated_and_removed_from_general_events() {
        let hub = JoinRequestResponseHub::default();
        let request_id = JoinRequestId::from_public_key(b"correlated-request");
        let receiver = hub
            .register(PendingJoinRequest::Status(request_id.clone()))
            .expect("register pending approval request");

        assert!(hub.route(&SignalingMessage::JoinRequestStatus {
            request_id: request_id.clone(),
            state: JoinRequestState::Pending,
            revision: 3,
            information_request: Some("private note".into()),
        }));
        let response = receiver.await.expect("response sender").expect("status");
        assert_eq!(
            response,
            JoinRequestResponse::Status(JoinRequestStatusSnapshot {
                request_id,
                state: JoinRequestState::Pending,
                revision: 3,
                information_request: Some("private note".into()),
            })
        );
        assert!(
            hub.route(&SignalingMessage::JoinRequestListResponse {
                requests: Vec::new()
            }),
            "an unmatched list response must be swallowed, not published as a general event"
        );
        assert!(
            !hub.route(&SignalingMessage::protocol_error(
                fuckipv6_common::ErrorCode::Internal,
                "not part of an approval operation",
            )),
            "unrelated signaling errors must remain available to the data plane"
        );
    }

    #[tokio::test]
    async fn invitation_responses_are_correlated_and_unmatched_acks_are_swallowed() {
        let hub = InvitationResponseHub::default();
        let network_id = NetworkId::from_public_key(b"invitation-network");
        let invitation_id = InvitationId::from_public_key(b"invitation-id");
        let mut receiver = hub
            .register(PendingInvitation::Status {
                network_id: network_id.clone(),
                invitation_id: invitation_id.clone(),
            })
            .expect("register pending invitation status");

        let mismatched = InvitationStatusView::new(
            fuckipv6_common::domain::InvitationRecord::new(
                InvitationId::from_public_key(b"other-invitation"),
                network_id.clone(),
                NodeId::from_public_key(b"owner"),
                MembershipRole::Member,
                Vec::new(),
                1,
                1,
                1_000,
            )
            .expect("valid invitation"),
            10,
        );
        assert!(
            hub.route(&SignalingMessage::InvitationStatusAck {
                invitation: mismatched,
            }),
            "an unmatched invitation acknowledgement must not enter the data-plane event stream"
        );
        assert!(
            tokio::time::timeout(Duration::from_millis(10), &mut receiver)
                .await
                .is_err(),
            "an unmatched acknowledgement must not complete the pending operation"
        );

        let matching = InvitationStatusView::new(
            fuckipv6_common::domain::InvitationRecord::new(
                invitation_id,
                network_id,
                NodeId::from_public_key(b"owner"),
                MembershipRole::Member,
                Vec::new(),
                1,
                1,
                1_000,
            )
            .expect("valid invitation"),
            10,
        );
        assert!(hub.route(&SignalingMessage::InvitationStatusAck {
            invitation: matching.clone(),
        }));
        assert_eq!(
            receiver.await.expect("matching response channel"),
            Ok(InvitationResponse::Status(matching))
        );
    }

    #[tokio::test]
    async fn disconnect_fails_the_pending_invitation() {
        let hub = InvitationResponseHub::default();
        let network_id = NetworkId::from_public_key(b"disconnect-invitation-network");
        let receiver = hub
            .register(PendingInvitation::List(network_id))
            .expect("register pending invitation list");
        hub.fail_pending(InvitationControlError::Disconnected);
        assert_eq!(
            receiver
                .await
                .expect("disconnect must not drop the response channel"),
            Err(InvitationControlError::Disconnected)
        );
    }

    #[tokio::test]
    async fn term_install_ack_requires_the_exact_installed_term_and_token() {
        let hub = CoordinatorCandidateResponseHub::default();
        let expected = coordinator_term(2, 20, 1_700_000_060_000);
        let mut receiver = hub
            .register(PendingCoordinatorCandidate::TermInstall {
                network_id: expected.network_id.clone(),
                term: expected.term,
                fencing_token: expected.fencing_token.0,
            })
            .expect("register term install");

        let wrong = coordinator_term(3, 30, 1_700_000_060_000);
        assert!(hub.route(&SignalingMessage::CoordinatorTermInstallAck {
            status: coordinator_status(&wrong, 1_700_000_001_000),
        }));
        assert!(
            tokio::time::timeout(Duration::from_millis(10), &mut receiver)
                .await
                .is_err(),
            "a mismatched term acknowledgement must not complete the pending install"
        );

        let status = coordinator_status(&expected, 1_700_000_001_000);
        assert!(hub.route(&SignalingMessage::CoordinatorTermInstallAck {
            status: status.clone(),
        }));
        assert_eq!(
            receiver.await.expect("matching install response"),
            Ok(status)
        );
    }

    #[tokio::test]
    async fn lease_renewal_ack_requires_the_exact_new_expiry() {
        let hub = CoordinatorCandidateResponseHub::default();
        let expected = coordinator_term(2, 20, 1_700_000_120_000);
        let mut receiver = hub
            .register(PendingCoordinatorCandidate::TermLeaseRenew {
                network_id: expected.network_id.clone(),
                term: expected.term,
                fencing_token: expected.fencing_token.0,
                expires_at_unix_ms: expected.expires_at_unix_ms,
            })
            .expect("register lease renewal");

        let wrong = coordinator_term(2, 20, 1_700_000_180_000);
        assert!(hub.route(&SignalingMessage::CoordinatorTermLeaseRenewAck {
            status: coordinator_status(&wrong, 1_700_000_121_000),
        }));
        assert!(
            tokio::time::timeout(Duration::from_millis(10), &mut receiver)
                .await
                .is_err(),
            "an acknowledgement with another expiry must not complete the renewal"
        );

        let status = coordinator_status(&expected, 1_700_000_061_000);
        assert_eq!(
            status.lease.state,
            CoordinatorLeaseState::Active,
            "test acknowledgement must carry a writable projected lease"
        );
        assert!(hub.route(&SignalingMessage::CoordinatorTermLeaseRenewAck {
            status: status.clone(),
        }));
        assert_eq!(
            receiver.await.expect("matching renewal response"),
            Ok(status)
        );
    }

    #[tokio::test]
    async fn election_proposal_ack_requires_the_exact_proposal() {
        let (proposal, _) = coordinator_election_material();
        let (other_proposal, _) = coordinator_election_material();
        let hub = CoordinatorCandidateResponseHub::default();
        let mut receiver = hub
            .register(PendingCoordinatorCandidate::ElectionPropose {
                network_id: proposal.network_id.clone(),
                proposal_id: proposal.proposal_id.clone(),
            })
            .expect("register election proposal");

        assert!(hub.route(&SignalingMessage::CoordinatorElectionProposeAck {
            status: coordinator_election_status(&other_proposal, Vec::new(), 1_100),
        }));
        assert!(
            tokio::time::timeout(Duration::from_millis(10), &mut receiver)
                .await
                .is_err(),
            "a different election proposal must not complete the pending request"
        );

        let status = coordinator_election_status(&proposal, Vec::new(), 1_100);
        assert!(hub.route(&SignalingMessage::CoordinatorElectionProposeAck {
            status: status.clone(),
        }));
        assert_eq!(
            receiver.await.expect("matching proposal response"),
            Ok(status)
        );
    }

    #[tokio::test]
    async fn election_vote_ack_requires_the_exact_proposal_and_recorded_vote() {
        let (proposal, vote) = coordinator_election_material();
        let hub = CoordinatorCandidateResponseHub::default();
        let mut receiver = hub
            .register(PendingCoordinatorCandidate::ElectionVote {
                network_id: proposal.network_id.clone(),
                proposal_id: proposal.proposal_id.clone(),
                vote: vote.clone(),
            })
            .expect("register election vote");

        assert!(hub.route(&SignalingMessage::CoordinatorElectionVoteAck {
            status: coordinator_election_status(&proposal, Vec::new(), 1_100),
        }));
        assert!(
            tokio::time::timeout(Duration::from_millis(10), &mut receiver)
                .await
                .is_err(),
            "an acknowledgement without the submitted vote must not complete the request"
        );

        let status = coordinator_election_status(&proposal, vec![vote], 1_100);
        assert!(hub.route(&SignalingMessage::CoordinatorElectionVoteAck {
            status: status.clone(),
        }));
        assert_eq!(receiver.await.expect("matching vote response"), Ok(status));
    }

    #[tokio::test]
    async fn election_timeout_ack_requires_the_exact_proposal() {
        let (proposal, _) = coordinator_election_material();
        let (other_proposal, _) = coordinator_election_material();
        let hub = CoordinatorCandidateResponseHub::default();
        let mut receiver = hub
            .register(PendingCoordinatorCandidate::ElectionTimeout {
                network_id: proposal.network_id.clone(),
                proposal_id: proposal.proposal_id.clone(),
            })
            .expect("register election timeout");

        assert!(hub.route(&SignalingMessage::CoordinatorElectionTimeoutAck {
            status: coordinator_election_status(&other_proposal, Vec::new(), 1_100),
        }));
        assert!(
            tokio::time::timeout(Duration::from_millis(10), &mut receiver)
                .await
                .is_err(),
            "a different election timeout must not complete the pending request"
        );

        let status = coordinator_election_status(&proposal, Vec::new(), 1_100);
        assert!(hub.route(&SignalingMessage::CoordinatorElectionTimeoutAck {
            status: status.clone(),
        }));
        assert_eq!(
            receiver.await.expect("matching timeout response"),
            Ok(status)
        );
    }

    #[test]
    fn network_join_preview_ack_requires_matching_signed_fresh_terms() -> Result<()> {
        let coordinator = Identity::generate()?;
        let owner = NodeId::from_public_key(&coordinator.public_key());
        let now = current_timestamp_millis();
        let network = NetworkRecord::create_owned(
            NetworkCreateProfile {
                name: "preview-network".into(),
                purpose: "Exercise signed join preview correlation".into(),
                icon: "home".into(),
                language: "zh-CN".into(),
                region: "HK".into(),
                tags: Vec::new(),
                capability_declarations: Vec::new(),
                visibility: NetworkVisibility::Unlisted,
                join_mode: NetworkJoinMode::InviteOnly,
                exit_contribution: ExitContributionObligation::NotRequired,
                exit_contribution_scope: ExitContributionScope::NotApplicable,
                exit_contribution_mode: ExitContributionMode::Disabled,
                exit_selection_policy: Default::default(),
            },
            PublicKey::from_bytes([51; 32]),
            owner,
            now,
        )?;
        let preview = NetworkJoinPreview::for_record(
            &network,
            &coordinator,
            now,
            now + fuckipv6_common::domain::NETWORK_JOIN_PREVIEW_VALIDITY_MS,
        )?;
        let request = NetworkJoinPreviewRequest {
            coordinator_public_key: coordinator
                .public_key()
                .try_into()
                .map_err(|_| anyhow::anyhow!("coordinator key must contain 32 bytes"))?,
            network_name: network.name.clone(),
        };
        assert!(network_join_preview_ack_matches(&request, &preview, now));

        let mut tampered = preview.clone();
        tampered.network_name = "other-network".into();
        assert!(!network_join_preview_ack_matches(&request, &tampered, now));

        let wrong_signer = Identity::generate()?;
        let wrong_request = NetworkJoinPreviewRequest {
            coordinator_public_key: wrong_signer
                .public_key()
                .try_into()
                .map_err(|_| anyhow::anyhow!("coordinator key must contain 32 bytes"))?,
            network_name: network.name.clone(),
        };
        assert!(!network_join_preview_ack_matches(
            &wrong_request,
            &preview,
            now
        ));
        assert!(!network_join_preview_ack_matches(
            &request,
            &preview,
            now + fuckipv6_common::domain::NETWORK_JOIN_PREVIEW_VALIDITY_MS
        ));
        Ok(())
    }

    #[test]
    fn invitation_notifications_are_redacted_and_never_include_plaintext_codes() {
        let plaintext = format!("fkinv_{}", "A".repeat(43));
        let event = signaling_message_to_event(SignalingMessage::InvitationChanged {
            invitation_id: InvitationId::from_public_key(b"notification"),
            state: InvitationState::Active,
            revision: 1,
        })
        .expect("invitation changed event");
        assert!(!format!("{event:?}").contains(&plaintext));
        assert!(!format!("{event:?}").contains("fkinv_"));
    }

    #[tokio::test]
    async fn disconnect_fails_the_pending_join_request() {
        let hub = JoinRequestResponseHub::default();
        let network_id = NetworkId::from_public_key(b"disconnect-network");
        let receiver = hub
            .register(PendingJoinRequest::List(network_id))
            .expect("register pending list request");
        hub.fail_pending(JoinRequestControlError::Disconnected);
        assert_eq!(
            receiver
                .await
                .expect("disconnect must not drop the response channel"),
            Err(JoinRequestControlError::Disconnected)
        );
    }

    #[tokio::test]
    async fn connection_manager_reregisters_after_reconnect_and_preserves_ip() -> Result<()> {
        let identity = Identity::generate()?;
        let registration = RegistrationIdentity {
            client_id: identity.client_id().to_string(),
            public_key: identity.public_key(),
            private_identity: Arc::new(identity),
        };

        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        let url = format!("ws://{}", addr);

        let (_outbound_tx, outbound_rx) = mpsc::unbounded_channel();
        let (event_tx, mut event_rx) = mpsc::unbounded_channel();
        let (assigned_ip_tx, _assigned_ip_rx) = watch::channel(None);
        let (assigned_virtual_ips_tx, _assigned_virtual_ips_rx) = watch::channel(None);
        let (relay_endpoint_tx, _relay_endpoint_rx) = watch::channel(None);
        let (relay_endpoints_tx, _relay_endpoints_rx) = watch::channel(Vec::new());
        let (known_peers_tx, _known_peers_rx) = watch::channel(Vec::new());
        let (shutdown_tx, shutdown_rx) = watch::channel(false);

        let assigned_ip = Ipv6Addr::new(0xfd00, 0, 0, 0, 0, 0, 0x10, 1);
        let assigned_ips = VirtualIps::from_ipv6(assigned_ip);
        let (close_first_tx, close_first_rx) = tokio::sync::oneshot::channel();
        let (release_second_tx, release_second_rx) = tokio::sync::oneshot::channel();
        let server_task = tokio::spawn(async move {
            serve_register_once(&listener, assigned_ip, None, Some(close_first_rx)).await?;
            serve_register_once(
                &listener,
                assigned_ip,
                Some(assigned_ips),
                Some(release_second_rx),
            )
            .await
        });

        let connection_config = ConnectionConfig {
            url,
            registration,
            p2p_port: 51820,
            nat_type: NatType::FullCone,
            candidate_endpoints: Vec::new(),
            initial_reconnect_delay: Duration::from_millis(20),
            max_reconnect_delay: Duration::from_millis(50),
        };

        let manager_task = tokio::spawn(connection_manager(
            connection_config,
            outbound_rx,
            event_tx,
            assigned_ip_tx,
            assigned_virtual_ips_tx,
            relay_endpoint_tx,
            relay_endpoints_tx,
            known_peers_tx,
            Arc::new(JoinRequestResponseHub::default()),
            Arc::new(InvitationResponseHub::default()),
            Arc::new(NetworkJoinPreviewResponseHub::default()),
            Arc::new(CoordinatorCandidateResponseHub::default()),
            shutdown_rx,
        ));

        let first = tokio::time::timeout(Duration::from_secs(1), event_rx.recv())
            .await?
            .expect("first registration event should arrive");
        assert!(
            matches!(first, SignalingEvent::Registered { .. }),
            "first event was {first:?}"
        );
        close_first_tx
            .send(())
            .expect("test server should still be waiting to close the first connection");

        let mut saw_disconnect = false;
        let mut saw_reconnect = false;
        let mut saw_restored_register = false;

        for _ in 0..5 {
            let event = tokio::time::timeout(Duration::from_secs(1), event_rx.recv())
                .await?
                .expect("reconnect event should arrive");

            match event {
                SignalingEvent::Disconnected { .. } => saw_disconnect = true,
                SignalingEvent::Reconnected => saw_reconnect = true,
                SignalingEvent::Registered { assigned_ip, .. }
                    if assigned_ip == Ipv6Addr::new(0xfd00, 0, 0, 0, 0, 0, 0x10, 1) =>
                {
                    saw_restored_register = true;
                    break;
                }
                _ => {}
            }
        }

        release_second_tx
            .send(())
            .expect("test server should still be waiting after the second registration");
        assert!(saw_disconnect);
        assert!(saw_reconnect);
        assert!(saw_restored_register);

        let _ = shutdown_tx.send(true);
        manager_task.abort();
        server_task.await??;
        Ok(())
    }

    async fn serve_register_once(
        listener: &TcpListener,
        assigned_ip: Ipv6Addr,
        expected_requested_ips: Option<VirtualIps>,
        close_after_ack: Option<tokio::sync::oneshot::Receiver<()>>,
    ) -> Result<()> {
        let (stream, _) = listener.accept().await?;
        let mut ws = accept_async(stream).await?;

        let msg = ws
            .next()
            .await
            .expect("client should send register message")?;
        let Message::Binary(data) = msg else {
            panic!("expected binary register");
        };
        let register = codec::decode::<SignalingMessage>(&data)?;
        let SignalingMessage::Register {
            client_id,
            public_key,
            p2p_port,
            timestamp,
            signature,
            requested_ips,
            ..
        } = register
        else {
            panic!("expected register message");
        };

        assert_eq!(requested_ips, expected_requested_ips);
        let challenge = match requested_ips.as_ref() {
            Some(ips) => SignalingMessage::register_challenge_with_ips(
                &client_id,
                &public_key,
                Some(ips),
                p2p_port,
                timestamp,
            ),
            None => {
                SignalingMessage::register_challenge(&client_id, &public_key, p2p_port, timestamp)
            }
        };
        assert!(Identity::verify(&public_key, &challenge, &signature));

        let ack = SignalingMessage::RegisterAck {
            assigned_ip,
            assigned_ips: Some(VirtualIps::from_ipv6(assigned_ip)),
            network_prefix: 64,
            ipv4_prefix: None,
            peers: Vec::new(),
            relay_endpoint: None,
            relay_endpoints: Vec::new(),
        };
        ws.send(Message::Binary(codec::encode(&ack)?.into()))
            .await?;

        if let Some(close_after_ack) = close_after_ack {
            let _ = close_after_ack.await;
            let _ = ws.close(None).await;
        }
        Ok(())
    }
}
