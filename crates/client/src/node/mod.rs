mod authorization;
mod command;
pub mod contract;
mod coordinator_takeover;
mod event;
mod http_adapter;
mod lifecycle;
mod network_interfaces;
mod operation_service;
mod plan;
mod privacy_projection;
mod runtime_control;
mod runtime_handler;
pub(crate) mod runtime_metadata;
mod state;
mod ui_manifest;

pub use authorization::{NetworkCommandScope, NodeCommandContext};

#[cfg(test)]
pub(crate) fn test_owner_context() -> NodeCommandContext {
    NodeCommandContext::local_owner(
        fuckipv6_common::domain::NodeId::from_public_key(b"fuckipv6-command-bus-test-owner"),
        "test",
    )
}
pub use command::{
    validate_network_name, NodeAcknowledgeMembershipPolicyCommand, NodeAdminRecoveryListCommand,
    NodeAdminRecoveryStatusCommand, NodeApplyConfigurationCommand, NodeApplyPolicyCommand,
    NodeApplyTrafficPolicyCommand, NodeBatchReviewJoinRequestsCommand, NodeBlacklistSnapshot,
    NodeChangeMemberLifecycleCommand, NodeCleanupFailureReason, NodeCleanupFailureSnapshot,
    NodeCleanupStepSnapshot, NodeCleanupStepStatus, NodeClientBlacklistCommand, NodeCommand,
    NodeCommandBus, NodeCommandBusError, NodeCommandFuture, NodeCommandHandler, NodeCommandKind,
    NodeCommandOutcome, NodeCommandRequest, NodeCommandResult,
    NodeCoordinatorCandidateStatusCommand, NodeCreateInvitationCommand, NodeCreateNetworkCommand,
    NodeCreatedNetwork, NodeDiagnosticCheck, NodeDiagnosticLayer, NodeDiagnosticRecoveryAction,
    NodeDiagnosticState, NodeDiagnosticsSnapshot, NodeExecuteAdminRecoveryCommand,
    NodeGetInvitationStatusCommand, NodeIdentityRotationSnapshot, NodeJoinNetworkCommand,
    NodeJoinRequestListResult, NodeJoinRequestReviewAck, NodeJoinRequestReviewItem,
    NodeJoinRequestReviewOutcome, NodeLeaveNetworkCommand, NodeListInvitationsCommand,
    NodeListJoinRequestsCommand, NodeMemberRemovalPreviewCommand, NodeNetworkAllocation,
    NodeNetworkAllocationSnapshot, NodeNetworkCreationNextStep, NodeNetworkDiscoveryCommand,
    NodeNetworkJoinPreview, NodeNetworkJoinResult, NodeNetworkLeaveResult,
    NodeNetworkMembersCommand, NodeNetworkProfileCommand, NodePeerProbeSnapshot,
    NodePreviewConfigurationCommand, NodePreviewInvitationCommand, NodePreviewJoinNetworkCommand,
    NodePreviewPolicyCommand, NodePreviewPolicyRollbackCommand, NodePreviewTrafficPolicyCommand,
    NodePreviewTrafficPolicyRollbackCommand, NodeProbePeerCommand,
    NodePublishCoordinatorCandidateDeclarationCommand, NodeRedeemInvitationCommand,
    NodeResidualCleanupResult, NodeResubmitJoinRequestCommand, NodeReviewJoinRequestCommand,
    NodeRevokeAdminRecoveryCommand, NodeRevokeInvitationCommand, NodeRollbackConfigurationCommand,
    NodeRollbackPolicyCommand, NodeRollbackTrafficPolicyCommand, NodeRotateNodeIdentityCommand,
    NodeRuntimeResidueSnapshot, NodeSetLocalExitCapabilityCommand, NodeSetNetworkRoleCommand,
    NodeSetNodeDisplayNameCommand, NodeSubmitAdminRecoveryCommand, NodeSubmitJoinRequestCommand,
    NodeTestTrafficPolicyCommand, NodeUpdateCoordinatorCandidatesCommand,
    NodeUpdateNetworkAllocationCommand, NodeUpdateRoutingCommand, NodeWithdrawJoinRequestCommand,
};
pub use event::{
    NodeEvent, NodeEventBus, NodeEventCursor, NodeEventKind, NodeEventReplay, NodeEventReplayReason,
};
pub use http_adapter::{
    NodeApiCommandResponse, NodeApiErrorResponse, NodeApiEvent, NodeApiManifestResponse,
    NodeApiObject, NodeApiObjectResponse, NodeApiOperationResponse, NodeApiResponseMeta,
    NodeApiSnapshotResponse, NodeCommandHttpRequest, NodeCommandHttpResponse, NodeErrorDetails,
    NodeHttpAdapter, NodeHttpAdapterError, NodeHttpErrorCode, NodeHttpErrorResponse,
    NodeManifestResponse, NodeSnapshotResponse, NodeWsMessage, NodeWsResyncReason, NodeWsSession,
    NodeWsSessionError, NODE_HTTP_API_PREFIX, NODE_HTTP_SCHEMA_VERSION, NODE_HTTP_V1_API_PREFIX,
};
pub use lifecycle::{NodeLifecycle, NodeLifecycleSnapshot, NodePhase};
pub use operation_service::{
    NodeOperationSnapshot, OperationClient, OperationClientError, OperationService,
    OperationSubmitError,
};
pub use plan::{NodeNetworkSelection, NodeNetworkSpec, NodeRuntimePlan};
pub use privacy_projection::{
    project_node_command_result, project_node_event, project_node_snapshot,
    NodeSnapshotProjectionError, ProjectedNetworkMemberDirectory, ProjectedNodeCommandOutcome,
    ProjectedNodeCommandResult, ProjectedNodeEmbeddedServiceSnapshot, ProjectedNodeEvent,
    ProjectedNodeEventKind, ProjectedNodeIdentitySnapshot, ProjectedNodeInterfaceSnapshot,
    ProjectedNodeNetworkJoinResult, ProjectedNodeNetworkLeaveResult, ProjectedNodeNetworkSnapshot,
    ProjectedNodePeerSnapshot, ProjectedNodeSnapshot,
};
pub(crate) use runtime_control::{
    NetworkRuntimeManager, RuntimeControlError, StartSummary, RESTART_TIMEOUT, START_TIMEOUT,
    STOP_TIMEOUT,
};
#[cfg(test)]
pub(crate) use runtime_handler::spawn_command_bus_with_runtime;
pub(crate) use runtime_handler::spawn_command_bus_with_runtime_and_audit;
pub use runtime_handler::{spawn_command_bus, spawn_command_bus_with_services, NodeRuntimeHandler};
pub use state::{
    LocalExitCapabilitySnapshot, LocalExitCapabilityStage, NodeAccessModel,
    NodeCapabilityRegistrySnapshot, NodeCapabilitySnapshot, NodeConfigurationSnapshot,
    NodeControlHealth, NodeEmbeddedServiceSnapshot, NodeEmbeddedServiceStatus, NodeErrorCategory,
    NodeExitSelectionSnapshot, NodeFileCapabilitySnapshot, NodeIdentitySnapshot,
    NodeInterfaceAddress, NodeInterfaceKind, NodeInterfaceOwnership, NodeInterfaceSnapshot,
    NodeInterfaceStatus, NodeLogEntry, NodeLogLevel, NodeNetworkConfigSnapshot,
    NodeNetworkMembershipSnapshot, NodeNetworkRole, NodeNetworkSnapshot, NodeNetworkStatus,
    NodePeerSnapshot, NodePermission, NodePermission as NodeUiPermission, NodePermissionSnapshot,
    NodePrimaryStatus, NodeProblem, NodeProblemCode, NodeProblemDetails, NodeRoutingDefaultAction,
    NodeRoutingMode, NodeRoutingSummarySnapshot, NodeRoutingSummaryStatus,
    NodeRoutingTargetSnapshot, NodeSignedNetworkProfileSnapshot, NodeSnapshot, NodeStateStore,
    NodeTrafficSnapshot, NodeValidationIssue,
};
pub use ui_manifest::{
    audit_for_command, node_ui_manifest, NodeUiAction, NodeUiActionBlockReason, NodeUiActionGroup,
    NodeUiAudit, NodeUiAuditPolicy, NodeUiAvailability, NodeUiBackendStatus, NodeUiConfirmation,
    NodeUiField, NodeUiInputKind, NodeUiManifest, NodeUiPage, NodeUiPageState, NodeUiValidation,
    NODE_UI_ACTION_BLOCK_REASONS, NODE_UI_ACTION_GROUPS, NODE_UI_MANIFEST_SCHEMA_VERSION,
    NODE_UI_PAGE_STATES,
};
