mod authorization;
mod command;
pub mod contract;
mod event;
mod http_adapter;
mod lifecycle;
mod network_interfaces;
mod operation_service;
mod plan;
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
    validate_network_name, NodeApplyConfigurationCommand, NodeBlacklistSnapshot,
    NodeCleanupFailureReason, NodeCleanupFailureSnapshot, NodeCleanupStepSnapshot,
    NodeCleanupStepStatus, NodeClientBlacklistCommand, NodeCommand, NodeCommandBus,
    NodeCommandBusError, NodeCommandFuture, NodeCommandHandler, NodeCommandKind,
    NodeCommandOutcome, NodeCommandRequest, NodeCommandResult, NodeCreateNetworkCommand,
    NodeCreatedNetwork, NodeDiagnosticsSnapshot, NodeJoinNetworkCommand, NodeNetworkAllocation,
    NodeNetworkAllocationSnapshot, NodePeerProbeSnapshot, NodePreviewConfigurationCommand,
    NodeProbePeerCommand, NodeResidualCleanupResult, NodeRollbackConfigurationCommand,
    NodeRuntimeResidueSnapshot, NodeSetNetworkRoleCommand, NodeUpdateNetworkAllocationCommand,
    NodeUpdateRoutingCommand,
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
pub(crate) use runtime_control::{
    NetworkRuntimeManager, RuntimeControlError, StartSummary, RESTART_TIMEOUT, START_TIMEOUT,
    STOP_TIMEOUT,
};
pub(crate) use runtime_handler::spawn_command_bus_with_runtime;
pub use runtime_handler::{spawn_command_bus, spawn_command_bus_with_services, NodeRuntimeHandler};
pub use state::{
    NodeAccessModel, NodeCapabilityRegistrySnapshot, NodeCapabilitySnapshot,
    NodeConfigurationSnapshot, NodeControlHealth, NodeEmbeddedServiceSnapshot,
    NodeEmbeddedServiceStatus, NodeErrorCategory, NodeIdentitySnapshot, NodeInterfaceAddress,
    NodeInterfaceKind, NodeInterfaceOwnership, NodeInterfaceSnapshot, NodeInterfaceStatus,
    NodeLogEntry, NodeLogLevel, NodeNetworkConfigSnapshot, NodeNetworkRole, NodeNetworkSnapshot,
    NodeNetworkStatus, NodePeerSnapshot, NodePermission, NodePermission as NodeUiPermission,
    NodePermissionSnapshot, NodePrimaryStatus, NodeProblem, NodeProblemCode, NodeProblemDetails,
    NodeRoutingMode, NodeSnapshot, NodeStateStore, NodeTrafficSnapshot, NodeValidationIssue,
};
pub use ui_manifest::{
    audit_for_command, node_ui_manifest, NodeUiAction, NodeUiAudit, NodeUiAuditPolicy,
    NodeUiAvailability, NodeUiBackendStatus, NodeUiConfirmation, NodeUiField, NodeUiInputKind,
    NodeUiManifest, NodeUiPage, NodeUiValidation, NODE_UI_MANIFEST_SCHEMA_VERSION,
};
