// Generated from contracts/schema/*.schema.json. Do not edit by hand.
// Regenerate with scripts/generate-contracts.sh.

export const NODE_CONTRACT_SCHEMA_VERSION = 1 as const;
export const NODE_COMMAND_KINDS = ["acknowledge_membership_policy", "apply_configuration", "apply_policy", "apply_traffic_policy", "ban_client", "batch_review_join_requests", "change_member_lifecycle", "cleanup_node_residuals", "create_invitation", "create_network", "get_blacklist", "get_configuration", "get_configuration_history", "get_invitation_status", "get_network_allocation", "get_node_diagnostics", "get_policy", "get_policy_history", "get_snapshot", "get_traffic_policy", "get_traffic_policy_history", "get_ui_manifest", "join_network", "leave_network", "list_invitations", "list_join_requests", "list_network_members", "preview_configuration", "preview_invitation", "preview_join_network", "preview_member_removal", "preview_network_profile", "preview_policy", "preview_policy_rollback", "preview_traffic_policy", "preview_traffic_policy_rollback", "probe_peer", "publish_network_discovery", "rebuild_tun", "redeem_invitation", "refresh_discovery_index", "restart_networks", "restart_node", "resubmit_join_request", "retry_discovery_index", "review_join_request", "revoke_invitation", "rollback_configuration", "rollback_policy", "rollback_traffic_policy", "rotate_node_identity", "set_local_exit_capability", "set_network_role", "set_node_display_name", "shutdown", "start_networks", "start_node", "stop_networks", "stop_node", "submit_join_request", "test_policy", "test_traffic_policy", "unban_client", "unpublish_network_discovery", "update_network_allocation", "update_network_discovery", "update_network_profile", "update_routing", "withdraw_join_request"] as const;
export type NodeCommandKindWire = (typeof NODE_COMMAND_KINDS)[number];
export const NODE_MANIFEST_ACTION_COMMAND_KINDS = ["acknowledge_membership_policy", "apply_configuration", "apply_policy", "apply_traffic_policy", "ban_client", "batch_review_join_requests", "change_member_lifecycle", "cleanup_node_residuals", "create_invitation", "create_network", "get_blacklist", "get_configuration", "get_configuration_history", "get_invitation_status", "get_network_allocation", "get_node_diagnostics", "get_policy", "get_policy_history", "get_snapshot", "get_traffic_policy", "get_traffic_policy_history", "join_network", "leave_network", "list_invitations", "list_join_requests", "list_network_members", "preview_configuration", "preview_invitation", "preview_join_network", "preview_member_removal", "preview_network_profile", "preview_policy", "preview_policy_rollback", "preview_traffic_policy", "preview_traffic_policy_rollback", "probe_peer", "publish_network_discovery", "rebuild_tun", "redeem_invitation", "refresh_discovery_index", "restart_networks", "restart_node", "resubmit_join_request", "retry_discovery_index", "review_join_request", "revoke_invitation", "rollback_configuration", "rollback_policy", "rollback_traffic_policy", "rotate_node_identity", "set_local_exit_capability", "set_network_role", "set_node_display_name", "shutdown", "start_networks", "start_node", "stop_networks", "stop_node", "submit_join_request", "test_policy", "test_traffic_policy", "unban_client", "unpublish_network_discovery", "update_network_allocation", "update_network_discovery", "update_network_profile", "update_routing", "withdraw_join_request"] as const;
export type NodeManifestActionCommandKind =
  (typeof NODE_MANIFEST_ACTION_COMMAND_KINDS)[number];

export type AclAction = "allow" | "deny";

export type ActivityCategory = "user_activity" | "system_event" | "security_event" | "operation_failure";

export type ActivityEntry = {
  "action": string;
  "actor": NodeId;
  "actor_role"?: string | null;
  "audit_id": AuditId;
  "category"?: ActivityCategory;
  "details": Record<string, string>;
  "network_id"?: NetworkId | null;
  "object_id"?: string | null;
  "object_kind"?: string;
  "occurred_at_unix_ms": number;
  "operation_id"?: OperationId | null;
  "outcome": AuditOutcome;
  "reason"?: string | null;
  "resource": string;
  "severity"?: ActivitySeverity;
  "summary": string;
};

export type ActivitySeverity = "info" | "notice" | "warning" | "critical";

export type ApplicationIdentity = {
  "kind": "executable_path";
  "value": string;
} | {
  "kind": "package_id";
  "value": string;
} | {
  "kind": "unknown";
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

export type ConfirmedTrafficDirectFallback = {
  "allowed_failures": Array<TrafficFailureKind>;
  "confirmed_at_unix_ms": number;
  "confirmed_by": TrafficDefaultConfirmedBy;
};

export type ContentHash = string;

export type ContributionKind = "relay" | "exit" | "file";

export type DiscoveryTransport = "quic" | "tcp" | "websocket" | "local";

export type DisplayEnvironment = "linux_x11" | "linux_wayland" | "windows_desktop" | "macos_desktop" | "headless";

export type DomainActorRef = {
  "display": string;
  "node_id": NodeId;
};

export type DomainName = string;

export type DomainObjectRef = {
  "id": string;
  "kind": ObjectKind;
};

export type DomainSuffix = string;

export type EffectiveAccess = {
  "allowed": boolean;
  "code"?: string | null;
  "evaluated_at_unix_ms": number;
  "expired_grant_ids": Array<SecurityRuleId>;
  "layer"?: EffectivePolicyLayer | null;
  "local_policy_revision": number;
  "matched_allow_rule_ids": Array<SecurityRuleId>;
  "matched_deny_rule_ids": Array<SecurityRuleId>;
  "network_acl_revision": number;
  "reason": EffectiveAccessReason;
  "reauthentication_required_grant_ids": Array<SecurityRuleId>;
  "reauthentication_satisfied": boolean;
  "revoked_grant_ids": Array<SecurityRuleId>;
  "rule_id"?: SecurityRuleId | null;
};

export type EffectiveAccessReason = "allowed" | "cross_network_denied" | "network_scope_restricted" | "local_contribution_veto" | "authorization_expired" | "authorization_revoked" | "recent_reauthentication_required" | "explicit_deny" | "role_ceiling_exceeded" | "temporary_coordinator_read_only" | "default_denied" | "invalid_policy";

export type EffectivePolicyLayer = "local" | "network" | "request";

export type ExitAvailabilitySource = "not_observed" | "local_runtime" | "coordinator_health" | "remote_peer_report";

export type ExitAvailabilityState = "unknown" | "available" | "degraded" | "unavailable";

export type ExitContributionAvailability = {
  "declared_available"?: boolean | null;
  "observed_at_unix_ms"?: number | null;
  "reason_code"?: string | null;
  "source": ExitAvailabilitySource;
  "state": ExitAvailabilityState;
};

export type ExitContributionMemberScope = {
  "kind": "all_members";
} | {
  "groups"?: Array<GroupId>;
  "kind": "selected";
  "nodes"?: Array<NodeId>;
  "roles"?: Array<MembershipRole>;
};

export type ExitContributionMode = "autonomous" | "required_on_join" | "disabled" | "approval_required";

export type ExitContributionObligation = "required" | "not_required";

export type ExitContributionQuotaPeriod = "daily" | "weekly";

export type ExitContributionRestrictions = {
  "max_bandwidth_kbps"?: number | null;
  "members"?: ExitContributionMemberScope;
  "time_windows_utc"?: Array<ExitContributionTimeWindow>;
  "traffic_quota"?: ExitContributionTrafficQuota | null;
};

export type ExitContributionScope = "not_applicable" | "joining_members" | "all_members";

export type ExitContributionTimeWindow = {
  "end_minute_utc": number;
  "start_minute_utc": number;
  "weekdays": Array<number>;
};

export type ExitContributionTrafficQuota = {
  "max_bytes": number;
  "period": ExitContributionQuotaPeriod;
};

export type ExitHealthAssessment = {
  "affected_layers": Array<ExitHealthLayer>;
  "affected_protocols": Array<ExitHealthProtocol>;
  "affected_targets": Array<ExitHealthTarget>;
  "default_candidate": boolean;
  "fresh_evidence_count": number;
  "observed_at_unix_ms"?: number | null;
  "reason_code": string;
  "source"?: ExitHealthSource | null;
  "state": ExitHealthState;
};

export type ExitHealthEvidence = {
  "duration_ms": number;
  "error_code"?: string | null;
  "layer": ExitHealthLayer;
  "observed_at_unix_ms": number;
  "protocol"?: ExitHealthProtocol | null;
  "source": ExitHealthSource;
  "state": ExitHealthEvidenceState;
  "target"?: ExitHealthTarget | null;
};

export type ExitHealthEvidenceState = "passed" | "degraded" | "failed" | "unsupported" | "not_tested" | "unknown";

export type ExitHealthLayer = "process" | "listening" | "network" | "dns" | "internet_reachability";

export type ExitHealthProtocol = "dns" | "tcp" | "udp" | "quic" | "ipv4" | "ipv6";

export type ExitHealthRetest = {
  "finished_at_unix_ms"?: number | null;
  "phase"?: ExitHealthRetestPhase;
  "requested_at_unix_ms"?: number;
  "started_at_unix_ms"?: number | null;
};

export type ExitHealthRetestPhase = "idle" | "requested" | "running" | "cancelled" | "completed" | "failed";

export type ExitHealthSnapshot = {
  "evidence"?: Array<ExitHealthEvidence>;
  "max_age_ms"?: number;
  "retest"?: ExitHealthRetest;
};

export type ExitHealthSource = "local_process" | "local_socket" | "local_network" | "local_dns" | "controlled_probe" | "coordinator_health" | "remote_peer_report";

export type ExitHealthState = "unknown" | "available" | "degraded" | "unavailable";

export type ExitHealthTarget = {
  "kind": "dns_resolver";
} | {
  "kind": "ipv4_internet";
} | {
  "kind": "ipv6_internet";
} | {
  "kind": "tcp_internet";
} | {
  "kind": "udp_internet";
} | {
  "kind": "quic_internet";
} | {
  "id": string;
  "kind": "controlled";
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

export type ExitSelectionCandidate = {
  "availability": ExitContributionAvailability;
  "exit": NodeId;
  "is_default": boolean;
  "reason_code": string;
  "selectable": boolean;
  "source": ExitSelectionSource;
};

export type ExitSelectionGroupRule = {
  "allow"?: Array<NodeId>;
  "deny"?: Array<NodeId>;
  "group_id": GroupId;
  "members"?: Array<NodeId>;
};

export type ExitSelectionMemberRule = {
  "allow"?: Array<NodeId>;
  "deny"?: Array<NodeId>;
  "node_id": NodeId;
};

export type ExitSelectionMode = {
  "mode": "member_choice";
} | {
  "exit": NodeId;
  "mode": "fixed";
};

export type ExitSelectionPolicy = {
  "available_exits"?: Array<NodeId>;
  "default_exit"?: NodeId | null;
  "denied_exits"?: Array<NodeId>;
  "groups"?: Array<ExitSelectionGroupRule>;
  "members"?: Array<ExitSelectionMemberRule>;
  "mode"?: ExitSelectionMode;
  "session_transition"?: ExitSessionTransitionPolicy;
};

export type ExitSelectionResolution = {
  "candidates": Array<ExitSelectionCandidate>;
  "default_exit"?: NodeId | null;
  "fixed_exit"?: NodeId | null;
  "mode": ExitSelectionMode;
  "policy_revision": number;
  "selectable_exits": Array<NodeId>;
  "selected_default"?: NodeId | null;
  "subject": NodeId;
};

export type ExitSelectionSource = {
  "scope": "network";
} | {
  "group_id": GroupId;
  "scope": "group";
} | {
  "scope": "member";
} | {
  "scope": "selection_mode";
} | {
  "scope": "availability";
};

export type ExitSessionTransitionPolicy = "migrate" | "terminate" | "fail_closed";

export type FieldPrivacyPolicy = {
  "effective_at_unix_ms": number;
  "expires_at_unix_ms": number;
  "issued_at_unix_ms": number;
  "network_id"?: NetworkId | null;
  "revision": number;
  "rules"?: Array<FieldVisibilityRule>;
};

export type FieldVisibility = "public" | "network" | "administrators" | "owner_only";

export type FieldVisibilityRule = {
  "field": PrivacyField;
  "visibility": FieldVisibility;
};

export type GroupId = string;

export type IdempotencyKey = string;

export type InvitationCode = string;

export type InvitationId = string;

export type InvitationPreview = {
  "can_accept": boolean;
  "capabilities": Array<CapabilityKind>;
  "creates_permanent_membership": boolean;
  "denial_key"?: string | null;
  "expires_at_unix_ms": number;
  "invitation_id": InvitationId;
  "invitation_revision": number;
  "network_id": NetworkId;
  "network_name": string;
  "network_revision": number;
  "permission_changes": Array<string>;
  "preview_digest": ContentHash;
  "remaining_uses": number;
  "requires_recent_reauthentication": boolean;
  "role": MembershipRole;
  "state": InvitationState;
  "terms": MembershipTerms;
  "terms_hash": ContentHash;
};

export type InvitationRecord = {
  "capabilities"?: Array<CapabilityKind>;
  "created_at_unix_ms": number;
  "created_by": NodeId;
  "expires_at_unix_ms": number;
  "invitation_id": InvitationId;
  "max_uses": number;
  "network_id": NetworkId;
  "revision": number;
  "revoked_at_unix_ms"?: number | null;
  "revoked_by"?: NodeId | null;
  "role": MembershipRole;
  "used_count": number;
};

export type InvitationRedeemRequest = {
  "acknowledged_at_unix_ms": number;
  "code": InvitationCode;
  "exit_contribution_acknowledged"?: boolean | null;
  "expected_invitation_revision": number;
  "invitation_id": InvitationId;
  "network_id": NetworkId;
  "preview_digest": ContentHash;
  "terms_hash": ContentHash;
};

export type InvitationRedemption = {
  "already_redeemed": boolean;
  "capabilities": Array<CapabilityKind>;
  "grants_permanent_membership": boolean;
  "invitation_id": InvitationId;
  "invitation_revision": number;
  "invited_by": NodeId;
  "joined_at_unix_ms": number;
  "network_id": NetworkId;
  "network_revision": number;
  "node_id": NodeId;
  "role": MembershipRole;
};

export type InvitationState = "active" | "exhausted" | "expired" | "revoked";

export type InvitationStatusView = {
  "invitation": InvitationRecord;
  "remaining_uses": number;
  "state": InvitationState;
};

export type IpCidr = string;

export type IssuedInvitation = {
  "code": InvitationCode;
  "status": InvitationStatusView;
};

export type JoinApplicantProfile = {
  "display_name"?: string | null;
  "node_id": NodeId;
  "public_key_fingerprint": string;
};

export type JoinRequest = {
  "applicant": JoinApplicantProfile;
  "approved_at_unix_ms"?: number | null;
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
  "exit_contribution_mode"?: ExitContributionMode;
  "exit_contribution_scope"?: ExitContributionScope;
  "file_access"?: boolean;
  "grants_data_plane_access"?: boolean;
  "role": MembershipRole;
};

export type JoinRequestId = string;

export type JoinRequestReasonVisibility = "owners_and_administrators";

export type JoinRequestReviewAction = "approve" | "reject" | "request_information";

export type JoinRequestState = "pending" | "needs_information" | "approved" | "rejected" | "withdrawn" | "expired";

export type LocalExitCapabilitySnapshot = {
  "active_clients"?: number;
  "healthy"?: boolean;
  "last_transition_at_unix_ms"?: number;
  "network_allowed"?: boolean;
  "owner_exit_enabled"?: boolean;
  "problem"?: NodeProblem | null;
  "stage"?: LocalExitCapabilityStage;
  "total_clients"?: number;
};

export type LocalExitCapabilityStage = "disabled" | "waiting_for_network_authorization" | "listening" | "health_checking" | "enabled" | "draining" | "cleaning" | "failed";

export type MemberLifecycleAction = "invite" | "approve" | "reject" | "suspend" | "resume" | "exit" | "remove" | "undo_remove";

export type MemberLifecycleChange = {
  "access_epoch": number;
  "access_revoked": boolean;
  "action": MemberLifecycleAction;
  "audit_id": AuditId;
  "idempotent": boolean;
  "network_id": NetworkId;
  "operation_id": OperationId;
  "requires_reapproval": boolean;
  "requires_reauthentication": boolean;
  "revision": number;
  "status": MemberLifecycleStatus;
  "target_node_id": NodeId;
};

export type MemberLifecycleStatus = "invited" | "pending_approval" | "approved" | "active" | "degraded" | "suspended" | "exited" | "removed" | "expired" | "rejected";

export type MemberRemovalPreview = {
  "access_epoch": number;
  "active_capabilities"?: Array<CapabilityKind>;
  "can_remove": boolean;
  "credentials_revoked": boolean;
  "denial_key"?: string | null;
  "display_name"?: string | null;
  "file_access_revoked": boolean;
  "impact_keys": Array<string>;
  "incomplete_transfers_resume_blocked": boolean;
  "lifecycle_state": MemberLifecycleStatus;
  "local_files_deleted": boolean;
  "network_access_revoked": boolean;
  "network_id": NetworkId;
  "old_access_epoch_invalidated": boolean;
  "presence": NetworkMemberPresence;
  "revision": number;
  "role": MembershipRole;
  "safe_undo_requires_reapproval": boolean;
  "safe_undo_restores_old_credentials": boolean;
  "status": NetworkMemberStatus;
  "target_node_id": NodeId;
};

export type MembershipRole = "owner" | "administrator" | "member";

export type MembershipState = "discovered" | "applying" | "pending_approval" | "accepted" | "key_ready" | "connecting" | "active" | "degraded" | "leaving" | "left" | "rejected" | "suspended" | "cancelled";

export type MembershipTerms = {
  "exit_contribution": ExitContributionObligation;
  "exit_contribution_mode"?: ExitContributionMode;
  "exit_contribution_scope"?: ExitContributionScope;
  "member_limit"?: number | null;
  "network_policy_revision": number;
  "privacy_policy_revision": number;
};

export type NetworkAcl = {
  "expires_at_unix_ms": number;
  "grants"?: Array<NetworkAclGrant>;
  "issued_at_unix_ms": number;
  "network_id": NetworkId;
  "revision": number;
  "rules"?: Array<PermissionRule>;
};

export type NetworkAclGrant = {
  "actions": Array<PolicyAction>;
  "expires_at_unix_ms": number;
  "grant_id": SecurityRuleId;
  "issued_at_unix_ms": number;
  "requires_recent_reauthentication"?: boolean;
  "resources": Array<ResourceKind>;
  "revoked_at_unix_ms"?: number | null;
  "revoked_by"?: NodeId | null;
  "subjects": Array<SubjectSelector>;
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
  "exit_contribution_mode"?: ExitContributionMode;
  "exit_contribution_scope"?: ExitContributionScope;
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

export type NetworkMember = {
  "access_epoch": number;
  "available_actions"?: Array<MemberLifecycleAction>;
  "capabilities"?: Array<CapabilityKind> | null;
  "display_name"?: string | null;
  "exit_availability"?: ExitContributionAvailability;
  "groups": Array<NetworkMemberGroup>;
  "identity_fingerprint"?: string | null;
  "joined_at_unix_ms"?: number | null;
  "lifecycle_state": MemberLifecycleStatus;
  "node_id": NodeId;
  "presence": NetworkMemberPresence;
  "revision": number;
  "role": MembershipRole;
  "status": NetworkMemberStatus;
};

export type NetworkMemberDirectory = {
  "can_manage_members": boolean;
  "exit_selection_policy"?: ExitSelectionPolicy;
  "exit_selection_policy_revision"?: number;
  "generated_at_unix_ms": number;
  "members": Array<NetworkMember>;
  "network_id": NetworkId;
  "viewer_role": MembershipRole;
};

export type NetworkMemberGroup = "owner" | "administrator" | "member";

export type NetworkMemberObservationSource = "live_session" | "disconnect_observed" | "not_observed";

export type NetworkMemberOnlineState = "online" | "offline" | "unknown";

export type NetworkMemberPresence = {
  "observed_at_unix_ms"?: number | null;
  "source": NetworkMemberObservationSource;
  "state": NetworkMemberOnlineState;
};

export type NetworkMemberProfileProjection = {
  "administrator_count": number;
  "member_count": number;
  "profile": NetworkPublicProfileDocument;
};

export type NetworkMemberStatus = "pending" | "approved" | "active" | "degraded" | "suspended" | "exited" | "removed" | "expired";

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
  "exit_contribution"?: ExitContributionObligation;
  "exit_contribution_mode"?: ExitContributionMode;
  "exit_contribution_scope"?: ExitContributionScope;
  "exit_selection_policy"?: ExitSelectionPolicy | null;
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

export type NetworkRole = "owner" | "administrator" | "member" | "temporary_coordinator";

export type NetworkVisibility = "public" | "unlisted" | "private";

export type NodeAccessControlSnapshot = {
  "field_privacy_policies"?: Array<FieldPrivacyPolicy>;
  "local_policy"?: NodeLocalAccessPolicy | null;
  "network_acls"?: Array<NetworkAcl>;
};

export type NodeAccessModel = "trusted_control_plane";

export type NodeAcknowledgeMembershipPolicyCommand = {
  "acknowledged_at_unix_ms": number;
  "exit_contribution_acknowledged"?: boolean | null;
  "expected_terms_hash": ContentHash;
  "network_id": string;
};

export type NodeApiCommandResponse = {
  "operation"?: NodeOperationSnapshot | null;
  "request_id": number;
  "result"?: ProjectedNodeCommandResult | null;
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
  "snapshot": ProjectedNodeSnapshot;
  "type": "snapshot";
} | {
  "event": ProjectedNodeEvent;
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
  "identity": ProjectedNodeIdentitySnapshot;
  "kind": "node";
} | {
  "id": string;
  "kind": "network";
  "network": ProjectedNodeNetworkSnapshot;
} | {
  "id": string;
  "kind": "peer";
  "peer": ProjectedNodePeerSnapshot;
} | {
  "id": string;
  "kind": "embedded_service";
  "service": ProjectedNodeEmbeddedServiceSnapshot;
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
  "snapshot": ProjectedNodeSnapshot;
};

export type NodeApplyConfigurationCommand = {
  "expected_revision": number;
  "networks": Record<string, unknown>;
};

export type NodeApplyPolicyCommand = {
  "candidate": PolicySnapshot;
  "confirm_privilege_expansion"?: boolean;
  "expected_revision": number;
  "includes_local_policy"?: boolean;
  "network_ids"?: Array<string>;
};

export type NodeApplyTrafficPolicyCommand = {
  "confirm_risk"?: boolean;
  "draft": TrafficPolicyDraft;
  "expected_revision": number;
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

export type NodeChangeMemberLifecycleCommand = {
  "action": MemberLifecycleAction;
  "expected_revision": number;
  "network_id": string;
  "reason"?: string | null;
  "role"?: MembershipRole;
  "target_node_id": string;
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
  "command": "list_network_members";
  "payload": NodeNetworkMembersCommand;
} | {
  "command": "preview_member_removal";
  "payload": NodeMemberRemovalPreviewCommand;
} | {
  "command": "change_member_lifecycle";
  "payload": NodeChangeMemberLifecycleCommand;
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
  "command": "create_invitation";
  "payload": NodeCreateInvitationCommand;
} | {
  "command": "list_invitations";
  "payload": NodeListInvitationsCommand;
} | {
  "command": "get_invitation_status";
  "payload": NodeGetInvitationStatusCommand;
} | {
  "command": "revoke_invitation";
  "payload": NodeRevokeInvitationCommand;
} | {
  "command": "preview_invitation";
  "payload": NodePreviewInvitationCommand;
} | {
  "command": "redeem_invitation";
  "payload": NodeRedeemInvitationCommand;
} | {
  "command": "leave_network";
  "payload": NodeLeaveNetworkCommand;
} | {
  "command": "acknowledge_membership_policy";
  "payload": NodeAcknowledgeMembershipPolicyCommand;
} | {
  "command": "update_routing";
  "payload": NodeUpdateRoutingCommand;
} | {
  "command": "set_network_role";
  "payload": NodeSetNetworkRoleCommand;
} | {
  "command": "set_local_exit_capability";
  "payload": NodeSetLocalExitCapabilityCommand;
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
} | {
  "command": "get_policy";
} | {
  "command": "preview_policy";
  "payload": NodePreviewPolicyCommand;
} | {
  "command": "preview_policy_rollback";
  "payload": NodePreviewPolicyRollbackCommand;
} | {
  "command": "apply_policy";
  "payload": NodeApplyPolicyCommand;
} | {
  "command": "rollback_policy";
  "payload": NodeRollbackPolicyCommand;
} | {
  "command": "get_policy_history";
} | {
  "command": "test_policy";
  "payload": PolicyTestInput;
} | {
  "command": "get_traffic_policy";
} | {
  "command": "preview_traffic_policy";
  "payload": NodePreviewTrafficPolicyCommand;
} | {
  "command": "preview_traffic_policy_rollback";
  "payload": NodePreviewTrafficPolicyRollbackCommand;
} | {
  "command": "apply_traffic_policy";
  "payload": NodeApplyTrafficPolicyCommand;
} | {
  "command": "rollback_traffic_policy";
  "payload": NodeRollbackTrafficPolicyCommand;
} | {
  "command": "get_traffic_policy_history";
} | {
  "command": "test_traffic_policy";
  "payload": NodeTestTrafficPolicyCommand;
};

export type NodeCommandHttpRequest = {
  "idempotency_key"?: IdempotencyKey | null;
  "request": NodeCommandRequest;
  "schema_version": number;
};

export type NodeCommandHttpResponse = {
  "result": ProjectedNodeCommandResult;
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
  "data": NetworkMemberDirectory;
  "status": "network_members";
} | {
  "data": MemberRemovalPreview;
  "status": "member_removal_preview";
} | {
  "data": MemberLifecycleChange;
  "status": "member_lifecycle_changed";
} | {
  "data": NodeJoinRequestReviewAck;
  "status": "join_requests_reviewed";
} | {
  "data": JoinRequest;
  "status": "join_request_updated";
} | {
  "data": IssuedInvitation;
  "status": "invitation_created";
} | {
  "data": Array<InvitationStatusView>;
  "status": "invitations";
} | {
  "data": InvitationStatusView;
  "status": "invitation_status";
} | {
  "data": InvitationStatusView;
  "status": "invitation_revoked";
} | {
  "data": InvitationPreview;
  "status": "invitation_preview";
} | {
  "data": InvitationRedemption;
  "status": "invitation_redeemed";
} | {
  "data": NodeNetworkLeaveResult;
  "status": "network_left";
} | {
  "data": NodeNetworkMembershipSnapshot;
  "status": "membership_policy_acknowledged";
} | {
  "data": LocalExitCapabilitySnapshot;
  "status": "local_exit_capability";
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
  "data": PolicyCurrent;
  "status": "policy";
} | {
  "data": PolicyPreview;
  "status": "policy_preview";
} | {
  "data": PolicyApplyResult;
  "status": "policy_applied";
} | {
  "data": PolicyHistorySnapshot;
  "status": "policy_history";
} | {
  "data": PolicyRollbackResult;
  "status": "policy_rolled_back";
} | {
  "data": PolicyTestResult;
  "status": "policy_test";
} | {
  "data": TrafficPolicyCurrent;
  "status": "traffic_policy";
} | {
  "data": TrafficPolicyPreview;
  "status": "traffic_policy_preview";
} | {
  "data": TrafficPolicyApplyResult;
  "status": "traffic_policy_applied";
} | {
  "data": TrafficPolicyHistorySnapshot;
  "status": "traffic_policy_history";
} | {
  "data": TrafficPolicyRollbackResult;
  "status": "traffic_policy_rolled_back";
} | {
  "data": TrafficPolicyTestComparison;
  "status": "traffic_policy_test";
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

export type NodeContributionVeto = {
  "contributions": Array<ContributionKind>;
  "network_id": NetworkId;
  "veto_id": SecurityRuleId;
};

export type NodeControlHealth = "unknown" | "available" | "unavailable";

export type NodeCreateInvitationCommand = {
  "capabilities"?: Array<CapabilityKind>;
  "expires_in_ms": number;
  "max_uses": number;
  "network_id": NetworkId;
  "role": MembershipRole;
};

export type NodeCreateNetworkCommand = {
  "auto_connect": boolean;
  "capability_declarations"?: Array<CapabilityKind>;
  "exit_contribution"?: ExitContributionObligation;
  "exit_contribution_mode"?: ExitContributionMode;
  "exit_contribution_scope"?: ExitContributionScope;
  "exit_selection_policy"?: ExitSelectionPolicy;
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
  "exit_health"?: ExitHealthSnapshot;
  "exit_health_assessment"?: ExitHealthAssessment;
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

export type NodeDiscoveryIndexErrorCategory = "offline" | "timeout" | "tls" | "signature" | "partial_corruption" | "incompatible_version" | "unsupported" | "rejected" | "invalid_response" | "storage" | "server" | "cancelled";

export type NodeDiscoveryIndexMetrics = {
  "cancellations": number;
  "last_request_duration_ms"?: number | null;
  "records_accepted": number;
  "records_rejected": number;
  "request_attempts": number;
  "requests_failed": number;
  "requests_succeeded": number;
  "retries_scheduled": number;
};

export type NodeDiscoveryIndexRecordSnapshot = {
  "accepted_at_unix_ms": number;
  "digest": string;
  "discoverable": boolean;
  "invalid_reason"?: string | null;
  "record": NetworkDiscoveryRecord;
  "source": NodeDiscoveryIndexSource;
};

export type NodeDiscoveryIndexSnapshot = {
  "data_plane_affected": boolean;
  "index_url"?: string | null;
  "last_attempt_unix_ms"?: number | null;
  "last_error"?: NodeDiscoveryIndexError | null;
  "last_success_unix_ms"?: number | null;
  "metrics": NodeDiscoveryIndexMetrics;
  "records": Array<NodeDiscoveryIndexRecordSnapshot>;
  "rejected_record_count": number;
  "search_history_stored": boolean;
  "source": NodeDiscoveryIndexSource;
  "state": NodeDiscoveryIndexState;
};

export type NodeDiscoveryIndexSource = "none" | "cache" | "network" | "join";

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

export type NodeExitSelectionSnapshot = {
  "directory_generated_at_unix_ms"?: number | null;
  "directory_received_at_unix_ms"?: number | null;
  "evaluated_at_unix_ms": number;
  "network_id": NetworkId;
  "policy"?: ExitSelectionPolicy | null;
  "policy_revision": number;
  "reason_code"?: string | null;
  "resolution"?: ExitSelectionResolution | null;
};

export type NodeGetInvitationStatusCommand = {
  "invitation_id": InvitationId;
  "network_id": NetworkId;
};

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
  "exit_contribution_acknowledged"?: boolean | null;
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

export type NodeListInvitationsCommand = {
  "network_id": NetworkId;
};

export type NodeListJoinRequestsCommand = {
  "network_id": string;
};

export type NodeLocalAccessPolicy = {
  "contribution_vetoes"?: Array<NodeContributionVeto>;
  "exit_contribution"?: ExitContributionRestrictions | null;
  "expires_at_unix_ms": number;
  "issued_at_unix_ms": number;
  "network_scope": NodeNetworkScope;
  "node_id": NodeId;
  "revision": number;
  "rules"?: Array<PermissionRule>;
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

export type NodeMemberRemovalPreviewCommand = {
  "network_id": string;
  "target_node_id": string;
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
  "can_join_without_local_exit": boolean;
  "capability_summary": Array<CapabilityKind>;
  "contribution_policy_key": string;
  "discovery_revision": number;
  "endpoints": Array<NetworkEndpoint>;
  "exit_contribution": ExitContributionObligation;
  "exit_contribution_bandwidth_impact_key": string;
  "exit_contribution_disable_key": string;
  "exit_contribution_logging_boundary_key": string;
  "exit_contribution_mode": ExitContributionMode;
  "exit_contribution_operational_impact_key": string;
  "exit_contribution_purpose_key": string;
  "exit_contribution_scope": ExitContributionScope;
  "exit_contribution_traffic_impact_key": string;
  "expires_at_unix_ms": number;
  "issued_at_unix_ms": number;
  "join_mode": NetworkJoinMode;
  "join_requires_exit_consent": boolean;
  "local_exit_available": boolean;
  "local_exit_enabled": boolean;
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

export type NodeNetworkMembersCommand = {
  "network_id": string;
};

export type NodeNetworkMembershipSnapshot = {
  "acknowledged_at_unix_ms"?: number | null;
  "created_at_unix_ms": number;
  "current_terms"?: MembershipTerms | null;
  "current_terms_hash"?: string | null;
  "exit_contribution"?: ExitContributionObligation | null;
  "exit_contribution_mode"?: ExitContributionMode | null;
  "exit_contribution_scope"?: ExitContributionScope | null;
  "local_policy_revision": number;
  "member_limit"?: number | null;
  "network_id": string;
  "network_name": string;
  "network_policy_revision"?: number | null;
  "node_id": string;
  "policy_confirmation_required"?: boolean;
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
  "exit_contribution": ExitContributionObligation;
  "exit_contribution_mode": ExitContributionMode;
  "exit_contribution_scope": ExitContributionScope;
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

export type NodeNetworkScope = {
  "kind": "all";
} | {
  "kind": "only";
  "network_ids": Array<NetworkId>;
};

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
  "result"?: ProjectedNodeCommandResult | null;
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

export type NodePreviewInvitationCommand = {
  "code": InvitationCode;
  "coordinator_url": string;
  "network_id": NetworkId;
};

export type NodePreviewJoinNetworkCommand = {
  "network_id": string;
};

export type NodePreviewPolicyCommand = {
  "candidate": PolicySnapshot;
  "expected_revision": number;
};

export type NodePreviewPolicyRollbackCommand = {
  "expected_revision": number;
  "target_revision": number;
};

export type NodePreviewTrafficPolicyCommand = {
  "draft": TrafficPolicyDraft;
  "expected_revision": number;
};

export type NodePreviewTrafficPolicyRollbackCommand = {
  "expected_revision": number;
  "target_revision": number;
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

export type NodeProblemCode = "authentication_required" | "invalid_request" | "not_found" | "conflict" | "unavailable" | "permission_denied" | "cancelled" | "timed_out" | "internal" | "resource_exhausted" | "unsupported";

export type NodeProblemDetails = {
  "current_revision"?: number | null;
  "expected_revision"?: number | null;
  "validation_issues"?: Array<NodeValidationIssue>;
};

export type NodeRedeemInvitationCommand = {
  "coordinator_url": string;
  "request": InvitationRedeemRequest;
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

export type NodeRevokeInvitationCommand = {
  "expected_revision": number;
  "invitation_id": InvitationId;
  "network_id": NetworkId;
};

export type NodeRollbackConfigurationCommand = {
  "expected_revision": number;
  "target_revision": number;
};

export type NodeRollbackPolicyCommand = {
  "confirm_privilege_expansion"?: boolean;
  "expected_revision": number;
  "includes_local_policy"?: boolean;
  "network_ids"?: Array<string>;
  "target_revision": number;
};

export type NodeRollbackTrafficPolicyCommand = {
  "confirm_risk"?: boolean;
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

export type NodeSetLocalExitCapabilityCommand = {
  "enabled": boolean;
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
  "access_control"?: NodeAccessControlSnapshot;
  "capabilities"?: NodeCapabilityRegistrySnapshot;
  "configuration"?: NodeConfigurationSnapshot;
  "control_health"?: NodeControlHealth;
  "discovery_index"?: NodeDiscoveryIndexSnapshot;
  "embedded_services"?: Array<NodeEmbeddedServiceSnapshot>;
  "exit_node_capabilities"?: ExitNodeCapabilities;
  "exit_selections"?: Array<NodeExitSelectionSnapshot>;
  "identity"?: NodeIdentitySnapshot | null;
  "interface_problems"?: Array<NodeProblem>;
  "interfaces"?: Array<NodeInterfaceSnapshot>;
  "last_error"?: NodeProblem | null;
  "lifecycle": NodeLifecycleSnapshot;
  "local_exit"?: LocalExitCapabilitySnapshot;
  "logs": Array<NodeLogEntry>;
  "memberships"?: Array<NodeNetworkMembershipSnapshot>;
  "network_profiles"?: Array<NodeNetworkProfileSnapshot>;
  "networks": Array<NodeNetworkSnapshot>;
  "peers": Array<NodePeerSnapshot>;
  "permissions"?: NodePermissionSnapshot;
  "recent_activity"?: Array<ActivityEntry>;
  "revision": number;
  "routing_summaries"?: Array<NodeRoutingSummarySnapshot>;
  "topology"?: TopologySnapshot | null;
  "traffic_routing_page"?: TrafficRoutingPageModel | null;
  "tun_capabilities"?: TunCapabilities;
  "ui_mode"?: UiModeResolution | null;
};

export type NodeSnapshotResponse = {
  "schema_version": number;
  "snapshot": ProjectedNodeSnapshot;
};

export type NodeSubmitJoinRequestCommand = {
  "network_id": string;
  "reason": string;
};

export type NodeTestTrafficPolicyCommand = {
  "candidate": TrafficPolicy;
  "current": TrafficPolicy;
  "input": TrafficPolicyTestInput;
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
  "command": "get_snapshot" | "get_node_diagnostics" | "start_networks" | "stop_networks" | "restart_networks" | "rebuild_tun" | "create_network" | "preview_network_profile" | "update_network_profile" | "publish_network_discovery" | "update_network_discovery" | "unpublish_network_discovery" | "refresh_discovery_index" | "retry_discovery_index" | "preview_join_network" | "join_network" | "submit_join_request" | "list_join_requests" | "list_network_members" | "preview_member_removal" | "change_member_lifecycle" | "review_join_request" | "batch_review_join_requests" | "withdraw_join_request" | "resubmit_join_request" | "create_invitation" | "list_invitations" | "get_invitation_status" | "revoke_invitation" | "preview_invitation" | "redeem_invitation" | "leave_network" | "acknowledge_membership_policy" | "update_routing" | "set_network_role" | "set_local_exit_capability" | "set_node_display_name" | "rotate_node_identity" | "get_blacklist" | "ban_client" | "unban_client" | "get_network_allocation" | "update_network_allocation" | "probe_peer" | "start_node" | "stop_node" | "restart_node" | "cleanup_node_residuals" | "get_configuration" | "preview_configuration" | "apply_configuration" | "rollback_configuration" | "get_configuration_history" | "get_policy" | "preview_policy" | "preview_policy_rollback" | "apply_policy" | "rollback_policy" | "get_policy_history" | "test_policy" | "get_traffic_policy" | "preview_traffic_policy" | "preview_traffic_policy_rollback" | "apply_traffic_policy" | "rollback_traffic_policy" | "get_traffic_policy_history" | "test_traffic_policy" | "shutdown";
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

export type NodeUiInputKind = "boolean" | "client_id" | "configuration_json" | "policy_json" | "policy_snapshot_json" | "traffic_policy_json" | "traffic_policy_test_json" | "policy_network_ids" | "display_name" | "external_host" | "fingerprint" | "network_mode" | "network_name" | "network_selection" | "peer_target" | "purpose" | "icon" | "language" | "region" | "tags" | "capability_declarations" | "visibility" | "join_mode" | "exit_contribution_mode" | "exit_contribution" | "exit_contribution_scope" | "exit_selection_policy_json" | "join_request_id" | "join_request_note" | "join_request_reason" | "join_request_review_action" | "join_request_reviews" | "node_id" | "member_lifecycle_action" | "member_lifecycle_reason" | "invitation_code" | "coordinator_url" | "port" | "revision" | "role" | "routing_mode" | "url_or_ticket" | "discovery_index_url" | "timeout_ms";

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

export type NodeUiValidation = "none" | "non_empty" | "network_name" | "port" | "distinct_ports" | "network_mode" | "network_selection" | "role" | "routing_mode" | "positive_timeout" | "revision" | "configuration_json" | "policy_json" | "policy_snapshot_json" | "traffic_policy_json" | "traffic_policy_test_json" | "policy_network_ids" | "display_name" | "fingerprint" | "network_purpose" | "network_icon" | "network_language" | "region" | "tags" | "capability_declarations" | "visibility" | "join_mode" | "exit_contribution_mode" | "exit_contribution" | "exit_contribution_scope" | "exit_selection_policy_json" | "join_request_id" | "join_request_note" | "join_request_reason" | "join_request_review_action" | "join_request_reviews" | "node_id" | "member_lifecycle_action" | "member_lifecycle_reason" | "invitation_code" | "coordinator_url";

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
  "snapshot": ProjectedNodeSnapshot;
  "type": "snapshot";
} | {
  "event": ProjectedNodeEvent;
  "schema_version": number;
  "type": "event";
} | {
  "reason": NodeWsResyncReason;
  "schema_version": number;
  "snapshot": ProjectedNodeSnapshot;
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

export type OperationKind = "node_start" | "node_stop" | "node_restart" | "node_cleanup" | "diagnostics" | "network_create" | "network_profile_update" | "discovery_index_update" | "network_apply" | "tun_rebuild" | "network_leave" | "membership_approve" | "membership_reject" | "membership_remove" | "membership_invite" | "membership_suspend" | "membership_resume" | "membership_exit" | "membership_undo_remove" | "configuration_history" | "policy_test" | "policy_apply" | "policy_rollback" | "traffic_rules_apply" | "network_role_update" | "identity_update" | "client_blacklist_update" | "network_allocation_update" | "capability_enable" | "capability_disable" | "exit_select" | "exit_revoke" | "coordinator_candidates_update" | "coordinator_recover" | "file_share_update" | "file_upload_start" | "file_download_start" | "owner_passkey_register" | "owner_passkey_revoke" | "certificate_reload";

export type OperationRecovery = {
  "hint_key": string;
  "kind": string;
};

export type OperationState = "queued" | "running" | "waiting_for_reauth" | "succeeded" | "failed" | "cancelled" | "timed_out";

export type PermissionCondition = {
  "kind": "membership_active";
} | {
  "kind": "recent_reauthentication";
} | {
  "kind": "resource_in_network";
  "value": NetworkId;
} | {
  "kind": "non_emergency_coordinator";
};

export type PermissionEffect = "allow" | "deny";

export type PermissionIntent = "read_only" | "mutating";

export type PermissionRule = {
  "actions": Array<PolicyAction>;
  "conditions"?: Array<PermissionCondition>;
  "effect": PermissionEffect;
  "resources": Array<ResourceKind>;
  "rule_id": SecurityRuleId;
  "subjects": Array<SubjectSelector>;
};

export type PolicyAction = "read" | "create" | "update" | "delete" | "approve" | "reject" | "remove" | "enable" | "disable" | "use" | "recover" | "export" | "test";

export type PolicyAffectedObject = {
  "kind": PolicyObjectKind;
  "network_id"?: string | null;
  "object_id"?: string | null;
};

export type PolicyApplyResult = {
  "affected_objects": Array<PolicyAffectedObject>;
  "audit"?: PolicyHistoryEntry | null;
  "diffs": Array<PolicyDiffEntry>;
  "no_op": boolean;
  "previous_revision": number;
  "revision": number;
  "risk": PolicyRiskSummary;
  "snapshot": PolicySnapshot;
};

export type PolicyChangeAction = "apply" | "rollback";

export type PolicyChangeKind = "local_policy_added" | "local_policy_updated" | "local_policy_removed" | "network_acl_added" | "network_acl_updated" | "network_acl_removed" | "permission_rule_added" | "permission_rule_updated" | "permission_rule_removed" | "grant_added" | "grant_updated" | "grant_removed" | "veto_added" | "veto_updated" | "veto_removed" | "network_scope_changed" | "validity_changed";

export type PolicyCurrent = {
  "revision": number;
  "snapshot": PolicySnapshot;
};

export type PolicyDiffEntry = {
  "after"?: unknown;
  "before"?: unknown;
  "change": PolicyChangeKind;
  "path": string;
};

export type PolicyHistoryEntry = {
  "action": PolicyChangeAction;
  "actor": string;
  "affected_objects": Array<PolicyAffectedObject>;
  "diffs": Array<PolicyDiffEntry>;
  "local_policy_revision"?: number | null;
  "network_acl_revisions": Record<string, number>;
  "occurred_at": string;
  "previous_revision": number;
  "revision": number;
  "risk": PolicyRiskSummary;
  "source": string;
  "target_revision"?: number | null;
  "transaction_id": string;
};

export type PolicyHistorySnapshot = {
  "current_revision": number;
  "entries": Array<PolicyHistoryEntry>;
};

export type PolicyObjectKind = "local_policy" | "network_acl" | "permission_rule" | "network_acl_grant" | "contribution_veto" | "network_scope";

export type PolicyPrecedence = "cross_network_denied" | "invalid_or_expired_policy" | "network_scope_restricted" | "local_contribution_veto" | "explicit_deny" | "temporary_coordinator_read_only" | "role_ceiling_exceeded" | "authorization_revoked" | "authorization_expired" | "recent_reauthentication_required" | "default_denied" | "allow";

export type PolicyPrecedenceExplanation = {
  "precedence": PolicyPrecedence;
  "rule_id"?: SecurityRuleId | null;
  "source"?: PolicyRuleSource | null;
  "triggered": boolean;
};

export type PolicyPreview = {
  "affected_objects": Array<PolicyAffectedObject>;
  "can_apply": boolean;
  "candidate": PolicySnapshot;
  "current": PolicySnapshot;
  "current_revision": number;
  "diffs": Array<PolicyDiffEntry>;
  "expected_revision": number;
  "has_changes": boolean;
  "includes_local_policy": boolean;
  "network_ids": Array<string>;
  "revision_conflict": boolean;
  "risk": PolicyRiskSummary;
  "rollback_target_revision"?: number | null;
};

export type PolicyRiskLevel = "none" | "elevated" | "high";

export type PolicyRiskReason = "allow_rule_added" | "deny_rule_removed" | "permission_rule_expanded" | "network_acl_grant_added" | "network_acl_grant_expanded" | "grant_reauthentication_removed" | "grant_validity_extended" | "local_policy_removed" | "network_acl_removed" | "network_scope_expanded" | "contribution_veto_removed" | "policy_validity_extended";

export type PolicyRiskSummary = {
  "level": PolicyRiskLevel;
  "privilege_expansion": boolean;
  "reasons": Array<PolicyRiskReason>;
  "requires_explicit_confirmation": boolean;
  "requires_recent_reauthentication": boolean;
};

export type PolicyRollbackResult = {
  "affected_objects": Array<PolicyAffectedObject>;
  "audit": PolicyHistoryEntry;
  "diffs": Array<PolicyDiffEntry>;
  "previous_revision": number;
  "revision": number;
  "risk": PolicyRiskSummary;
  "snapshot": PolicySnapshot;
  "target_revision": number;
};

export type PolicyRuleExplanation = {
  "actions"?: Array<PolicyAction>;
  "applied": boolean;
  "conditions"?: Array<PermissionCondition>;
  "effect"?: PermissionEffect | null;
  "matched": boolean;
  "precedence": PolicyPrecedence;
  "resources"?: Array<ResourceKind>;
  "rule_id"?: SecurityRuleId | null;
  "source": PolicyRuleSource;
  "source_path": string;
  "subjects"?: Array<SubjectSelector>;
};

export type PolicyRuleSource = "role_baseline" | "local_policy_rule" | "local_contribution_veto" | "local_network_scope" | "network_acl_rule" | "network_acl_grant" | "request" | "access_context";

export type PolicySnapshot = {
  "local_policy"?: NodeLocalAccessPolicy | null;
  "network_acls"?: Array<NetworkAcl>;
};

export type PolicyTestInput = {
  "actor": NodeId;
  "emergency_coordinator": boolean;
  "groups"?: Array<GroupId>;
  "local_policy"?: NodeLocalAccessPolicy | null;
  "membership_active": boolean;
  "network_acl"?: NetworkAcl | null;
  "network_id": NetworkId;
  "now_unix_ms": number;
  "recent_reauthentication": boolean;
  "request": PolicyTestRequest;
  "role": NetworkRole;
};

export type PolicyTestRequest = {
  "action": PolicyAction;
  "intent": PermissionIntent;
  "object_id"?: string | null;
  "resource": ResourceKind;
  "resource_network_id": NetworkId;
};

export type PolicyTestResult = {
  "decision": EffectiveAccess;
  "explanations": Array<PolicyRuleExplanation>;
  "precedence": Array<PolicyPrecedenceExplanation>;
  "validation_issues": Array<PolicyValidationIssue>;
};

export type PolicyValidationIssue = {
  "code": string;
  "message": string;
  "path": string;
  "related_rule_id"?: SecurityRuleId | null;
  "source": PolicyValidationSource;
};

export type PolicyValidationSource = "local_policy" | "network_acl" | "test_input";

export type PortRange = {
  "end": number;
  "start": number;
};

export type PrivacyField = "member_node_id" | "member_display_name" | "member_identity_fingerprint" | "member_role" | "member_groups" | "member_capabilities" | "member_status" | "member_lifecycle_state" | "member_revision" | "member_access_epoch" | "member_available_actions" | "member_presence" | "member_joined_at" | "node_display_name" | "node_public_key" | "node_platform_and_version" | "node_online_state" | "node_endpoints" | "node_real_network_addresses" | "node_traffic_statistics" | "node_local_processes" | "node_local_paths" | "node_detailed_logs" | "topology_node_id" | "topology_display_name" | "topology_role" | "topology_status" | "topology_presence" | "topology_health" | "topology_capabilities" | "topology_network_membership" | "topology_connections" | "activity_actor" | "activity_action" | "activity_resource" | "activity_outcome" | "activity_occurred_at" | "activity_summary" | "activity_details" | "file_id" | "file_network_id" | "file_display_name" | "file_owner" | "file_mode" | "file_revision" | "file_network_acl" | "file_local_acl" | "file_quota" | "file_path_policy" | "file_timestamps" | "file_audit_settings";

export type ProjectedActivityEntry = {
  "action"?: string | null;
  "actor"?: NodeId | null;
  "actor_role"?: string | null;
  "audit_id": AuditId;
  "category": ActivityCategory;
  "details"?: Record<string, string> | null;
  "network_id"?: NetworkId | null;
  "object_id"?: string | null;
  "object_kind"?: string | null;
  "occurred_at_unix_ms"?: number | null;
  "operation_id"?: OperationId | null;
  "outcome"?: AuditOutcome | null;
  "reason"?: string | null;
  "resource"?: string | null;
  "severity": ActivitySeverity;
  "summary"?: string | null;
};

export type ProjectedNetworkMemberDirectory = NetworkMemberDirectory;

export type ProjectedNodeCommandOutcome = {
  "data": ProjectedNodeSnapshot;
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
  "data": ProjectedNodeNetworkJoinResult;
  "status": "network_joined";
} | {
  "data": JoinRequest;
  "status": "join_request_submitted";
} | {
  "data": NodeJoinRequestListResult;
  "status": "join_requests";
} | {
  "data": ProjectedNetworkMemberDirectory;
  "status": "network_members";
} | {
  "data": MemberRemovalPreview;
  "status": "member_removal_preview";
} | {
  "data": MemberLifecycleChange;
  "status": "member_lifecycle_changed";
} | {
  "data": NodeJoinRequestReviewAck;
  "status": "join_requests_reviewed";
} | {
  "data": JoinRequest;
  "status": "join_request_updated";
} | {
  "data": IssuedInvitation;
  "status": "invitation_created";
} | {
  "data": Array<InvitationStatusView>;
  "status": "invitations";
} | {
  "data": InvitationStatusView;
  "status": "invitation_status";
} | {
  "data": InvitationStatusView;
  "status": "invitation_revoked";
} | {
  "data": InvitationPreview;
  "status": "invitation_preview";
} | {
  "data": InvitationRedemption;
  "status": "invitation_redeemed";
} | {
  "data": ProjectedNodeNetworkLeaveResult;
  "status": "network_left";
} | {
  "data": NodeNetworkMembershipSnapshot;
  "status": "membership_policy_acknowledged";
} | {
  "data": LocalExitCapabilitySnapshot;
  "status": "local_exit_capability";
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
  "data": PolicyCurrent;
  "status": "policy";
} | {
  "data": PolicyPreview;
  "status": "policy_preview";
} | {
  "data": PolicyApplyResult;
  "status": "policy_applied";
} | {
  "data": PolicyHistorySnapshot;
  "status": "policy_history";
} | {
  "data": PolicyRollbackResult;
  "status": "policy_rolled_back";
} | {
  "data": PolicyTestResult;
  "status": "policy_test";
} | {
  "data": TrafficPolicyCurrent;
  "status": "traffic_policy";
} | {
  "data": TrafficPolicyPreview;
  "status": "traffic_policy_preview";
} | {
  "data": TrafficPolicyApplyResult;
  "status": "traffic_policy_applied";
} | {
  "data": TrafficPolicyHistorySnapshot;
  "status": "traffic_policy_history";
} | {
  "data": TrafficPolicyRollbackResult;
  "status": "traffic_policy_rolled_back";
} | {
  "data": TrafficPolicyTestComparison;
  "status": "traffic_policy_test";
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

export type ProjectedNodeCommandResult = {
  "outcome": ProjectedNodeCommandOutcome;
  "request_id": number;
};

export type ProjectedNodeEmbeddedServiceSnapshot = {
  "network_name": string;
  "relay_addr"?: string | null;
  "signaling_addr"?: string | null;
  "status": NodeEmbeddedServiceStatus;
};

export type ProjectedNodeEvent = {
  "kind": ProjectedNodeEventKind;
  "sequence": number;
  "snapshot_revision": number;
};

export type ProjectedNodeEventKind = {
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
  "message"?: string | null;
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

export type ProjectedNodeIdentitySnapshot = {
  "created_at_unix_ms"?: number | null;
  "display_name"?: string | null;
  "node_id": string;
  "public_key_fingerprint"?: string | null;
};

export type ProjectedNodeInterfaceSnapshot = {
  "addresses"?: Array<NodeInterfaceAddress> | null;
  "default_ipv4": boolean;
  "default_ipv6": boolean;
  "index": number;
  "kind": NodeInterfaceKind;
  "mtu": number;
  "name": string;
  "ownership": NodeInterfaceOwnership;
  "status": NodeInterfaceStatus;
};

export type ProjectedNodeNetworkJoinResult = {
  "config_saved": boolean;
  "idempotent": boolean;
  "membership": NodeNetworkMembershipSnapshot;
  "snapshot": ProjectedNodeSnapshot;
  "started": boolean;
};

export type ProjectedNodeNetworkLeaveResult = {
  "left": boolean;
  "network_name": string;
  "snapshot": ProjectedNodeSnapshot;
};

export type ProjectedNodeNetworkSnapshot = {
  "assigned_ipv4"?: string | null;
  "assigned_ipv6"?: string | null;
  "name": string;
  "p2p_listen_addr"?: string | null;
  "peer_count"?: number | null;
  "roles"?: Array<NodeNetworkRole> | null;
  "status"?: NodeNetworkStatus | null;
  "traffic"?: NodeTrafficSnapshot | null;
  "tun_name"?: string | null;
  "uptime_secs"?: number | null;
};

export type ProjectedNodePeerSnapshot = {
  "endpoints"?: Array<string> | null;
  "loss_rate"?: number | null;
  "network_name": string;
  "online"?: boolean | null;
  "p2p_active"?: boolean | null;
  "p2p_healthy"?: boolean | null;
  "peer_id": string;
  "rtt_us"?: number | null;
  "traffic"?: NodeTrafficSnapshot | null;
  "virtual_ipv4"?: string | null;
  "virtual_ipv6"?: string | null;
};

export type ProjectedNodeSnapshot = {
  "access_control"?: NodeAccessControlSnapshot;
  "capabilities"?: NodeCapabilityRegistrySnapshot;
  "configuration"?: NodeConfigurationSnapshot | null;
  "control_health": NodeControlHealth;
  "discovery_index"?: NodeDiscoveryIndexSnapshot;
  "embedded_services"?: Array<ProjectedNodeEmbeddedServiceSnapshot>;
  "exit_node_capabilities"?: ExitNodeCapabilities;
  "exit_selections"?: Array<NodeExitSelectionSnapshot>;
  "identity"?: ProjectedNodeIdentitySnapshot | null;
  "interface_problems"?: Array<NodeProblem>;
  "interfaces"?: Array<ProjectedNodeInterfaceSnapshot>;
  "last_error"?: NodeProblem | null;
  "lifecycle": NodeLifecycleSnapshot;
  "local_exit"?: LocalExitCapabilitySnapshot;
  "logs"?: Array<NodeLogEntry>;
  "memberships"?: Array<NodeNetworkMembershipSnapshot>;
  "network_profiles"?: Array<NodeNetworkProfileSnapshot>;
  "networks"?: Array<ProjectedNodeNetworkSnapshot>;
  "peers"?: Array<ProjectedNodePeerSnapshot>;
  "permissions"?: NodePermissionSnapshot;
  "recent_activity"?: Array<ProjectedActivityEntry>;
  "revision": number;
  "routing_summaries"?: Array<NodeRoutingSummarySnapshot>;
  "topology"?: TopologySnapshot | null;
  "traffic_routing_page"?: TrafficRoutingPageModel | null;
  "tun_capabilities"?: TunCapabilities | null;
  "ui_mode"?: UiModeResolution | null;
};

export type PublicKey = string;

export type ResourceKind = "network" | "ownership" | "membership" | "group" | "security_policy" | "traffic_policy" | "capability" | "exit_grant" | "file_share" | "audit" | "coordinator_term" | "keys";

export type SecurityRuleId = string;

export type Signature = string;

export type SubjectSelector = {
  "kind": "any_member";
} | {
  "kind": "role";
  "value": NetworkRole;
} | {
  "kind": "node";
  "value": NodeId;
} | {
  "kind": "group";
  "value": GroupId;
};

export type TopologyBoundary = {
  "capabilities"?: Array<CapabilityKind>;
  "member_count": number;
  "name": string;
  "network_id": NetworkId;
  "viewer_role"?: MembershipRole | null;
  "visible_member_count": number;
};

export type TopologyCluster = {
  "expandable": boolean;
  "hidden_member_count": number;
  "network_id": NetworkId;
  "sample"?: Array<NodeId>;
};

export type TopologyConnection = {
  "connection_id": ContentHash;
  "from": NodeId;
  "networks"?: Array<NetworkId>;
  "observed_at_unix_ms"?: number | null;
  "state": TopologyConnectionState;
  "to": NodeId;
};

export type TopologyConnectionState = "active" | "idle" | "stale";

export type TopologyNode = {
  "capabilities"?: Array<CapabilityKind>;
  "display_name"?: string | null;
  "health": TopologyNodeHealth;
  "is_self": boolean;
  "networks": Array<TopologyNodeNetwork>;
  "node_id": NodeId;
};

export type TopologyNodeHealth = "healthy" | "unknown" | "degraded" | "unreachable";

export type TopologyNodeNetwork = {
  "capabilities"?: Array<CapabilityKind>;
  "health": TopologyNodeHealth;
  "network_id": NetworkId;
  "presence": NetworkMemberPresence;
  "role": MembershipRole;
  "status": NetworkMemberStatus;
};

export type TopologySnapshot = {
  "boundaries": Array<TopologyBoundary>;
  "clusters"?: Array<TopologyCluster>;
  "connections": Array<TopologyConnection>;
  "generated_at_unix_ms": number;
  "nodes": Array<TopologyNode>;
  "revision": number;
  "truncated": boolean;
  "viewer": NodeId;
};

export type TrafficAction = {
  "kind": "direct";
} | {
  "kind": "network";
  "value": NetworkId;
} | {
  "kind": "exit";
  "value": NodeId;
} | {
  "kind": "reject";
};

export type TrafficDecisionReason = "first_match" | "default";

export type TrafficDefaultConfirmedBy = "owner" | "administrator";

export type TrafficDefaultPolicy = {
  "action": TrafficAction;
  "confirmed_at_unix_ms": number;
  "confirmed_by": TrafficDefaultConfirmedBy;
};

export type TrafficFailureDecision = {
  "consequence_key": string;
  "direct_fallback"?: ConfirmedTrafficDirectFallback | null;
  "failure": TrafficFailureKind;
  "outcome": TrafficFailureOutcome;
  "reason": TrafficFailureDecisionReason;
  "recovery_action": TrafficFailureRecoveryAction;
};

export type TrafficFailureDecisionReason = "default_fail_closed" | "not_allowed_by_policy" | "explicit_direct_fallback" | "unsafe_failure_class" | "reject_rule" | "fixed_exit" | "fixed_network" | "direct_action_has_no_alternate";

export type TrafficFailureKind = "unknown_application" | "dns_resolution_timeout" | "dns_pollution_detected" | "exit_unavailable" | "network_selection_unavailable" | "connection_timeout" | "partial_connection_failure";

export type TrafficFailureOutcome = "fail_closed" | "direct_fallback";

export type TrafficFailurePolicy = {
  "direct_fallback"?: ConfirmedTrafficDirectFallback | null;
};

export type TrafficFailureRecoveryAction = "retry_requested_route";

export type TrafficFlow = {
  "application": ApplicationIdentity;
  "destination_ip": string;
  "destination_port": number;
  "domain"?: DomainName | null;
  "protocol": TrafficProtocol;
};

export type TrafficMatch = {
  "applications"?: Array<ApplicationIdentity>;
  "destination_cidrs"?: Array<IpCidr>;
  "destination_ports"?: Array<PortRange>;
  "domain_suffixes"?: Array<DomainSuffix>;
  "exact_domains"?: Array<DomainName>;
  "protocols"?: Array<TrafficProtocol>;
};

export type TrafficMatchRisk = {
  "kind": "always_matches";
} | {
  "kind": "any_destination";
} | {
  "kind": "any_port";
} | {
  "kind": "any_protocol";
} | {
  "kind": "any_application";
} | {
  "cidr": string;
  "kind": "broad_cidr";
  "prefix_len": number;
} | {
  "end": number;
  "kind": "full_port_range";
  "start": number;
};

export type TrafficPolicy = {
  "default_policy": TrafficDefaultPolicy;
  "failure_policy"?: TrafficFailurePolicy;
  "node_id": NodeId;
  "revision": number;
  "rules"?: Array<TrafficRule>;
};

export type TrafficPolicyAffectedObject = {
  "kind": TrafficPolicyObjectKind;
  "object_id"?: string | null;
};

export type TrafficPolicyApplyResult = {
  "affected_objects": Array<TrafficPolicyAffectedObject>;
  "content_hash": ContentHash;
  "diffs": Array<TrafficPolicyDiffEntry>;
  "history"?: TrafficPolicyHistoryEntry | null;
  "no_op": boolean;
  "policy": TrafficPolicy;
  "previous_revision": number;
  "revision": number;
};

export type TrafficPolicyChangeAction = "apply" | "rollback";

export type TrafficPolicyChangeKind = "rule_added" | "rule_removed" | "rule_updated" | "rules_reordered" | "default_policy_changed" | "failure_policy_changed";

export type TrafficPolicyCurrent = {
  "content_hash": ContentHash;
  "policy": TrafficPolicy;
  "revision": number;
};

export type TrafficPolicyDiffEntry = {
  "after"?: unknown;
  "before"?: unknown;
  "change": TrafficPolicyChangeKind;
  "path": string;
};

export type TrafficPolicyDraft = {
  "default_policy": TrafficDefaultPolicy;
  "failure_policy"?: TrafficFailurePolicy;
  "node_id": NodeId;
  "rules"?: Array<TrafficRule>;
};

export type TrafficPolicyHistoryEntry = {
  "action": TrafficPolicyChangeAction;
  "actor": string;
  "affected_objects": Array<TrafficPolicyAffectedObject>;
  "applied_revision"?: number | null;
  "candidate_revision": number;
  "content_hash": ContentHash;
  "diffs": Array<TrafficPolicyDiffEntry>;
  "occurred_at": string;
  "outcome": TrafficPolicyHistoryOutcome;
  "previous_revision": number;
  "reason"?: string | null;
  "reason_code"?: string | null;
  "source": string;
  "target_revision"?: number | null;
  "transaction_id": string;
};

export type TrafficPolicyHistoryOutcome = "applied" | "rolled_back" | "failed" | "recovered";

export type TrafficPolicyHistorySnapshot = {
  "current_revision": number;
  "entries": Array<TrafficPolicyHistoryEntry>;
};

export type TrafficPolicyObjectKind = "rule" | "default_policy" | "failure_policy" | "network" | "exit";

export type TrafficPolicyPreview = {
  "affected_objects": Array<TrafficPolicyAffectedObject>;
  "can_apply": boolean;
  "candidate": TrafficPolicy;
  "candidate_revision": number;
  "current": TrafficPolicy;
  "current_revision": number;
  "diffs": Array<TrafficPolicyDiffEntry>;
  "expected_revision": number;
  "has_changes": boolean;
  "revision_conflict": boolean;
  "risks": Array<TrafficRuleRisk>;
  "rollback_target_revision"?: number | null;
};

export type TrafficPolicyRollbackResult = {
  "affected_objects": Array<TrafficPolicyAffectedObject>;
  "content_hash": ContentHash;
  "diffs": Array<TrafficPolicyDiffEntry>;
  "history": TrafficPolicyHistoryEntry;
  "policy": TrafficPolicy;
  "previous_revision": number;
  "revision": number;
  "target_revision": number;
};

export type TrafficPolicyTestComparison = {
  "after": TrafficPolicyTestDecision;
  "before": TrafficPolicyTestDecision;
  "delta": TrafficPolicyTestDelta;
  "network_id": NetworkId;
  "source": TrafficPolicyTestContextSource;
};

export type TrafficPolicyTestContextSource = {
  "kind": "manual";
} | {
  "activity_id": string;
  "kind": "activity";
} | {
  "diagnostic_id": string;
  "kind": "diagnostic";
};

export type TrafficPolicyTestDecision = {
  "action": TrafficAction;
  "failure_decisions": Array<TrafficFailureDecision>;
  "matched_rule"?: TrafficRuleId | null;
  "reason": TrafficDecisionReason;
  "visibility": "full";
} | {
  "action": TrafficAction;
  "visibility": "route_summary";
};

export type TrafficPolicyTestDelta = {
  "changed": boolean;
  "failure_outcomes_changed"?: boolean | null;
  "matched_rule_changed"?: boolean | null;
  "route_changed": boolean;
};

export type TrafficPolicyTestInput = {
  "fixed_exit"?: NodeId | null;
  "fixed_network"?: NetworkId | null;
  "flow": TrafficFlow;
  "network_id": NetworkId;
  "source": TrafficPolicyTestContextSource;
};

export type TrafficProtocol = "tcp" | "udp" | "icmp";

export type TrafficRoutingDefaultPolicyView = {
  "action": TrafficAction;
  "confirmed_at_unix_ms": number;
  "confirmed_by": TrafficDefaultConfirmedBy;
  "target": TrafficRoutingTargetRef;
};

export type TrafficRoutingHardConstraints = {
  "fixed_exit"?: NodeId | null;
  "fixed_network"?: NetworkId | null;
};

export type TrafficRoutingHitSample = {
  "activity_id"?: string | null;
  "after_action": TrafficAction;
  "after_reason": TrafficDecisionReason;
  "after_rule_id"?: TrafficRuleId | null;
  "before_action": TrafficAction;
  "diagnostic_id"?: string | null;
  "flow": TrafficFlow;
  "sample_id": string;
};

export type TrafficRoutingMoveAction = {
  "action_id": string;
  "direction": TrafficRoutingRuleDirection;
  "from_index": number;
  "keyboard_shortcut": string;
  "rule_id": TrafficRuleId;
  "target_index": number;
};

export type TrafficRoutingNavigation = {
  "network_name"?: string | null;
  "object_id": string;
  "page_id": string;
};

export type TrafficRoutingObservation = {
  "hits"?: Array<TrafficRoutingObservedHit>;
  "observed_flow_count": number;
  "status": "available";
  "window_end_unix_ms": number;
  "window_start_unix_ms": number;
} | {
  "reason_code": string;
  "status": "unavailable";
};

export type TrafficRoutingObservedHit = {
  "activity_id"?: string | null;
  "diagnostic_id"?: string | null;
  "flow": TrafficFlow;
  "sample_id": string;
};

export type TrafficRoutingPageModel = {
  "authority_revision": number;
  "can_edit": boolean;
  "default_policy": TrafficRoutingDefaultPolicyView;
  "failure_policy": TrafficFailurePolicy;
  "hard_constraint_target"?: TrafficRoutingTargetRef | null;
  "hard_constraints"?: TrafficRoutingHardConstraints;
  "node_id": NodeId;
  "observation"?: TrafficRoutingObservation;
  "rules": Array<TrafficRoutingRuleView>;
  "targets": Array<TrafficRoutingTarget>;
  "workflow": TrafficRoutingWorkflowState;
};

export type TrafficRoutingRuleActivityEffect = {
  "kind": "observed_traffic_reassigned";
  "sample_count": number;
} | {
  "kind": "no_observed_traffic";
};

export type TrafficRoutingRuleChange = "disable" | "delete";

export type TrafficRoutingRuleDirection = "up" | "down";

export type TrafficRoutingRuleImpact = {
  "activity_effect": TrafficRoutingRuleActivityEffect;
  "activity_ids": Array<string>;
  "change": TrafficRoutingRuleChange;
  "diagnostic_ids": Array<string>;
  "hit_samples": Array<TrafficRoutingHitSample>;
  "previous_action": TrafficAction;
  "rule_id": TrafficRuleId;
};

export type TrafficRoutingRuleImpactView = {
  "impact": TrafficRoutingRuleImpact;
  "status": "available";
} | {
  "reason_code": string;
  "status": "unavailable";
};

export type TrafficRoutingRuleView = {
  "action": TrafficAction;
  "delete_impact"?: TrafficRoutingRuleImpactView;
  "description": string;
  "disable_impact"?: TrafficRoutingRuleImpactView;
  "enabled": boolean;
  "matcher": TrafficMatch;
  "move_down"?: TrafficRoutingMoveAction | null;
  "move_up"?: TrafficRoutingMoveAction | null;
  "priority": number;
  "risks": Array<TrafficMatchRisk>;
  "rule_id": TrafficRuleId;
  "target": TrafficRoutingTargetRef;
};

export type TrafficRoutingTarget = {
  "id": string;
  "kind": TrafficRoutingTargetKind;
  "label": string;
  "navigation"?: TrafficRoutingNavigation | null;
  "search_terms"?: Array<string>;
};

export type TrafficRoutingTargetKind = "direct" | "network" | "exit" | "reject";

export type TrafficRoutingTargetRef = {
  "id": string;
  "kind": TrafficRoutingTargetKind;
  "resolved": boolean;
};

export type TrafficRoutingWorkflowState = {
  "reason_code": string;
  "status": "unconfigured";
} | {
  "base_revision": number;
  "status": "draft";
} | {
  "base_revision": number;
  "candidate_revision": number;
  "operation_id": string;
  "status": "applying";
} | {
  "revision": number;
  "status": "applied";
} | {
  "base_revision": number;
  "candidate_revision": number;
  "reason_code": string;
  "stage": string;
  "status": "failed";
} | {
  "reason_code": string;
  "revision": number;
  "status": "stale";
};

export type TrafficRule = {
  "action": TrafficAction;
  "description"?: string;
  "enabled"?: boolean;
  "matcher": TrafficMatch;
  "priority": number;
  "rule_id": TrafficRuleId;
};

export type TrafficRuleId = string;

export type TrafficRuleRisk = {
  "enabled": boolean;
  "priority": number;
  "risks": Array<TrafficMatchRisk>;
  "rule_id": TrafficRuleId;
};

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
