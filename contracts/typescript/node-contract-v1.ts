// Generated from contracts/schema/*.schema.json. Do not edit by hand.
// Regenerate with scripts/generate-contracts.sh.

export const NODE_CONTRACT_SCHEMA_VERSION = 1 as const;
export const NODE_COMMAND_KINDS = ["apply_configuration", "ban_client", "cleanup_node_residuals", "create_network", "get_blacklist", "get_configuration", "get_configuration_history", "get_network_allocation", "get_node_diagnostics", "get_snapshot", "get_ui_manifest", "join_network", "preview_configuration", "probe_peer", "restart_networks", "restart_node", "rollback_configuration", "set_network_role", "shutdown", "start_networks", "start_node", "stop_networks", "stop_node", "unban_client", "update_network_allocation", "update_routing"] as const;
export type NodeCommandKindWire = (typeof NODE_COMMAND_KINDS)[number];
export const NODE_MANIFEST_ACTION_COMMAND_KINDS = ["apply_configuration", "ban_client", "cleanup_node_residuals", "create_network", "get_blacklist", "get_configuration", "get_configuration_history", "get_network_allocation", "get_node_diagnostics", "get_snapshot", "join_network", "preview_configuration", "probe_peer", "restart_networks", "restart_node", "rollback_configuration", "set_network_role", "shutdown", "start_networks", "start_node", "stop_networks", "stop_node", "unban_client", "update_network_allocation", "update_routing"] as const;
export type NodeManifestActionCommandKind =
  (typeof NODE_MANIFEST_ACTION_COMMAND_KINDS)[number];

export type AclAction = "allow" | "deny";

export type ActivityEntry = {
  "action": string;
  "actor": NodeId;
  "audit_id": AuditId;
  "details": Record<string, string>;
  "network_id"?: NetworkId | null;
  "occurred_at_unix_ms": number;
  "operation_id"?: OperationId | null;
  "outcome": AuditOutcome;
  "resource": string;
  "summary": string;
};

export type AuditId = string;

export type AuditOutcome = "succeeded" | "failed" | "denied";

export type CapabilityKind = "coordinator" | "relay" | "exit" | "file";

export type CapabilityState = "disabled" | "starting" | "enabled" | "degraded" | "unavailable" | "unsupported" | "unknown";

export type ConfigurationApplyResult = {
  "audit"?: ConfigurationHistoryEntry | null;
  "diffs": Array<ConfigurationDiffEntry>;
  "document": ConfigurationDocument;
  "no_op": boolean;
  "previous_revision": number;
  "revision": number;
};

export type ConfigurationChangeAction = "apply" | "rollback";

export type ConfigurationDiffEntry = {
  "after": unknown;
  "before": unknown;
  "impact": ConfigurationImpact;
  "path": string;
};

export type ConfigurationDocument = {
  "networks": Record<string, unknown>;
  "revision": number;
  "schema_version": number;
};

export type ConfigurationHistoryEntry = {
  "action": ConfigurationChangeAction;
  "actor": string;
  "changed_paths": Array<string>;
  "occurred_at": string;
  "previous_revision": number;
  "revision": number;
  "source": string;
  "target_revision"?: number | null;
  "transaction_id": string;
};

export type ConfigurationImpact = "immediate" | "restart_required";

export type ConfigurationPreview = {
  "can_apply": boolean;
  "candidate": ConfigurationDocument;
  "candidate_revision": number;
  "current": ConfigurationDocument;
  "current_revision": number;
  "diffs": Array<ConfigurationDiffEntry>;
  "expected_revision": number;
  "has_changes": boolean;
  "revision_conflict": boolean;
  "validation_errors": Array<ConfigurationValidationIssue>;
};

export type ConfigurationValidationCode = "invalid_type" | "missing_required" | "unknown_network" | "missing_network" | "invalid_value" | "unsupported";

export type ConfigurationValidationIssue = {
  "code": ConfigurationValidationCode;
  "message": string;
  "path": string;
};

export type DisplayEnvironment = "linux_x11" | "linux_wayland" | "windows_desktop" | "macos_desktop" | "headless";

export type DomainActorRef = {
  "display": string;
  "node_id": NodeId;
};

export type DomainObjectRef = {
  "id": string;
  "kind": ObjectKind;
};

export type ExitNodeCapabilities = {
  "cleanup": boolean;
  "ipv4_forwarding": boolean;
  "ipv6_forwarding": boolean;
  "nat": boolean;
  "platform": ExitNodePlatform;
  "schema_version": number;
  "supported": boolean;
  "unsupported_reasons": Array<ExitNodeUnsupportedReason>;
};

export type ExitNodePlatform = "linux" | "windows" | "macos" | "other";

export type ExitNodeUnsupportedReason = "platform_not_linux" | "privileges_required" | "routing_tool_unavailable" | "iptables_unavailable" | "ipv4_forwarding_unavailable";

export type IdempotencyKey = string;

export type NetworkId = string;

export type NetworkMode = "ipv4_only" | "ipv6_only" | "dual_stack";

export type NodeAccessModel = "trusted_control_plane";

export type NodeApiCommandResponse = {
  "operation"?: NodeOperationSnapshot | null;
  "request_id": number;
  "result"?: NodeCommandResult | null;
  "schema_version": number;
  "server_time": string;
};

export type NodeApiErrorResponse = {
  "category": NodeErrorCategory;
  "code": NodeHttpErrorCode;
  "latest_cursor"?: NodeEventCursor | null;
  "message": string;
  "message_key": string;
  "object"?: DomainObjectRef | null;
  "params"?: Record<string, string>;
  "problem"?: NodeProblem | null;
  "recovery"?: OperationRecovery | null;
  "request_id": number;
  "requested_cursor"?: NodeEventCursor | null;
  "requested_schema_version"?: number | null;
  "resync_reason"?: NodeEventReplayReason | null;
  "retryable": boolean;
  "schema_version": number;
  "server_time": string;
  "supported_schema_version"?: number | null;
};

export type NodeApiEvent = {
  "cursor": NodeEventCursor;
  "request_id": number;
  "schema_version": number;
  "server_time": string;
  "snapshot": NodeSnapshot;
  "type": "snapshot";
} | {
  "event": NodeEvent;
  "request_id": number;
  "schema_version": number;
  "server_time": string;
  "type": "event";
} | {
  "latest_cursor": NodeEventCursor;
  "reason": NodeEventReplayReason;
  "request_id": number;
  "requested_cursor": NodeEventCursor;
  "schema_version": number;
  "server_time": string;
  "type": "resync_required";
};

export type NodeApiManifestResponse = {
  "manifest": NodeUiManifest;
  "request_id": number;
  "schema_version": number;
  "server_time": string;
};

export type NodeApiObject = {
  "id": string;
  "identity": NodeIdentitySnapshot;
  "kind": "node";
} | {
  "id": string;
  "kind": "network";
  "network": NodeNetworkSnapshot;
} | {
  "id": string;
  "kind": "peer";
  "peer": NodePeerSnapshot;
} | {
  "id": string;
  "kind": "embedded_service";
  "service": NodeEmbeddedServiceSnapshot;
} | {
  "configuration": NodeNetworkConfigSnapshot;
  "id": string;
  "kind": "network_configuration";
};

export type NodeApiObjectResponse = {
  "object": NodeApiObject;
  "request_id": number;
  "schema_version": number;
  "server_time": string;
};

export type NodeApiOperationResponse = {
  "operation": NodeOperationSnapshot;
  "request_id": number;
  "schema_version": number;
  "server_time": string;
};

export type NodeApiSnapshotResponse = {
  "request_id": number;
  "schema_version": number;
  "server_time": string;
  "snapshot": NodeSnapshot;
};

export type NodeApplyConfigurationCommand = {
  "expected_revision": number;
  "networks": Record<string, unknown>;
};

export type NodeBlacklistSnapshot = {
  "changed": boolean;
  "clients": Array<string>;
  "network_name": string;
};

export type NodeCapabilityRegistrySnapshot = {
  "capabilities": Array<NodeCapabilitySnapshot>;
};

export type NodeCapabilitySnapshot = {
  "kind": CapabilityKind;
  "state": CapabilityState;
};

export type NodeCleanupFailureReason = "still_running" | "owner_unavailable" | "timed_out" | "incomplete" | "unknown";

export type NodeCleanupFailureSnapshot = {
  "network_name": string;
  "reason": NodeCleanupFailureReason;
};

export type NodeCleanupStepSnapshot = {
  "detail"?: string | null;
  "network_name": string;
  "resource": string;
  "status": NodeCleanupStepStatus;
};

export type NodeCleanupStepStatus = "removed" | "not_owned" | "deferred" | "failed" | "unknown";

export type NodeClientBlacklistCommand = {
  "client_id": string;
  "network_name": string;
};

export type NodeCommand = {
  "command": "get_snapshot";
} | {
  "command": "get_ui_manifest";
} | {
  "command": "get_node_diagnostics";
} | {
  "command": "start_networks";
  "payload": {
  "selection": NodeNetworkSelection;
};
} | {
  "command": "stop_networks";
  "payload": {
  "all": boolean;
  "networks": Array<string>;
};
} | {
  "command": "restart_networks";
  "payload": {
  "all": boolean;
  "networks": Array<string>;
};
} | {
  "command": "create_network";
  "payload": NodeCreateNetworkCommand;
} | {
  "command": "join_network";
  "payload": NodeJoinNetworkCommand;
} | {
  "command": "update_routing";
  "payload": NodeUpdateRoutingCommand;
} | {
  "command": "set_network_role";
  "payload": NodeSetNetworkRoleCommand;
} | {
  "command": "get_blacklist";
  "payload": {
  "network_name": string;
};
} | {
  "command": "ban_client";
  "payload": NodeClientBlacklistCommand;
} | {
  "command": "unban_client";
  "payload": NodeClientBlacklistCommand;
} | {
  "command": "get_network_allocation";
  "payload": {
  "network_name": string;
};
} | {
  "command": "update_network_allocation";
  "payload": NodeUpdateNetworkAllocationCommand;
} | {
  "command": "probe_peer";
  "payload": NodeProbePeerCommand;
} | {
  "command": "start_node";
} | {
  "command": "stop_node";
} | {
  "command": "restart_node";
} | {
  "command": "cleanup_node_residuals";
  "payload": {
  "all": boolean;
  "networks": Array<string>;
};
} | {
  "command": "shutdown";
} | {
  "command": "get_configuration";
} | {
  "command": "preview_configuration";
  "payload": NodePreviewConfigurationCommand;
} | {
  "command": "apply_configuration";
  "payload": NodeApplyConfigurationCommand;
} | {
  "command": "rollback_configuration";
  "payload": NodeRollbackConfigurationCommand;
} | {
  "command": "get_configuration_history";
};

export type NodeCommandHttpRequest = {
  "idempotency_key"?: IdempotencyKey | null;
  "request": NodeCommandRequest;
  "schema_version": number;
};

export type NodeCommandHttpResponse = {
  "result": NodeCommandResult;
  "schema_version": number;
};

export type NodeCommandOutcome = {
  "data": NodeSnapshot;
  "status": "snapshot";
} | {
  "data": NodeUiManifest;
  "status": "ui_manifest";
} | {
  "data": NodeDiagnosticsSnapshot;
  "status": "diagnostics";
} | {
  "data": NodeResidualCleanupResult;
  "status": "cleanup";
} | {
  "data": NodeBlacklistSnapshot;
  "status": "blacklist";
} | {
  "data": NodeNetworkAllocationSnapshot;
  "status": "network_allocation";
} | {
  "data": NodePeerProbeSnapshot;
  "status": "probe";
} | {
  "data": NodeCreatedNetwork;
  "status": "network_created";
} | {
  "data": ConfigurationDocument;
  "status": "configuration";
} | {
  "data": ConfigurationPreview;
  "status": "configuration_preview";
} | {
  "data": ConfigurationApplyResult;
  "status": "configuration_applied";
} | {
  "data": Array<ConfigurationHistoryEntry>;
  "status": "configuration_history";
} | {
  "data": {
  "operation_id": string;
};
  "status": "accepted";
} | {
  "data": {
  "message": string;
};
  "status": "completed";
} | {
  "data": NodeProblem;
  "status": "rejected";
};

export type NodeCommandRequest = {
  "command": NodeCommand;
  "request_id": number;
};

export type NodeCommandResult = {
  "outcome": NodeCommandOutcome;
  "request_id": number;
};

export type NodeConfigurationSnapshot = {
  "networks": Array<NodeNetworkConfigSnapshot>;
};

export type NodeControlHealth = "unknown" | "available" | "unavailable";

export type NodeCreateNetworkCommand = {
  "auto_connect": boolean;
  "external_host"?: string | null;
  "name": string;
  "relay_port": number;
  "signaling_port": number;
};

export type NodeCreatedNetwork = {
  "config_saved": boolean;
  "invitation_ticket": string;
  "network_name": string;
  "started": boolean;
};

export type NodeDiagnosticsSnapshot = {
  "cleanup_candidate_networks": Array<string>;
  "lifecycle": NodeLifecycleSnapshot;
  "residue": NodeRuntimeResidueSnapshot;
  "retained_owner_networks": Array<string>;
};

export type NodeEmbeddedServiceSnapshot = {
  "network_name": string;
  "relay_addr"?: string | null;
  "signaling_addr"?: string | null;
  "status": NodeEmbeddedServiceStatus;
};

export type NodeEmbeddedServiceStatus = "starting" | "running" | "stopping" | "stopped" | "failed";

export type NodeErrorCategory = "authentication" | "authorization" | "validation" | "state" | "capability" | "dependency" | "internal";

export type NodeEvent = {
  "kind": NodeEventKind;
  "sequence": number;
  "snapshot_revision": number;
};

export type NodeEventCursor = {
  "sequence": number;
  "snapshot_revision": number;
};

export type NodeEventKind = {
  "data": {
  "revision": number;
};
  "type": "state_changed";
} | {
  "data": {
  "network_name": string;
  "status": NodeNetworkStatus;
};
  "type": "network_changed";
} | {
  "data": {
  "network_name": string;
  "status": NodeEmbeddedServiceStatus;
};
  "type": "embedded_service_changed";
} | {
  "data": {
  "level": NodeLogLevel;
  "message": string;
};
  "type": "log_appended";
} | {
  "data": {
  "exit_node"?: string | null;
  "is_exit_node": boolean;
  "mode": NodeRoutingMode;
  "network_name": string;
};
  "type": "routing_changed";
} | {
  "data": {
  "enabled": boolean;
  "network_name": string;
  "role": NodeNetworkRole;
  "roles": Array<NodeNetworkRole>;
};
  "type": "role_changed";
} | {
  "data": {
  "request_id": number;
  "success": boolean;
};
  "type": "command_completed";
} | {
  "data": {
  "operation_id": string;
  "stage": string;
  "state": OperationState;
};
  "type": "operation_changed";
};

export type NodeEventReplayReason = "cursor_ahead" | "history_gap" | "snapshot_rolled_back" | "lagged" | "revision_gap";

export type NodeHttpContract = {
  "api_command_response": NodeApiCommandResponse;
  "api_error_response": NodeApiErrorResponse;
  "api_event": NodeApiEvent;
  "api_manifest_response": NodeApiManifestResponse;
  "api_object_response": NodeApiObjectResponse;
  "api_operation_response": NodeApiOperationResponse;
  "api_snapshot_response": NodeApiSnapshotResponse;
  "command_request": NodeCommandHttpRequest;
  "command_response": NodeCommandHttpResponse;
  "error_response": NodeHttpErrorResponse;
  "manifest_response": NodeManifestResponse;
  "snapshot_response": NodeSnapshotResponse;
  "websocket_message": NodeWsMessage;
};

export type NodeHttpErrorCode = "schema_version_mismatch" | "invalid_request" | "payload_too_large" | "rate_limited" | "not_found" | "authentication_required" | "permission_denied" | "operation_conflict" | "operation_cancelled" | "operation_timed_out" | "resync_required" | "capability_unavailable" | "dependency_unavailable" | "backend_rejected" | "backend_unavailable" | "contract_violation" | "internal";

export type NodeHttpErrorResponse = {
  "category": NodeErrorCategory;
  "code": NodeHttpErrorCode;
  "message": string;
  "message_key": string;
  "object"?: DomainObjectRef | null;
  "params"?: Record<string, string>;
  "problem"?: NodeProblem | null;
  "recovery"?: OperationRecovery | null;
  "requested_schema_version"?: number | null;
  "retryable": boolean;
  "schema_version": number;
  "supported_schema_version"?: number | null;
};

export type NodeId = string;

export type NodeIdentitySnapshot = {
  "display_name"?: string | null;
  "node_id": string;
  "public_key_fingerprint": string;
};

export type NodeJoinNetworkCommand = {
  "network_name"?: string | null;
  "start": boolean;
  "target": string;
};

export type NodeLifecycleSnapshot = {
  "active_networks": number;
  "configured_networks": number;
  "failed_networks": Array<string>;
  "phase": NodePhase;
};

export type NodeLogEntry = {
  "level": NodeLogLevel;
  "message": string;
  "target": string;
  "unix_ms": number;
};

export type NodeLogLevel = "trace" | "debug" | "info" | "warn" | "error";

export type NodeManifestResponse = {
  "manifest": NodeUiManifest;
  "schema_version": number;
};

export type NodeNetworkAllocation = {
  "ipv4_pool": string;
  "ipv6_pool": string;
  "mode": NetworkMode;
};

export type NodeNetworkAllocationSnapshot = {
  "allocation": NodeNetworkAllocation;
  "config_saved": boolean;
  "network_name": string;
};

export type NodeNetworkConfigSnapshot = {
  "acl_default_action": AclAction;
  "acl_enabled": boolean;
  "acl_rule_count": number;
  "coordinator_url": string;
  "direct_cidrs": Array<string>;
  "exit_node"?: string | null;
  "exit_node_allow_lan_access": boolean;
  "exit_node_allowed_clients": Array<string>;
  "exit_node_enabled": boolean;
  "exit_node_region"?: string | null;
  "identity_configured": boolean;
  "log_file_configured": boolean;
  "log_level": string;
  "mtu": number;
  "network_name": string;
  "p2p_connect_timeout_secs": number;
  "p2p_heartbeat_interval_secs": number;
  "p2p_listen_port": number;
  "preferred_ipv4"?: string | null;
  "preferred_ipv6"?: string | null;
  "proxy_cidrs": Array<string>;
  "routing_mode": NodeRoutingMode;
  "tun_name": string;
  "virtual_mode"?: NetworkMode | null;
  "virtual_network"?: string | null;
};

export type NodeNetworkRole = "member" | "discovery" | "relay" | "exit_node";

export type NodeNetworkSelection = "default" | "all" | "none" | {
  "named": Array<string>;
};

export type NodeNetworkSnapshot = {
  "assigned_ipv4"?: string | null;
  "assigned_ipv6"?: string | null;
  "name": string;
  "p2p_listen_addr"?: string | null;
  "peer_count": number;
  "roles": Array<NodeNetworkRole>;
  "status": NodeNetworkStatus;
  "traffic": NodeTrafficSnapshot;
  "tun_name": string;
  "uptime_secs": number;
};

export type NodeNetworkStatus = "offline" | "connecting" | "online" | "degraded" | "stopping" | "failed";

export type NodeOperationSnapshot = {
  "actor": DomainActorRef;
  "attempt": number;
  "cancel_blocked_reason"?: string | null;
  "cancellable"?: boolean;
  "created_at_unix_ms": number;
  "error"?: OperationError | null;
  "idempotency_key": IdempotencyKey;
  "kind": OperationKind;
  "max_attempts": number;
  "operation_id": OperationId;
  "result"?: NodeCommandResult | null;
  "stage": string;
  "state": OperationState;
  "target": DomainObjectRef;
  "updated_at_unix_ms": number;
};

export type NodePeerProbeSnapshot = {
  "endpoint"?: string | null;
  "p2p_active": boolean;
  "peer_id": string;
  "rtt_us"?: number | null;
  "timed_out": boolean;
  "virtual_ipv6": string;
};

export type NodePeerSnapshot = {
  "endpoints": Array<string>;
  "loss_rate": number;
  "network_name": string;
  "online": boolean;
  "p2p_active": boolean;
  "p2p_healthy": boolean;
  "peer_id": string;
  "rtt_us"?: number | null;
  "traffic": NodeTrafficSnapshot;
  "virtual_ipv4"?: string | null;
  "virtual_ipv6": string;
};

export type NodePermission = "view_state" | "manage_networks" | "change_routing" | "manage_roles" | "probe_peers" | "stop_node" | "view_logs";

export type NodePermissionSnapshot = {
  "access_model": NodeAccessModel;
  "permissions": Array<NodePermission>;
};

export type NodePhase = "stopped" | "starting" | "running" | "stopping" | "failed";

export type NodePreviewConfigurationCommand = {
  "expected_revision": number;
  "networks": Record<string, unknown>;
};

export type NodeProbePeerCommand = {
  "network_name": string;
  "target": string;
  "timeout_ms": number;
};

export type NodeProblem = {
  "category": NodeErrorCategory;
  "code": NodeProblemCode;
  "details"?: NodeProblemDetails | null;
  "message": string;
  "message_key": string;
  "network_name"?: string | null;
  "params"?: Record<string, string>;
  "recovery"?: OperationRecovery | null;
  "retryable": boolean;
};

export type NodeProblemCode = "authentication_required" | "invalid_request" | "not_found" | "conflict" | "unavailable" | "permission_denied" | "cancelled" | "timed_out" | "internal" | "unsupported";

export type NodeProblemDetails = {
  "current_revision"?: number | null;
  "expected_revision"?: number | null;
  "validation_issues"?: Array<NodeValidationIssue>;
};

export type NodeResidualCleanupResult = {
  "cleaned_networks": Array<string>;
  "failed_networks": Array<NodeCleanupFailureSnapshot>;
  "residue": NodeRuntimeResidueSnapshot;
};

export type NodeRollbackConfigurationCommand = {
  "expected_revision": number;
  "target_revision": number;
};

export type NodeRoutingMode = "mesh_only" | "bypass_mainland" | "global_proxy";

export type NodeRuntimeResidueSnapshot = {
  "cleanup_steps": Array<NodeCleanupStepSnapshot>;
  "configuration_retained": boolean;
  "embedded_services_remaining": number;
  "issues": Array<string>;
  "listeners_remaining": number;
  "peers_remaining": number;
  "residual_networks": Array<string>;
  "sessions_remaining": number;
};

export type NodeSetNetworkRoleCommand = {
  "enabled": boolean;
  "network_name": string;
  "role": NodeNetworkRole;
};

export type NodeSnapshot = {
  "capabilities"?: NodeCapabilityRegistrySnapshot;
  "configuration"?: NodeConfigurationSnapshot;
  "control_health"?: NodeControlHealth;
  "embedded_services"?: Array<NodeEmbeddedServiceSnapshot>;
  "exit_node_capabilities"?: ExitNodeCapabilities;
  "identity"?: NodeIdentitySnapshot | null;
  "last_error"?: NodeProblem | null;
  "lifecycle": NodeLifecycleSnapshot;
  "logs": Array<NodeLogEntry>;
  "networks": Array<NodeNetworkSnapshot>;
  "peers": Array<NodePeerSnapshot>;
  "permissions"?: NodePermissionSnapshot;
  "recent_activity"?: Array<ActivityEntry>;
  "revision": number;
  "ui_mode"?: UiModeResolution | null;
};

export type NodeSnapshotResponse = {
  "schema_version": number;
  "snapshot": NodeSnapshot;
};

export type NodeTrafficSnapshot = {
  "error_count": number;
  "p2p_rx_bytes": number;
  "p2p_rx_packets": number;
  "p2p_tx_bytes": number;
  "p2p_tx_packets": number;
  "tun_rx_bytes": number;
  "tun_rx_packets": number;
  "tun_tx_bytes": number;
  "tun_tx_packets": number;
};

export type NodeUiAction = {
  "audit": NodeUiAudit;
  "availability": NodeUiAvailability;
  "backend_status": NodeUiBackendStatus;
  "command": "get_snapshot" | "get_node_diagnostics" | "start_networks" | "stop_networks" | "restart_networks" | "create_network" | "join_network" | "update_routing" | "set_network_role" | "get_blacklist" | "ban_client" | "unban_client" | "get_network_allocation" | "update_network_allocation" | "probe_peer" | "start_node" | "stop_node" | "restart_node" | "cleanup_node_residuals" | "get_configuration" | "preview_configuration" | "apply_configuration" | "rollback_configuration" | "get_configuration_history" | "shutdown";
  "confirmation"?: NodeUiConfirmation | null;
  "fields": Array<NodeUiField>;
  "id": string;
  "label_key": string;
  "permission": NodePermission;
};

export type NodeUiAudit = {
  "action"?: string | null;
  "policy": NodeUiAuditPolicy;
};

export type NodeUiAuditPolicy = "none" | "record_outcome";

export type NodeUiAvailability = "always" | "node_running" | "network_online" | "network_selected";

export type NodeUiBackendStatus = "implemented" | "unavailable";

export type NodeUiConfirmation = {
  "consequence_key": string;
  "destructive": boolean;
  "title_key": string;
};

export type NodeUiField = {
  "id": string;
  "input": NodeUiInputKind;
  "label_key": string;
  "required": boolean;
  "validation": NodeUiValidation;
};

export type NodeUiInputKind = "boolean" | "client_id" | "configuration_json" | "external_host" | "network_mode" | "network_name" | "network_selection" | "peer_target" | "port" | "revision" | "role" | "routing_mode" | "url_or_ticket" | "timeout_ms";

export type NodeUiManifest = {
  "pages": Array<NodeUiPage>;
  "schema_version": number;
};

export type NodeUiPage = {
  "actions": Array<NodeUiAction>;
  "id": string;
  "nav_label_key": string;
  "title_key": string;
};

export type NodeUiValidation = "none" | "non_empty" | "network_name" | "port" | "distinct_ports" | "network_mode" | "network_selection" | "role" | "routing_mode" | "positive_timeout" | "revision" | "configuration_json";

export type NodeUpdateNetworkAllocationCommand = {
  "allocation": NodeNetworkAllocation;
  "network_name": string;
};

export type NodeUpdateRoutingCommand = {
  "exit_node"?: string | null;
  "is_exit_node": boolean;
  "mode": NodeRoutingMode;
  "network_name": string;
};

export type NodeValidationIssue = {
  "code": string;
  "message": string;
  "path": string;
};

export type NodeWsMessage = {
  "schema_version": number;
  "snapshot": NodeSnapshot;
  "type": "snapshot";
} | {
  "event": NodeEvent;
  "schema_version": number;
  "type": "event";
} | {
  "reason": NodeWsResyncReason;
  "schema_version": number;
  "snapshot": NodeSnapshot;
  "type": "resync";
} | {
  "category": NodeErrorCategory;
  "code": NodeHttpErrorCode;
  "message": string;
  "message_key": string;
  "object"?: DomainObjectRef | null;
  "params"?: Record<string, string>;
  "recovery"?: OperationRecovery | null;
  "retryable": boolean;
  "schema_version": number;
  "type": "error";
};

export type NodeWsResyncReason = "lagged" | "revision_gap";

export type ObjectKind = "node" | "network" | "membership" | "group" | "security_policy" | "traffic_policy" | "capability" | "exit_grant" | "coordinator_term" | "file_share" | "operation" | "audit";

export type OperationError = {
  "code": string;
  "message_key": string;
  "object"?: DomainObjectRef | null;
  "params"?: Record<string, string>;
  "partial_changes"?: Array<DomainObjectRef>;
  "recovery"?: OperationRecovery | null;
  "retryable": boolean;
};

export type OperationId = string;

export type OperationKind = "node_start" | "node_stop" | "node_restart" | "node_cleanup" | "network_create" | "network_apply" | "network_leave" | "membership_approve" | "membership_reject" | "membership_remove" | "policy_test" | "policy_apply" | "policy_rollback" | "traffic_rules_apply" | "network_role_update" | "client_blacklist_update" | "network_allocation_update" | "capability_enable" | "capability_disable" | "exit_select" | "exit_revoke" | "coordinator_candidates_update" | "coordinator_recover" | "file_share_update" | "file_upload_start" | "file_download_start" | "owner_passkey_register" | "owner_passkey_revoke" | "certificate_reload";

export type OperationRecovery = {
  "hint_key": string;
  "kind": string;
};

export type OperationState = "queued" | "running" | "waiting_for_reauth" | "succeeded" | "failed" | "cancelled" | "timed_out";

export type UiMode = "gpui" | "web_ui" | "headless";

export type UiModeRequest = "auto" | "gpui" | "web_ui" | "headless";

export type UiModeResolution = {
  "display": DisplayEnvironment;
  "effective": UiMode;
  "gpui_available": boolean;
  "requested": UiModeRequest;
};


export type NodeFetcher = (input: string, init?: RequestInit) => Promise<Response>;

/** Thin transport client. Deserialize once at the edge; do not clone domain logic. */
export class NodeContractV1Client {
  private readonly baseUrl: string;
  private readonly token: string;
  private readonly fetcher: NodeFetcher;

  constructor(
    baseUrl: string,
    token: string,
    fetcher: NodeFetcher = globalThis.fetch.bind(globalThis),
  ) {
    this.baseUrl = baseUrl.replace(/\/+$/, "");
    this.token = token;
    this.fetcher = fetcher;
  }

  async loadManifest(): Promise<NodeManifestResponse> {
    return this.request<NodeManifestResponse>(
      `/api/node/manifest?schema_version=${NODE_CONTRACT_SCHEMA_VERSION}`,
    );
  }

  async loadSnapshot(): Promise<NodeSnapshotResponse> {
    return this.request<NodeSnapshotResponse>(
      `/api/node/snapshot?schema_version=${NODE_CONTRACT_SCHEMA_VERSION}`,
    );
  }

  async sendCommand(request: NodeCommandRequest): Promise<NodeCommandHttpResponse> {
    const response = await this.request<NodeCommandHttpResponse>(
      "/api/node/commands",
      {
        method: "POST",
        body: JSON.stringify({
          schema_version: NODE_CONTRACT_SCHEMA_VERSION,
          request,
        } satisfies NodeCommandHttpRequest),
      },
    );
    if (response.result.request_id !== request.request_id) {
      throw new Error(
        `node contract: response request_id ${response.result.request_id} does not match ${request.request_id}`,
      );
    }
    return response;
  }

  private async request<T extends { schema_version: number }>(
    path: string,
    init: RequestInit = {},
  ): Promise<T> {
    const headers = new Headers(init.headers);
    headers.set("Authorization", `Bearer ${this.token}`);
    if (init.body !== undefined) {
      headers.set("Content-Type", "application/json");
    }
    const response = await this.fetcher(`${this.baseUrl}${path}`, { ...init, headers });
    const body: unknown = await response.json();
    if (!isRecord(body) || body.schema_version !== NODE_CONTRACT_SCHEMA_VERSION) {
      throw new Error("node contract: schema version mismatch");
    }
    if (!response.ok) {
      throw new NodeContractHttpError(response.status, body as unknown as NodeHttpErrorResponse);
    }
    return body as T;
  }
}

export class NodeContractHttpError extends Error {
  constructor(
    readonly status: number,
    readonly payload: NodeHttpErrorResponse,
  ) {
    super(payload.message || `node HTTP error ${status}`);
    this.name = "NodeContractHttpError";
  }
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}
