// Generated from contracts/schema/*.schema.json. Do not edit by hand.
// Regenerate with scripts/generate-contracts.sh.

export const NODE_CONTRACT_SCHEMA_VERSION = 1 as const;
export const NODE_COMMAND_KINDS = ["apply_configuration", "ban_client", "batch_review_join_requests", "cleanup_node_residuals", "create_network", "get_blacklist", "get_configuration", "get_configuration_history", "get_network_allocation", "get_node_diagnostics", "get_snapshot", "get_ui_manifest", "join_network", "leave_network", "list_join_requests", "preview_configuration", "preview_join_network", "preview_network_profile", "probe_peer", "publish_network_discovery", "rebuild_tun", "refresh_discovery_index", "restart_networks", "restart_node", "resubmit_join_request", "retry_discovery_index", "review_join_request", "rollback_configuration", "rotate_node_identity", "set_network_role", "set_node_display_name", "shutdown", "start_networks", "start_node", "stop_networks", "stop_node", "submit_join_request", "unban_client", "unpublish_network_discovery", "update_network_allocation", "update_network_discovery", "update_network_profile", "update_routing", "withdraw_join_request"] as const;
export type NodeCommandKindWire = (typeof NODE_COMMAND_KINDS)[number];
export const NODE_MANIFEST_ACTION_COMMAND_KINDS = ["apply_configuration", "ban_client", "batch_review_join_requests", "cleanup_node_residuals", "create_network", "get_blacklist", "get_configuration", "get_configuration_history", "get_network_allocation", "get_node_diagnostics", "get_snapshot", "join_network", "leave_network", "list_join_requests", "preview_configuration", "preview_join_network", "preview_network_profile", "probe_peer", "publish_network_discovery", "rebuild_tun", "refresh_discovery_index", "restart_networks", "restart_node", "resubmit_join_request", "retry_discovery_index", "review_join_request", "rollback_configuration", "rotate_node_identity", "set_network_role", "set_node_display_name", "shutdown", "start_networks", "start_node", "stop_networks", "stop_node", "submit_join_request", "unban_client", "unpublish_network_discovery", "update_network_allocation", "update_network_discovery", "update_network_profile", "update_routing", "withdraw_join_request"] as const;
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

export type ContentHash = string;

export type DiscoveryTransport = "quic" | "tcp" | "websocket" | "local";

export type DisplayEnvironment = "linux_x11" | "linux_wayland" | "windows_desktop" | "macos_desktop" | "headless";

export type DomainActorRef = {
  "display": string;
  "node_id": NodeId;
};

export type DomainObjectRef = {
  "id": string;
  "kind": ObjectKind;
};

export type ExitContributionObligation = "required" | "not_required";

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

export type JoinApplicantProfile = {
  "display_name"?: string | null;
  "node_id": NodeId;
  "public_key_fingerprint": string;
};

export type JoinRequest = {
  "applicant": JoinApplicantProfile;
  "created_at_unix_ms": number;
  "decided_at_unix_ms"?: number | null;
  "expires_at_unix_ms": number;
  "grant_preview": JoinRequestGrantPreview;
  "information_request"?: string | null;
  "network_id": NetworkId;
  "reason"?: string | null;
  "reason_retention_until_unix_ms"?: number | null;
  "reason_visibility"?: JoinRequestReasonVisibility;
  "request_id": JoinRequestId;
  "reviewer_node_id"?: NodeId | null;
  "revision": number;
  "schema_version": number;
  "state": JoinRequestState;
  "terms": MembershipTerms;
  "updated_at_unix_ms": number;
};

export type JoinRequestGrantPreview = {
  "capabilities"?: Array<CapabilityKind>;
  "exit_contribution"?: ExitContributionObligation;
  "file_access"?: boolean;
  "grants_data_plane_access"?: boolean;
  "role": MembershipRole;
};

export type JoinRequestId = string;

export type JoinRequestReasonVisibility = "owners_and_administrators";

export type JoinRequestReviewAction = "approve" | "reject" | "request_information";

export type JoinRequestState = "pending" | "needs_information" | "approved" | "rejected" | "withdrawn" | "expired";

export type MembershipRole = "owner" | "administrator" | "member";

export type MembershipState = "discovered" | "applying" | "pending_approval" | "accepted" | "key_ready" | "connecting" | "active" | "degraded" | "leaving" | "left" | "rejected" | "suspended" | "cancelled";

export type MembershipTerms = {
  "exit_contribution": ExitContributionObligation;
  "member_limit"?: number | null;
  "network_policy_revision": number;
  "privacy_policy_revision": number;
};

export type NetworkAdminProfileProjection = {
  "administrator_count": number;
  "member_count": number;
  "policy_revision": number;
  "profile": NetworkPublicProfileDocument;
  "revision": number;
};

export type NetworkDiscoveryDocument = {
  "administrators"?: Array<NodeId>;
  "capability_summary": Array<CapabilityKind>;
  "endpoints"?: Array<NetworkEndpoint>;
  "exit_contribution"?: ExitContributionObligation;
  "expires_at_unix_ms": number;
  "issued_at_unix_ms": number;
  "join_mode": NetworkJoinMode;
  "language"?: string;
  "member_count"?: number;
  "member_limit"?: number | null;
  "name": string;
  "network_id": NetworkId;
  "owner": NodeId;
  "policy_revision": number;
  "privacy_changes"?: Array<string>;
  "privacy_policy_revision": number;
  "public_key": PublicKey;
  "publisher": NodeId;
  "purpose": string;
  "region"?: string;
  "revision": number;
  "revoked_at_unix_ms"?: number | null;
  "schema_version": number;
  "tags"?: Array<string>;
  "visibility": NetworkVisibility;
};

export type NetworkDiscoveryRecord = {
  "document": NetworkDiscoveryDocument;
  "signature": Signature;
};

export type NetworkEndpoint = {
  "host": string;
  "port": number;
  "transport": DiscoveryTransport;
};

export type NetworkId = string;

export type NetworkJoinMode = "open" | "approval_required" | "invite_only";

export type NetworkLocalSecretProjection = {
  "identity_present": boolean;
  "public_key_fingerprint": string;
  "secret_material_included": boolean;
  "storage": string;
};

export type NetworkMemberProfileProjection = {
  "administrator_count": number;
  "member_count": number;
  "profile": NetworkPublicProfileDocument;
};

export type NetworkMode = "ipv4_only" | "ipv6_only" | "dual_stack";

export type NetworkProfileApplyResult = {
  "applied_at_unix_ms": number;
  "audit_action": string;
  "changed_paths": Array<string>;
  "network_name": string;
  "previous_revision": number;
  "projections": NetworkProfileProjections;
  "revision": number;
  "signed_profile": NetworkPublicProfileRecord;
  "warnings": Array<NetworkProfileWarning>;
};

export type NetworkProfileDiffEntry = {
  "after"?: string | null;
  "before"?: string | null;
  "path": string;
};

export type NetworkProfilePreview = {
  "can_apply": boolean;
  "candidate_revision": number;
  "conflict": boolean;
  "current_revision": number;
  "diff": Array<NetworkProfileDiffEntry>;
  "expected_revision": number;
  "network_name": string;
  "projections": NetworkProfileProjections;
  "warnings": Array<NetworkProfileWarning>;
};

export type NetworkProfileProjections = {
  "administrator_view": NetworkAdminProfileProjection;
  "local_secret": NetworkLocalSecretProjection;
  "member_view": NetworkMemberProfileProjection;
  "public_card": NetworkPublicProfileProjection;
};

export type NetworkProfileUpdate = {
  "capability_declarations"?: Array<CapabilityKind>;
  "icon": string;
  "join_mode": NetworkJoinMode;
  "language": string;
  "purpose": string;
  "region": string;
  "tags"?: Array<string>;
  "visibility": NetworkVisibility;
};

export type NetworkProfileWarning = {
  "capability"?: CapabilityKind | null;
  "code": NetworkProfileWarningCode;
  "field"?: string | null;
  "message": string;
};

export type NetworkProfileWarningCode = "unsigned_legacy" | "profile_expired" | "revision_conflict" | "capability_not_observed" | "capability_undeclared";

export type NetworkPublicProfileDocument = {
  "capability_declarations": Array<CapabilityKind>;
  "expires_at_unix_ms": number;
  "icon": string;
  "issued_at_unix_ms": number;
  "join_mode": NetworkJoinMode;
  "language": string;
  "name": string;
  "network_id": NetworkId;
  "owner": NodeId;
  "public_key": PublicKey;
  "purpose": string;
  "region": string;
  "revision": number;
  "schema_version": number;
  "tags": Array<string>;
  "visibility": NetworkVisibility;
};

export type NetworkPublicProfileProjection = {
  "candidate": NetworkPublicProfileDocument;
  "currently_published"?: NetworkPublicProfileRecord | null;
  "withheld_categories": Array<string>;
};

export type NetworkPublicProfileRecord = {
  "document": NetworkPublicProfileDocument;
  "signature": Signature;
};

export type NetworkVisibility = "public" | "unlisted" | "private";

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

export type NodeBatchReviewJoinRequestsCommand = {
  "network_id": string;
  "reviews": Array<NodeJoinRequestReviewItem>;
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
  "command": "rebuild_tun";
  "payload": {
  "network_name": string;
};
} | {
  "command": "create_network";
  "payload": NodeCreateNetworkCommand;
} | {
  "command": "preview_network_profile";
  "payload": NodeNetworkProfileCommand;
} | {
  "command": "update_network_profile";
  "payload": NodeNetworkProfileCommand;
} | {
  "command": "publish_network_discovery";
  "payload": NodeNetworkDiscoveryCommand;
} | {
  "command": "update_network_discovery";
  "payload": NodeNetworkDiscoveryCommand;
} | {
  "command": "unpublish_network_discovery";
  "payload": NodeNetworkDiscoveryCommand;
} | {
  "command": "refresh_discovery_index";
  "payload": {
  "index_url"?: string | null;
};
} | {
  "command": "retry_discovery_index";
} | {
  "command": "preview_join_network";
  "payload": NodePreviewJoinNetworkCommand;
} | {
  "command": "join_network";
  "payload": NodeJoinNetworkCommand;
} | {
  "command": "submit_join_request";
  "payload": NodeSubmitJoinRequestCommand;
} | {
  "command": "list_join_requests";
  "payload": NodeListJoinRequestsCommand;
} | {
  "command": "review_join_request";
  "payload": NodeReviewJoinRequestCommand;
} | {
  "command": "batch_review_join_requests";
  "payload": NodeBatchReviewJoinRequestsCommand;
} | {
  "command": "withdraw_join_request";
  "payload": NodeWithdrawJoinRequestCommand;
} | {
  "command": "resubmit_join_request";
  "payload": NodeResubmitJoinRequestCommand;
} | {
  "command": "leave_network";
  "payload": NodeLeaveNetworkCommand;
} | {
  "command": "update_routing";
  "payload": NodeUpdateRoutingCommand;
} | {
  "command": "set_network_role";
  "payload": NodeSetNetworkRoleCommand;
} | {
  "command": "set_node_display_name";
  "payload": NodeSetNodeDisplayNameCommand;
} | {
  "command": "rotate_node_identity";
  "payload": NodeRotateNodeIdentityCommand;
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
  "data": NetworkProfilePreview;
  "status": "network_profile_preview";
} | {
  "data": NetworkProfileApplyResult;
  "status": "network_profile_updated";
} | {
  "data": NodeNetworkJoinPreview;
  "status": "network_join_preview";
} | {
  "data": NodeNetworkJoinResult;
  "status": "network_joined";
} | {
  "data": JoinRequest;
  "status": "join_request_submitted";
} | {
  "data": NodeJoinRequestListResult;
  "status": "join_requests";
} | {
  "data": NodeJoinRequestReviewAck;
  "status": "join_requests_reviewed";
} | {
  "data": JoinRequest;
  "status": "join_request_updated";
} | {
  "data": NodeNetworkLeaveResult;
  "status": "network_left";
} | {
  "data": NodeIdentityRotationSnapshot;
  "status": "identity_rotated";
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
  "capability_declarations"?: Array<CapabilityKind>;
  "external_host"?: string | null;
  "icon"?: string;
  "join_mode"?: NetworkJoinMode;
  "language"?: string;
  "name": string;
  "purpose"?: string;
  "region"?: string;
  "relay_port": number;
  "signaling_port": number;
  "tags"?: Array<string>;
  "visibility"?: NetworkVisibility;
};

export type NodeCreatedNetwork = {
  "config_saved": boolean;
  "invitation_ticket": string;
  "network_id": string;
  "network_name": string;
  "next_steps": Array<NodeNetworkCreationNextStep>;
  "owner_role": MembershipRole;
  "public_key": string;
  "started": boolean;
};

export type NodeDiagnosticCheck = {
  "duration_ms": number;
  "error_code"?: string | null;
  "layer": NodeDiagnosticLayer;
  "message_key": string;
  "started_at_unix_ms": number;
  "state": NodeDiagnosticState;
  "suggestion_key"?: string | null;
};

export type NodeDiagnosticLayer = "network" | "dns" | "tun" | "coordinator" | "relay" | "exit";

export type NodeDiagnosticRecoveryAction = {
  "action_id": string;
  "confirmation_required": boolean;
  "impact_key": string;
};

export type NodeDiagnosticState = "not_tested" | "passed" | "failed" | "unsupported" | "no_permission";

export type NodeDiagnosticsSnapshot = {
  "checks": Array<NodeDiagnosticCheck>;
  "cleanup_candidate_networks": Array<string>;
  "completed_at_unix_ms": number;
  "diagnostic_id": string;
  "lifecycle": NodeLifecycleSnapshot;
  "recovery_actions": Array<NodeDiagnosticRecoveryAction>;
  "residue": NodeRuntimeResidueSnapshot;
  "retained_owner_networks": Array<string>;
  "sanitized_report": string;
  "started_at_unix_ms": number;
  "timed_out": boolean;
};

export type NodeDiscoveryIndexError = {
  "category": NodeDiscoveryIndexErrorCategory;
  "retryable": boolean;
};

export type NodeDiscoveryIndexErrorCategory = "offline" | "timeout" | "unsupported" | "rejected" | "invalid_response" | "storage" | "server";

export type NodeDiscoveryIndexRecordSnapshot = {
  "accepted_at_unix_ms": number;
  "digest": string;
  "discoverable": boolean;
  "invalid_reason"?: string | null;
  "record": NetworkDiscoveryRecord;
};

export type NodeDiscoveryIndexSnapshot = {
  "data_plane_affected": boolean;
  "index_url"?: string | null;
  "last_attempt_unix_ms"?: number | null;
  "last_error"?: NodeDiscoveryIndexError | null;
  "last_success_unix_ms"?: number | null;
  "records": Array<NodeDiscoveryIndexRecordSnapshot>;
  "rejected_record_count": number;
  "search_history_stored": boolean;
  "state": NodeDiscoveryIndexState;
};

export type NodeDiscoveryIndexState = "idle" | "pending" | "ready" | "degraded" | "failed" | "unsupported";

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

export type NodeIdentityRotationSnapshot = {
  "affected_configured_network_count": number;
  "created_at_unix_ms": number;
  "new_fingerprint": string;
  "new_node_id": string;
  "old_fingerprint": string;
  "old_node_id": string;
  "owner_initialized": boolean;
  "owner_sessions_invalidated": boolean;
};

export type NodeIdentitySnapshot = {
  "created_at_unix_ms"?: number | null;
  "display_name"?: string | null;
  "node_id": string;
  "public_key_fingerprint": string;
};

export type NodeInterfaceAddress = {
  "address": string;
  "prefix_len": number;
};

export type NodeInterfaceKind = "loopback" | "physical" | "virtual" | "tun" | "unknown";

export type NodeInterfaceOwnership = "system" | "product" | "unknown";

export type NodeInterfaceSnapshot = {
  "addresses": Array<NodeInterfaceAddress>;
  "default_ipv4": boolean;
  "default_ipv6": boolean;
  "index": number;
  "kind": NodeInterfaceKind;
  "mtu": number;
  "name": string;
  "ownership": NodeInterfaceOwnership;
  "status": NodeInterfaceStatus;
};

export type NodeInterfaceStatus = "up" | "down" | "unknown";

export type NodeJoinNetworkCommand = {
  "acknowledged_at_unix_ms"?: number | null;
  "discovery_record"?: NetworkDiscoveryRecord | null;
  "network_id"?: string | null;
  "network_name"?: string | null;
  "network_policy_revision"?: number | null;
  "preview_digest"?: string | null;
  "privacy_policy_revision"?: number | null;
  "start": boolean;
  "target": string;
  "terms_hash"?: ContentHash | null;
};

export type NodeJoinRequestListResult = {
  "network_id": string;
  "requests": Array<JoinRequest>;
};

export type NodeJoinRequestReviewAck = {
  "network_id": string;
  "reviews": Array<NodeJoinRequestReviewOutcome>;
};

export type NodeJoinRequestReviewItem = {
  "action": JoinRequestReviewAction;
  "expected_revision": number;
  "note"?: string | null;
  "request_id": string;
};

export type NodeJoinRequestReviewOutcome = {
  "request_id": string;
  "revision": number;
  "state": JoinRequestState;
};

export type NodeLeaveNetworkCommand = {
  "network_name": string;
};

export type NodeLifecycleSnapshot = {
  "active_networks": number;
  "configured_networks": number;
  "failed_networks": Array<string>;
  "phase": NodePhase;
};

export type NodeListJoinRequestsCommand = {
  "network_id": string;
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

export type NodeNetworkCreationNextStep = "invite_members" | "configure_policy" | "contribute_capabilities";

export type NodeNetworkDiscoveryCommand = {
  "index_url": string;
  "network_name": string;
};

export type NodeNetworkJoinPreview = {
  "administrators": Array<string>;
  "already_member": boolean;
  "blocked_reason_key"?: string | null;
  "can_join": boolean;
  "capability_summary": Array<CapabilityKind>;
  "contribution_policy_key": string;
  "discovery_revision": number;
  "endpoints": Array<NetworkEndpoint>;
  "exit_contribution": ExitContributionObligation;
  "expires_at_unix_ms": number;
  "issued_at_unix_ms": number;
  "join_mode": NetworkJoinMode;
  "locally_contributed_capabilities": Array<CapabilityKind>;
  "member_count": number;
  "member_limit"?: number | null;
  "network_id": string;
  "network_name": string;
  "network_policy_revision": number;
  "owner": string;
  "preview_digest": string;
  "privacy_changes": Array<string>;
  "privacy_policy_revision": number;
  "public_key": string;
  "terms_hash": ContentHash;
  "visibility": NetworkVisibility;
};

export type NodeNetworkJoinResult = {
  "config_saved": boolean;
  "idempotent": boolean;
  "membership": NodeNetworkMembershipSnapshot;
  "snapshot": NodeSnapshot;
  "started": boolean;
};

export type NodeNetworkLeaveResult = {
  "left": boolean;
  "network_name": string;
  "snapshot": NodeSnapshot;
};

export type NodeNetworkMembershipSnapshot = {
  "acknowledged_at_unix_ms"?: number | null;
  "created_at_unix_ms": number;
  "exit_contribution"?: ExitContributionObligation | null;
  "local_policy_revision": number;
  "member_limit"?: number | null;
  "network_id": string;
  "network_name": string;
  "network_policy_revision"?: number | null;
  "node_id": string;
  "privacy_policy_revision"?: number | null;
  "role": MembershipRole;
  "state": MembershipState;
  "suspension_reason"?: string | null;
  "terms_hash"?: string | null;
  "updated_at_unix_ms": number;
};

export type NodeNetworkProfileCommand = {
  "expected_revision": number;
  "network_name": string;
  "update": NetworkProfileUpdate;
};

export type NodeNetworkProfileSnapshot = {
  "capability_declarations": Array<CapabilityKind>;
  "icon": string;
  "join_mode": NetworkJoinMode;
  "language": string;
  "name": string;
  "network_id": string;
  "policy_revision": number;
  "purpose": string;
  "region": string;
  "revision": number;
  "signed_profile"?: NodeSignedNetworkProfileSnapshot | null;
  "tags": Array<string>;
  "visibility": NetworkVisibility;
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

export type NodePreviewJoinNetworkCommand = {
  "network_id": string;
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

export type NodeResubmitJoinRequestCommand = {
  "expected_revision": number;
  "network_id": string;
  "reason": string;
  "request_id": string;
};

export type NodeReviewJoinRequestCommand = {
  "network_id": string;
  "review": NodeJoinRequestReviewItem;
};

export type NodeRollbackConfigurationCommand = {
  "expected_revision": number;
  "target_revision": number;
};

export type NodeRotateNodeIdentityCommand = {
  "expected_fingerprint": string;
};

export type NodeRoutingDefaultAction = {
  "kind": "direct";
} | {
  "kind": "network";
} | {
  "kind": "exit";
  "value": string;
} | {
  "kind": "reject";
};

export type NodeRoutingMode = "mesh_only" | "bypass_mainland" | "global_proxy";

export type NodeRoutingSummarySnapshot = {
  "default_action": NodeRoutingDefaultAction;
  "effective_target"?: NodeRoutingTargetSnapshot | null;
  "enabled_rule_count": number;
  "last_applied_at_unix_ms"?: number | null;
  "last_applied_revision"?: number | null;
  "network_name": string;
  "problem"?: NodeProblem | null;
  "status": NodeRoutingSummaryStatus;
};

export type NodeRoutingSummaryStatus = "current" | "stale" | "invalid" | "apply_failed";

export type NodeRoutingTargetSnapshot = {
  "kind": "network";
  "network_name": string;
} | {
  "kind": "exit";
  "node_id": string;
};

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

export type NodeSetNodeDisplayNameCommand = {
  "display_name"?: string | null;
};

export type NodeSignedNetworkProfileSnapshot = {
  "expires_at_unix_ms": number;
  "issued_at_unix_ms": number;
  "schema_version": number;
};

export type NodeSnapshot = {
  "capabilities"?: NodeCapabilityRegistrySnapshot;
  "configuration"?: NodeConfigurationSnapshot;
  "control_health"?: NodeControlHealth;
  "discovery_index"?: NodeDiscoveryIndexSnapshot;
  "embedded_services"?: Array<NodeEmbeddedServiceSnapshot>;
  "exit_node_capabilities"?: ExitNodeCapabilities;
  "identity"?: NodeIdentitySnapshot | null;
  "interface_problems"?: Array<NodeProblem>;
  "interfaces"?: Array<NodeInterfaceSnapshot>;
  "last_error"?: NodeProblem | null;
  "lifecycle": NodeLifecycleSnapshot;
  "logs": Array<NodeLogEntry>;
  "memberships"?: Array<NodeNetworkMembershipSnapshot>;
  "network_profiles"?: Array<NodeNetworkProfileSnapshot>;
  "networks": Array<NodeNetworkSnapshot>;
  "peers": Array<NodePeerSnapshot>;
  "permissions"?: NodePermissionSnapshot;
  "recent_activity"?: Array<ActivityEntry>;
  "revision": number;
  "routing_summaries"?: Array<NodeRoutingSummarySnapshot>;
  "tun_capabilities"?: TunCapabilities;
  "ui_mode"?: UiModeResolution | null;
};

export type NodeSnapshotResponse = {
  "schema_version": number;
  "snapshot": NodeSnapshot;
};

export type NodeSubmitJoinRequestCommand = {
  "network_id": string;
  "reason": string;
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
  "command": "get_snapshot" | "get_node_diagnostics" | "start_networks" | "stop_networks" | "restart_networks" | "rebuild_tun" | "create_network" | "preview_network_profile" | "update_network_profile" | "publish_network_discovery" | "update_network_discovery" | "unpublish_network_discovery" | "refresh_discovery_index" | "retry_discovery_index" | "preview_join_network" | "join_network" | "submit_join_request" | "list_join_requests" | "review_join_request" | "batch_review_join_requests" | "withdraw_join_request" | "resubmit_join_request" | "leave_network" | "update_routing" | "set_network_role" | "set_node_display_name" | "rotate_node_identity" | "get_blacklist" | "ban_client" | "unban_client" | "get_network_allocation" | "update_network_allocation" | "probe_peer" | "start_node" | "stop_node" | "restart_node" | "cleanup_node_residuals" | "get_configuration" | "preview_configuration" | "apply_configuration" | "rollback_configuration" | "get_configuration_history" | "shutdown";
  "confirmation"?: NodeUiConfirmation | null;
  "fields": Array<NodeUiField>;
  "id": string;
  "label_key": string;
  "permission": NodePermission;
};

export type NodeUiActionBlockReason = "node_not_running" | "no_online_network" | "no_network_selected" | "permission_denied" | "platform_unsupported" | "exit_node_unsupported" | "tun_platform_unsupported" | "tun_permission_denied" | "tun_device_unavailable" | "tun_tool_unavailable" | "tun_unavailable" | "backend_unavailable";

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
  "recent_authentication_required": boolean;
  "title_key": string;
};

export type NodeUiField = {
  "id": string;
  "input": NodeUiInputKind;
  "label_key": string;
  "required": boolean;
  "validation": NodeUiValidation;
};

export type NodeUiInputKind = "boolean" | "client_id" | "configuration_json" | "display_name" | "external_host" | "fingerprint" | "network_mode" | "network_name" | "network_selection" | "peer_target" | "purpose" | "icon" | "language" | "region" | "tags" | "capability_declarations" | "visibility" | "join_mode" | "join_request_id" | "join_request_note" | "join_request_reason" | "join_request_review_action" | "join_request_reviews" | "port" | "revision" | "role" | "routing_mode" | "url_or_ticket" | "discovery_index_url" | "timeout_ms";

export type NodeUiManifest = {
  "action_block_reasons": Array<NodeUiActionBlockReason>;
  "page_states": Array<NodeUiPageState>;
  "pages": Array<NodeUiPage>;
  "schema_version": number;
};

export type NodeUiPage = {
  "actions": Array<NodeUiAction>;
  "id": string;
  "nav_label_key": string;
  "title_key": string;
};

export type NodeUiPageState = "initial_loading" | "error" | "maintenance" | "permission_denied" | "unsupported" | "offline" | "control_unavailable" | "node_failed" | "degraded" | "stale" | "refreshing" | "empty" | "ready";

export type NodeUiValidation = "none" | "non_empty" | "network_name" | "port" | "distinct_ports" | "network_mode" | "network_selection" | "role" | "routing_mode" | "positive_timeout" | "revision" | "configuration_json" | "display_name" | "fingerprint" | "network_purpose" | "network_icon" | "network_language" | "region" | "tags" | "capability_declarations" | "visibility" | "join_mode" | "join_request_id" | "join_request_note" | "join_request_reason" | "join_request_review_action" | "join_request_reviews";

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

export type NodeWithdrawJoinRequestCommand = {
  "network_id": string;
  "request_id": string;
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

export type OperationKind = "node_start" | "node_stop" | "node_restart" | "node_cleanup" | "diagnostics" | "network_create" | "network_profile_update" | "discovery_index_update" | "network_apply" | "tun_rebuild" | "network_leave" | "membership_approve" | "membership_reject" | "membership_remove" | "policy_test" | "policy_apply" | "policy_rollback" | "traffic_rules_apply" | "network_role_update" | "identity_update" | "client_blacklist_update" | "network_allocation_update" | "capability_enable" | "capability_disable" | "exit_select" | "exit_revoke" | "coordinator_candidates_update" | "coordinator_recover" | "file_share_update" | "file_upload_start" | "file_download_start" | "owner_passkey_register" | "owner_passkey_revoke" | "certificate_reload";

export type OperationRecovery = {
  "hint_key": string;
  "kind": string;
};

export type OperationState = "queued" | "running" | "waiting_for_reauth" | "succeeded" | "failed" | "cancelled" | "timed_out";

export type PublicKey = string;

export type Signature = string;

export type TunCapabilities = {
  "can_rebuild": boolean;
  "platform": TunPlatform;
  "reasons"?: Array<TunCapabilityReason>;
  "schema_version": number;
  "supported": boolean;
};

export type TunCapabilityReason = "platform_unsupported" | "permission_denied" | "device_unavailable" | "tool_unavailable" | "unknown";

export type TunPlatform = "linux" | "macos" | "windows" | "other";

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
