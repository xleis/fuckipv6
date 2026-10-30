mod command;
mod event;
mod lifecycle;
mod plan;
mod runtime_handler;
pub(crate) mod runtime_metadata;
mod state;
mod ui_manifest;

pub use command::{
    validate_network_name, NodeCommand, NodeCommandBus, NodeCommandBusError, NodeCommandFuture,
    NodeCommandHandler, NodeCommandOutcome, NodeCommandRequest, NodeCommandResult,
    NodeCreateNetworkCommand, NodeJoinNetworkCommand, NodePeerProbeSnapshot, NodeProbePeerCommand,
    NodeRoutingMode, NodeSetNetworkRoleCommand, NodeUpdateRoutingCommand,
};
pub use event::{NodeEvent, NodeEventBus, NodeEventKind};
pub use lifecycle::{NodeLifecycle, NodeLifecycleSnapshot, NodePhase};
pub use plan::{NodeNetworkSelection, NodeNetworkSpec, NodeRuntimePlan};
pub use runtime_handler::{spawn_command_bus, NodeRuntimeHandler};
pub use state::{
    NodeIdentitySnapshot, NodeLogEntry, NodeLogLevel, NodeNetworkRole, NodeNetworkSnapshot,
    NodeNetworkStatus, NodePeerSnapshot, NodeProblem, NodeProblemCode, NodeSnapshot,
    NodeStateStore, NodeTrafficSnapshot,
};
pub use ui_manifest::{
    node_ui_manifest, NodeUiAction, NodeUiAvailability, NodeUiBackendStatus, NodeUiCommandKind,
    NodeUiConfirmation, NodeUiField, NodeUiInputKind, NodeUiManifest, NodeUiPage, NodeUiPermission,
    NodeUiValidation, NODE_UI_MANIFEST_SCHEMA_VERSION,
};
