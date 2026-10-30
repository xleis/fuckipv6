mod command;
mod event;
mod http_adapter;
mod lifecycle;
mod plan;
mod runtime_control;
mod runtime_handler;
pub(crate) mod runtime_metadata;
mod state;
mod ui_manifest;

pub use command::{
    validate_network_name, NodeCommand, NodeCommandBus, NodeCommandBusError, NodeCommandFuture,
    NodeCommandHandler, NodeCommandOutcome, NodeCommandRequest, NodeCommandResult,
    NodeCreateNetworkCommand, NodeJoinNetworkCommand, NodePeerProbeSnapshot, NodeProbePeerCommand,
    NodeSetNetworkRoleCommand, NodeUpdateRoutingCommand,
};
pub use event::{NodeEvent, NodeEventBus, NodeEventKind};
pub use http_adapter::{
    NodeHttpAdapter, NodeHttpAdapterError, NodeHttpErrorCode, NodeHttpErrorResponse,
    NodeManifestResponse, NodeSnapshotResponse, NodeWsMessage, NodeWsResyncReason, NodeWsSession,
    NodeWsSessionError, NODE_HTTP_API_PREFIX, NODE_HTTP_SCHEMA_VERSION,
};
pub use lifecycle::{NodeLifecycle, NodeLifecycleSnapshot, NodePhase};
pub use plan::{NodeNetworkSelection, NodeNetworkSpec, NodeRuntimePlan};
pub(crate) use runtime_control::{NetworkRuntimeManager, RuntimeControlError, StartSummary};
pub(crate) use runtime_handler::spawn_command_bus_with_runtime;
pub use runtime_handler::{spawn_command_bus, spawn_command_bus_with_services, NodeRuntimeHandler};
pub use state::{
    NodeAccessModel, NodeConfigurationSnapshot, NodeEmbeddedServiceSnapshot,
    NodeEmbeddedServiceStatus, NodeIdentitySnapshot, NodeLogEntry, NodeLogLevel,
    NodeNetworkConfigSnapshot, NodeNetworkRole, NodeNetworkSnapshot, NodeNetworkStatus,
    NodePeerSnapshot, NodePermission, NodePermission as NodeUiPermission, NodePermissionSnapshot,
    NodeProblem, NodeProblemCode, NodeRoutingMode, NodeSnapshot, NodeStateStore,
    NodeTrafficSnapshot,
};
pub use ui_manifest::{
    node_ui_manifest, NodeUiAction, NodeUiAvailability, NodeUiBackendStatus, NodeUiCommandKind,
    NodeUiConfirmation, NodeUiField, NodeUiInputKind, NodeUiManifest, NodeUiPage, NodeUiValidation,
    NODE_UI_MANIFEST_SCHEMA_VERSION,
};
