package cc.xlei.fuckipv6.contract;

import java.lang.reflect.Field;
import java.util.Collections;

import cc.xlei.fuckipv6.contract.NodeContractV1.CommandEnvelope;
import cc.xlei.fuckipv6.contract.NodeContractV1.CapabilityState;
import cc.xlei.fuckipv6.contract.NodeContractV1.CoordinatorElectionStage;
import cc.xlei.fuckipv6.contract.NodeContractV1.LocalExitCapabilityStage;
import cc.xlei.fuckipv6.contract.NodeContractV1.NodeCleanupFailureReason;
import cc.xlei.fuckipv6.contract.NodeContractV1.NodeCleanupStepStatus;
import cc.xlei.fuckipv6.contract.NodeContractV1.NodeCommandKind;
import cc.xlei.fuckipv6.contract.NodeContractV1.NodeDiagnosticLayer;
import cc.xlei.fuckipv6.contract.NodeContractV1.NodeDiagnosticState;
import cc.xlei.fuckipv6.contract.NodeContractV1.NodeEventReplayReason;
import cc.xlei.fuckipv6.contract.NodeContractV1.NodeProblemCode;
import cc.xlei.fuckipv6.contract.NodeContractV1.NodeRoutingSummaryStatus;
import cc.xlei.fuckipv6.contract.NodeContractV1.OperationKind;
import cc.xlei.fuckipv6.contract.NodeContractV1.OperationState;
import cc.xlei.fuckipv6.contract.NodeContractV1.TunCapabilityReason;
import cc.xlei.fuckipv6.contract.NodeContractV1.TunPlatform;

/** Dependency-free executable checks for Android-safe contract bindings. */
public final class NodeContractV1Test {
    private interface FailingCall {
        void run();
    }

    private static void assertTrue(boolean condition, String message) {
        if (!condition) {
            throw new AssertionError(message);
        }
    }

    private static void assertSame(Object expected, Object actual, String message) {
        if (expected != actual) {
            throw new AssertionError(message + ": expected=" + expected + " actual=" + actual);
        }
    }

    private static void expectIllegalArgument(FailingCall call, String label) {
        try {
            call.run();
        } catch (IllegalArgumentException expected) {
            return;
        }
        throw new AssertionError(label + " must fail closed");
    }

    private static void assertNoDeclaredField(Class<?> type, String name) {
        for (Field field : type.getDeclaredFields()) {
            if (field.getName().equals(name)) {
                throw new AssertionError(type.getSimpleName() + " must not expose " + name);
            }
        }
    }

    public static void main(String[] args) throws Exception {
        assertTrue(
                NodeContractV1.DTO_CLASS_NAMES.length == 307,
                "generated DTO inventory changed without updating the contract test");
        for (String name : NodeContractV1.DTO_CLASS_NAMES) {
            Class.forName("cc.xlei.fuckipv6.contract.NodeContractV1$" + name);
        }

        for (String field : new String[] {
            "actor",
            "attempt",
            "created_at_unix_ms",
            "error",
            "idempotency_key",
            "kind",
            "max_attempts",
            "operation_id",
            "result",
            "stage",
            "state",
            "target",
            "updated_at_unix_ms"
        }) {
            NodeContractV1.NodeOperationSnapshot.class.getDeclaredField(field);
        }
        NodeContractV1.NodeApiOperationResponse.class.getDeclaredField("operation");
        NodeContractV1.NodeApiCommandResponse.class.getDeclaredField("operation");
        NodeContractV1.NodeCommandHttpRequest.class.getDeclaredField("idempotency_key");
        for (String field : new String[] {"kind", "sequence", "snapshot_revision"}) {
            NodeContractV1.NodeEvent.class.getDeclaredField(field);
        }
        for (String field : new String[] {"sequence", "snapshot_revision"}) {
            NodeContractV1.NodeEventCursor.class.getDeclaredField(field);
        }
        NodeContractV1.NodeApiEvent.Snapshot.class.getDeclaredField("cursor");
        for (String field : new String[] {
            "latest_cursor", "reason", "requested_cursor"
        }) {
            NodeContractV1.NodeApiEvent.ResyncRequired.class.getDeclaredField(field);
        }
        for (String field : new String[] {
            "latest_cursor", "requested_cursor", "resync_reason"
        }) {
            NodeContractV1.NodeApiErrorResponse.class.getDeclaredField(field);
        }

        for (String field : new String[] {
            "revision",
            "lifecycle",
            "local_exit",
            "identity",
            "configuration",
            "exit_node_capabilities",
            "exit_selections",
            "capabilities",
            "permissions",
            "embedded_services",
            "ui_mode",
            "tun_capabilities",
            "interfaces",
            "interface_problems",
            "networks",
            "peers",
            "recent_activity",
            "routing_summaries",
            "traffic_routing_page",
            "traffic_session",
            "logs",
            "last_error",
            "access_control"
        }) {
            NodeContractV1.NodeSnapshot.class.getDeclaredField(field);
        }

        for (String field : new String[] {
            "network_name",
            "relay_addr",
            "relay_governance",
            "relay_health",
            "relay_status",
            "signaling_addr",
            "status"
        }) {
            NodeContractV1.NodeEmbeddedServiceSnapshot.class.getDeclaredField(field);
            NodeContractV1.ProjectedNodeEmbeddedServiceSnapshot.class.getDeclaredField(field);
        }
        for (String field : new String[] {
            "directory_generated_at_unix_ms",
            "directory_received_at_unix_ms",
            "evaluated_at_unix_ms",
            "network_id",
            "policy",
            "policy_revision",
            "reason_code",
            "resolution"
        }) {
            NodeContractV1.NodeExitSelectionSnapshot.class.getDeclaredField(field);
        }
        for (String field : new String[] {
            "changed_at_unix_ms", "events", "failure", "reason_code", "state", "validation"
        }) {
            NodeContractV1.TrafficSessionSnapshot.class.getDeclaredField(field);
        }
        for (String field : new String[] {
            "dns_validated",
            "observed_at_unix_ms",
            "reachability_validated",
            "routes_validated",
            "tun_validated"
        }) {
            NodeContractV1.TrafficSessionValidation.class.getDeclaredField(field);
        }
        for (String field : new String[] {
            "expires_at_unix_ms",
            "issued_at_unix_ms",
            "join_mode",
            "network_id",
            "network_name",
            "owner",
            "policy_revision",
            "preview_digest",
            "privacy_policy_revision",
            "public_key",
            "revision",
            "signature",
            "terms",
            "terms_hash"
        }) {
            NodeContractV1.NetworkJoinPreview.class.getDeclaredField(field);
        }
        for (String field : new String[] {
            "exit_contribution",
            "exit_contribution_mode",
            "exit_contribution_scope",
            "member_limit",
            "network_policy_revision",
            "privacy_policy_revision"
        }) {
            NodeContractV1.NetworkJoinTerms.class.getDeclaredField(field);
        }
        NodeContractV1.NodeJoinNetworkCommand.class.getDeclaredField("signed_join_preview");

        for (String field : new String[] {"local_policy", "network_acls"}) {
            NodeContractV1.NodeAccessControlSnapshot.class.getDeclaredField(field);
        }
        for (String field : new String[] {
            "max_bandwidth_kbps",
            "max_bytes",
            "max_concurrent_streams",
            "max_connections",
            "max_cpu_percent",
            "max_memory_bytes",
            "max_session_duration_secs",
            "quota_reset_at_unix_ms"
        }) {
            NodeContractV1.RelayLimitPolicy.class.getDeclaredField(field);
        }
        for (String field : new String[] {
            "active_connections",
            "active_sessions",
            "active_streams",
            "cpu_measurement",
            "forwarded_bytes",
            "forwarded_packets",
            "last_forwarded_at_unix_ms",
            "last_forwarding_failure_at_unix_ms",
            "memory_measurement",
            "observed_cpu_percent",
            "observed_memory_bytes",
            "oldest_session_age_secs",
            "quota_reset_at_unix_ms",
            "quota_started_at_unix_ms",
            "rejected_connections",
            "rejected_packets"
        }) {
            NodeContractV1.RelayUsageSnapshot.class.getDeclaredField(field);
        }
        for (String field : new String[] {
            "authorization_valid",
            "forwarding_observed",
            "issues",
            "lifecycle",
            "limit_sources",
            "limits",
            "listen_addr",
            "listener_bound",
            "network_reachability",
            "observed_at_unix_ms",
            "quota_limited",
            "reason_code",
            "status",
            "usage"
        }) {
            NodeContractV1.RelayHealthSnapshot.class.getDeclaredField(field);
        }
        for (String field : new String[] {
            "action",
            "limit",
            "node_id",
            "observed",
            "occurred_at_unix_ms",
            "reason_code",
            "reset_at_unix_ms"
        }) {
            NodeContractV1.RelayGovernanceEvent.class.getDeclaredField(field);
        }
        for (String field : new String[] {"dropped_events", "events"}) {
            NodeContractV1.RelayGovernanceAuditSnapshot.class.getDeclaredField(field);
        }
        for (String field : new String[] {
            "payload", "private_key", "session_key", "source_address", "destination_address"
        }) {
            assertNoDeclaredField(NodeContractV1.RelayGovernanceEvent.class, field);
        }
        assertSame(
                "session_rejected",
                NodeContractV1.RelayGovernanceAction.SESSION_REJECTED.wireName(),
                "Relay governance rejection wire name");
        assertSame(
                "quota_exhausted",
                NodeContractV1.RelayHealthStatus.QUOTA_EXHAUSTED.wireName(),
                "Relay quota health wire name");
        assertSame(
                "network_acl",
                NodeContractV1.RelayLimitSource.NETWORK_ACL.wireName(),
                "Relay limit source wire name");
        assertSame(
                "unsupported",
                NodeContractV1.RelayMeasurementState.UNSUPPORTED.wireName(),
                "Relay measurement state wire name");
        assertSame(
                "unreachable",
                NodeContractV1.RelayReachabilityState.UNREACHABLE.wireName(),
                "Relay reachability wire name");
        for (String field : new String[] {
            "actor",
            "emergency_coordinator",
            "groups",
            "local_policy",
            "membership_active",
            "network_acl",
            "network_id",
            "now_unix_ms",
            "recent_reauthentication",
            "request",
            "role"
        }) {
            NodeContractV1.PolicyTestInput.class.getDeclaredField(field);
        }
        for (String field : new String[] {
            "decision", "explanations", "precedence", "validation_issues"
        }) {
            NodeContractV1.PolicyTestResult.class.getDeclaredField(field);
        }
        NodeContractV1.NodeCommand.TestPolicy.class.getDeclaredField("payload");
        NodeContractV1.NodeCommandOutcome.PolicyTest.class.getDeclaredField("data");
        for (String field : new String[] {"candidate", "expected_revision"}) {
            NodeContractV1.NodePreviewPolicyCommand.class.getDeclaredField(field);
        }
        for (String field : new String[] {"expected_revision", "target_revision"}) {
            NodeContractV1.NodePreviewPolicyRollbackCommand.class.getDeclaredField(field);
        }
        for (String field : new String[] {
            "candidate", "confirm_privilege_expansion", "expected_revision",
            "includes_local_policy", "network_ids"
        }) {
            NodeContractV1.NodeApplyPolicyCommand.class.getDeclaredField(field);
        }
        for (String field : new String[] {
            "confirm_privilege_expansion", "expected_revision", "includes_local_policy",
            "network_ids", "target_revision"
        }) {
            NodeContractV1.NodeRollbackPolicyCommand.class.getDeclaredField(field);
        }
        NodeContractV1.NodeCommand.PreviewPolicy.class.getDeclaredField("payload");
        NodeContractV1.NodeCommand.PreviewPolicyRollback.class.getDeclaredField("payload");
        NodeContractV1.NodeCommand.ApplyPolicy.class.getDeclaredField("payload");
        NodeContractV1.NodeCommand.RollbackPolicy.class.getDeclaredField("payload");
        for (Class<?> outcome : new Class<?>[] {
            NodeContractV1.NodeCommandOutcome.Policy.class,
            NodeContractV1.NodeCommandOutcome.PolicyPreview.class,
            NodeContractV1.NodeCommandOutcome.PolicyApplied.class,
            NodeContractV1.NodeCommandOutcome.PolicyHistory.class,
            NodeContractV1.NodeCommandOutcome.PolicyRolledBack.class
        }) {
            outcome.getDeclaredField("data");
        }
        for (String field : new String[] {"confirm_risk", "draft", "expected_revision"}) {
            NodeContractV1.NodeApplyTrafficPolicyCommand.class.getDeclaredField(field);
        }
        for (String field : new String[] {"draft", "expected_revision"}) {
            NodeContractV1.NodePreviewTrafficPolicyCommand.class.getDeclaredField(field);
        }
        for (String field : new String[] {"expected_revision", "target_revision"}) {
            NodeContractV1.NodePreviewTrafficPolicyRollbackCommand.class.getDeclaredField(field);
        }
        for (String field : new String[] {"confirm_risk", "expected_revision", "target_revision"}) {
            NodeContractV1.NodeRollbackTrafficPolicyCommand.class.getDeclaredField(field);
        }
        for (String field : new String[] {"candidate", "current", "input"}) {
            NodeContractV1.NodeTestTrafficPolicyCommand.class.getDeclaredField(field);
        }
        NodeContractV1.NodeCommand.PreviewTrafficPolicy.class.getDeclaredField("payload");
        NodeContractV1.NodeCommand.PreviewTrafficPolicyRollback.class.getDeclaredField("payload");
        NodeContractV1.NodeCommand.ApplyTrafficPolicy.class.getDeclaredField("payload");
        NodeContractV1.NodeCommand.RollbackTrafficPolicy.class.getDeclaredField("payload");
        NodeContractV1.NodeCommand.TestTrafficPolicy.class.getDeclaredField("payload");
        for (Class<?> outcome : new Class<?>[] {
            NodeContractV1.NodeCommandOutcome.TrafficPolicy.class,
            NodeContractV1.NodeCommandOutcome.TrafficPolicyPreview.class,
            NodeContractV1.NodeCommandOutcome.TrafficPolicyApplied.class,
            NodeContractV1.NodeCommandOutcome.TrafficPolicyHistory.class,
            NodeContractV1.NodeCommandOutcome.TrafficPolicyRolledBack.class,
            NodeContractV1.NodeCommandOutcome.TrafficPolicyTest.class
        }) {
            outcome.getDeclaredField("data");
        }
        for (String field : new String[] {"content_hash", "policy", "revision"}) {
            NodeContractV1.TrafficPolicyCurrent.class.getDeclaredField(field);
        }
        NodeContractV1.NodeSnapshot.class.getDeclaredField("traffic_routing_page");
        NodeContractV1.ProjectedNodeSnapshot.class.getDeclaredField("traffic_routing_page");
        NodeContractV1.NodeSnapshot.class.getDeclaredField("local_exit");
        NodeContractV1.ProjectedNodeSnapshot.class.getDeclaredField("local_exit");
        for (String field : new String[] {
            "active_clients", "healthy", "last_transition_at_unix_ms", "network_allowed",
            "owner_exit_enabled", "problem", "stage", "total_clients"
        }) {
            NodeContractV1.LocalExitCapabilitySnapshot.class.getDeclaredField(field);
        }
        NodeContractV1.NodeSetLocalExitCapabilityCommand.class.getDeclaredField("enabled");
        NodeContractV1.NodeCommand.SetLocalExitCapability.class.getDeclaredField("payload");
        NodeContractV1.NodeCommandOutcome.LocalExitCapability.class.getDeclaredField("data");
        NodeContractV1.ProjectedNodeCommandOutcome.LocalExitCapability.class.getDeclaredField("data");
        for (String field : new String[] {
            "authority_revision", "can_edit", "default_policy", "failure_policy",
            "hard_constraint_target", "hard_constraints", "node_id", "observation", "rules",
            "targets", "workflow"
        }) {
            NodeContractV1.TrafficRoutingPageModel.class.getDeclaredField(field);
        }
        for (String field : new String[] {
            "action", "delete_impact", "description", "disable_impact", "enabled", "matcher",
            "move_down", "move_up", "priority", "risks", "rule_id", "target"
        }) {
            NodeContractV1.TrafficRoutingRuleView.class.getDeclaredField(field);
        }
        for (String field : new String[] {
            "id", "kind", "label", "navigation", "search_terms"
        }) {
            NodeContractV1.TrafficRoutingTarget.class.getDeclaredField(field);
        }
        for (String field : new String[] {
            "activity_effect", "activity_ids", "change", "diagnostic_ids", "hit_samples",
            "previous_action", "rule_id"
        }) {
            NodeContractV1.TrafficRoutingRuleImpact.class.getDeclaredField(field);
        }
        for (String field : new String[] {
            "activity_id", "after_action", "after_reason", "after_rule_id", "before_action",
            "diagnostic_id", "flow", "sample_id"
        }) {
            NodeContractV1.TrafficRoutingHitSample.class.getDeclaredField(field);
        }
        for (String field : new String[] {"activity_id", "diagnostic_id", "flow", "sample_id"}) {
            NodeContractV1.TrafficRoutingObservedHit.class.getDeclaredField(field);
        }
        for (String field : new String[] {
            "hits", "observed_flow_count", "window_end_unix_ms", "window_start_unix_ms"
        }) {
            NodeContractV1.TrafficRoutingObservation.Available.class.getDeclaredField(field);
        }
        NodeContractV1.TrafficRoutingObservation.Unavailable.class.getDeclaredField("reason_code");
        NodeContractV1.TrafficRoutingRuleImpactView.Available.class.getDeclaredField("impact");
        NodeContractV1.TrafficRoutingRuleImpactView.Unavailable.class.getDeclaredField(
                "reason_code");
        NodeContractV1.TrafficRoutingRuleActivityEffect.ObservedTrafficReassigned.class
                .getDeclaredField("sample_count");
        assertSame(
                "available",
                new NodeContractV1.TrafficRoutingObservation.Available(
                        Collections.emptyList(), 0L, 2L, 1L).status,
                "available traffic observation tag");
        assertSame(
                "unavailable",
                new NodeContractV1.TrafficRoutingObservation.Unavailable("collector_offline").status,
                "unavailable traffic observation tag");
        assertSame(
                "available",
                new NodeContractV1.TrafficRoutingRuleImpactView.Available(null).status,
                "available rule impact tag");
        assertSame(
                "no_observed_traffic",
                new NodeContractV1.TrafficRoutingRuleActivityEffect.NoObservedTraffic().kind,
                "observed zero-hit rule effect tag");
        for (String field : new String[] {
            "affected_objects", "can_apply", "candidate", "candidate_revision", "current",
            "current_revision", "diffs", "expected_revision", "has_changes",
            "revision_conflict", "risks", "rollback_target_revision"
        }) {
            NodeContractV1.TrafficPolicyPreview.class.getDeclaredField(field);
        }
        for (String field : new String[] {
            "affected_objects", "content_hash", "diffs", "history", "no_op", "policy",
            "previous_revision", "revision"
        }) {
            NodeContractV1.TrafficPolicyApplyResult.class.getDeclaredField(field);
        }
        for (String field : new String[] {
            "affected_objects", "content_hash", "diffs", "history", "policy",
            "previous_revision", "revision", "target_revision"
        }) {
            NodeContractV1.TrafficPolicyRollbackResult.class.getDeclaredField(field);
        }
        for (String field : new String[] {"current_revision", "entries"}) {
            NodeContractV1.TrafficPolicyHistorySnapshot.class.getDeclaredField(field);
        }
        for (String field : new String[] {"after", "before", "delta", "network_id", "source"}) {
            NodeContractV1.TrafficPolicyTestComparison.class.getDeclaredField(field);
        }
        for (String field : new String[] {
            "affected_objects", "audit", "diffs", "no_op", "previous_revision",
            "revision", "risk", "snapshot"
        }) {
            NodeContractV1.PolicyApplyResult.class.getDeclaredField(field);
        }
        for (String field : new String[] {
            "affected_objects", "can_apply", "candidate", "current", "current_revision",
            "diffs", "expected_revision", "has_changes", "includes_local_policy",
            "network_ids", "revision_conflict", "risk", "rollback_target_revision"
        }) {
            NodeContractV1.PolicyPreview.class.getDeclaredField(field);
        }
        for (String field : new String[] {
            "affected_objects", "audit", "diffs", "previous_revision", "revision",
            "risk", "snapshot", "target_revision"
        }) {
            NodeContractV1.PolicyRollbackResult.class.getDeclaredField(field);
        }
        for (String field : new String[] {
            "action", "actor", "affected_objects", "diffs", "local_policy_revision",
            "network_acl_revisions", "occurred_at", "previous_revision", "revision",
            "risk", "source", "target_revision", "transaction_id"
        }) {
            NodeContractV1.PolicyHistoryEntry.class.getDeclaredField(field);
        }
        for (String field : new String[] {
            "expires_at_unix_ms", "grants", "issued_at_unix_ms", "network_id", "relay_limits",
            "revision", "rules"
        }) {
            NodeContractV1.NetworkAcl.class.getDeclaredField(field);
        }
        for (String field : new String[] {
            "actions", "expires_at_unix_ms", "grant_id", "issued_at_unix_ms",
            "requires_recent_reauthentication", "resources", "revoked_at_unix_ms",
            "revoked_by", "subjects"
        }) {
            NodeContractV1.NetworkAclGrant.class.getDeclaredField(field);
        }
        for (String field : new String[] {
            "contribution_vetoes", "expires_at_unix_ms", "issued_at_unix_ms", "network_scope",
            "node_id", "relay_limits", "revision", "rules"
        }) {
            NodeContractV1.NodeLocalAccessPolicy.class.getDeclaredField(field);
        }
        for (String field : new String[] {"contributions", "network_id", "veto_id"}) {
            NodeContractV1.NodeContributionVeto.class.getDeclaredField(field);
        }
        for (String field : new String[] {
            "actions", "conditions", "effect", "resources", "rule_id", "subjects"
        }) {
            NodeContractV1.PermissionRule.class.getDeclaredField(field);
        }
        assertSame(
                "deny",
                NodeContractV1.PermissionEffect.fromWireName("deny").wireName(),
                "ACL deny effect");
        assertSame(
                "any_member",
                new NodeContractV1.SubjectSelector.AnyMember().kind,
                "ACL any-member selector");
        assertSame(
                "node",
                new NodeContractV1.SubjectSelector.Node("node-a").kind,
                "ACL node selector");
        assertSame(
                "membership_active",
                new NodeContractV1.PermissionCondition.MembershipActive().kind,
                "ACL membership condition");
        assertSame(
                "all",
                new NodeContractV1.NodeNetworkScope.All().kind,
                "local policy all scope");

        assertSame(
                NodeCommandKind.GET_SNAPSHOT,
                NodeCommandKind.fromWireName("get_snapshot"),
                "known command");
        assertSame(
                NodeCommandKind.GET_NODE_DIAGNOSTICS,
                NodeCommandKind.fromWireName("get_node_diagnostics"),
                "runtime diagnostics command");
        assertSame(
                NodeCommandKind.CLEANUP_NODE_RESIDUALS,
                NodeCommandKind.fromWireName("cleanup_node_residuals"),
                "runtime cleanup command");
        assertSame(
                NodeCommandKind.GET_CONFIGURATION,
                NodeCommandKind.fromWireName("get_configuration"),
                "configuration read command");
        assertSame(
                NodeCommandKind.PREVIEW_CONFIGURATION,
                NodeCommandKind.fromWireName("preview_configuration"),
                "configuration preview command");
        assertSame(
                NodeCommandKind.APPLY_CONFIGURATION,
                NodeCommandKind.fromWireName("apply_configuration"),
                "configuration apply command");
        assertSame(
                NodeCommandKind.ROLLBACK_CONFIGURATION,
                NodeCommandKind.fromWireName("rollback_configuration"),
                "configuration rollback command");
        assertSame(
                NodeCommandKind.GET_CONFIGURATION_HISTORY,
                NodeCommandKind.fromWireName("get_configuration_history"),
                "configuration history command");
        assertSame(
                NodeCommandKind.GET_TRAFFIC_POLICY,
                NodeCommandKind.fromWireName("get_traffic_policy"),
                "traffic policy read command");
        assertSame(
                NodeCommandKind.APPLY_TRAFFIC_POLICY,
                NodeCommandKind.fromWireName("apply_traffic_policy"),
                "traffic policy apply command");
        assertSame(
                NodeCommandKind.TEST_TRAFFIC_POLICY,
                NodeCommandKind.fromWireName("test_traffic_policy"),
                "traffic policy test command");
        assertSame(
                NodeCommandKind.REBUILD_TUN,
                NodeCommandKind.fromWireName("rebuild_tun"),
                "TUN rebuild command");
        assertSame(
                OperationKind.TUN_REBUILD,
                OperationKind.fromWireName("tun_rebuild"),
                "TUN rebuild operation kind");
        assertSame(
                NodeCommandKind.ROTATE_NODE_IDENTITY,
                NodeCommandKind.fromWireName("rotate_node_identity"),
                "node identity rotation command");
        assertSame(
                NodeCommandKind.CREATE_INVITATION,
                NodeCommandKind.fromWireName("create_invitation"),
                "invitation creation command");
        assertSame(
                NodeCommandKind.REDEEM_INVITATION,
                NodeCommandKind.fromWireName("redeem_invitation"),
                "invitation redemption command");
        for (String field : new String[] {
            "capabilities", "expires_in_ms", "max_uses", "network_id", "role"
        }) {
            NodeContractV1.NodeCreateInvitationCommand.class.getDeclaredField(field);
        }
        for (String field : new String[] {
            "can_accept", "capabilities", "creates_permanent_membership", "invitation_id",
            "role", "terms", "terms_hash"
        }) {
            NodeContractV1.InvitationPreview.class.getDeclaredField(field);
        }
        NodeContractV1.NodeRotateNodeIdentityCommand.class.getDeclaredField(
                "expected_fingerprint");
        for (String field : new String[] {
            "checks",
            "cleanup_candidate_networks",
            "completed_at_unix_ms",
            "diagnostic_id",
            "exit_health",
            "exit_health_assessment",
            "leak_detection",
            "leak_detection_assessment",
            "lifecycle",
            "recovery_actions",
            "residue",
            "retained_owner_networks",
            "sanitized_report",
            "started_at_unix_ms",
            "timed_out"
        }) {
            NodeContractV1.NodeDiagnosticsSnapshot.class.getDeclaredField(field);
        }
        for (String field : new String[] {
            "dns_mode",
            "ipv4_carrier_available",
            "ipv6_carrier_available",
            "platform",
            "protected_path_active"
        }) {
            NodeContractV1.LeakDetectionEnvironment.class.getDeclaredField(field);
        }
        for (String field : new String[] {
            "duration_ms",
            "error_code",
            "isolation_action",
            "observed_at_unix_ms",
            "source",
            "state",
            "target"
        }) {
            NodeContractV1.LeakDetectionEvidence.class.getDeclaredField(field);
        }
        for (String field : new String[] {
            "environment", "evidence", "generated_at_unix_ms", "max_age_ms"
        }) {
            NodeContractV1.LeakDetectionSnapshot.class.getDeclaredField(field);
        }
        for (String field : new String[] {
            "affected_protocols",
            "affected_targets",
            "fail_closed_required",
            "reason_code",
            "state"
        }) {
            NodeContractV1.LeakDetectionAssessment.class.getDeclaredField(field);
        }
        for (String field : new String[] {
            "duration_ms",
            "error_code",
            "layer",
            "message_key",
            "started_at_unix_ms",
            "state",
            "suggestion_key"
        }) {
            NodeContractV1.NodeDiagnosticCheck.class.getDeclaredField(field);
        }
        for (String field : new String[] {"action_id", "confirmation_required", "impact_key"}) {
            NodeContractV1.NodeDiagnosticRecoveryAction.class.getDeclaredField(field);
        }
        for (String field : new String[] {
            "affected_configured_network_count",
            "created_at_unix_ms",
            "new_fingerprint",
            "new_node_id",
            "old_fingerprint",
            "old_node_id",
            "owner_initialized",
            "owner_sessions_invalidated"
        }) {
            NodeContractV1.NodeIdentityRotationSnapshot.class.getDeclaredField(field);
        }
        expectIllegalArgument(
                () -> NodeCommandKind.fromWireName("not_a_command"),
                "unknown command");
        expectIllegalArgument(() -> NodeCommandKind.fromWireName(null), "null command");
        assertSame(
                CapabilityState.UNKNOWN,
                CapabilityState.fromWireName("unknown"),
                "migrated capability state");
        assertSame(
                NodeProblemCode.UNSUPPORTED,
                NodeProblemCode.fromWireName("unsupported"),
                "platform-unsupported problem code");
        assertSame(
                NodeProblemCode.RESOURCE_EXHAUSTED,
                NodeProblemCode.fromWireName("resource_exhausted"),
                "resource-exhausted problem code");
        assertSame(
                TunPlatform.LINUX,
                TunPlatform.fromWireName("linux"),
                "TUN platform");
        assertSame(
                NodeDiagnosticLayer.COORDINATOR,
                NodeDiagnosticLayer.fromWireName("coordinator"),
                "diagnostic coordinator layer");
        assertSame(
                NodeDiagnosticState.NO_PERMISSION,
                NodeDiagnosticState.fromWireName("no_permission"),
                "diagnostic no-permission state");
        expectIllegalArgument(
                () -> NodeDiagnosticLayer.fromWireName("invented"),
                "unknown diagnostic layer");
        expectIllegalArgument(
                () -> NodeDiagnosticState.fromWireName("invented"),
                "unknown diagnostic state");
        assertSame(
                NodeRoutingSummaryStatus.APPLY_FAILED,
                NodeRoutingSummaryStatus.fromWireName("apply_failed"),
                "routing summary apply failure");
        expectIllegalArgument(
                () -> NodeRoutingSummaryStatus.fromWireName("invented"),
                "unknown routing summary status");
        assertSame(
                TunCapabilityReason.PERMISSION_DENIED,
                TunCapabilityReason.fromWireName("permission_denied"),
                "TUN capability reason");
        expectIllegalArgument(
                () -> TunCapabilityReason.fromWireName("invented"),
                "unknown TUN capability reason");
        assertSame(
                NodeCleanupFailureReason.OWNER_UNAVAILABLE,
                NodeCleanupFailureReason.fromWireName("owner_unavailable"),
                "runtime cleanup failure reason");
        assertSame(
                NodeCleanupStepStatus.DEFERRED,
                NodeCleanupStepStatus.fromWireName("deferred"),
                "runtime cleanup step status");
        assertSame(
                LocalExitCapabilityStage.HEALTH_CHECKING,
                LocalExitCapabilityStage.fromWireName("health_checking"),
                "local Exit health-checking stage");
        assertSame(
                NodeEventReplayReason.REVISION_GAP,
                NodeEventReplayReason.fromWireName("revision_gap"),
                "event replay reason");
        assertSame(
                OperationState.TIMED_OUT,
                OperationState.fromWireName("timed_out"),
                "operation timed-out state");
        expectIllegalArgument(
                () -> OperationState.fromWireName("invented"),
                "unknown operation state");
        expectIllegalArgument(
                () -> CapabilityState.fromWireName("invented"),
                "unknown capability state");
        expectIllegalArgument(
                () -> NodeProblemCode.fromWireName(null),
                "null problem code");

        assertTrue(
                NodeContractV1.isManifestAction(NodeCommandKind.CREATE_NETWORK),
                "create_network must be a visible action");
        assertTrue(
                !NodeContractV1.isManifestAction(NodeCommandKind.GET_UI_MANIFEST),
                "get_ui_manifest must remain bootstrap-only");
        assertTrue(
                NodeContractV1.isManifestAction(NodeCommandKind.REBUILD_TUN),
                "rebuild_tun must be a visible action");
        assertTrue(
                NodeContractV1.isManifestAction(NodeCommandKind.ROTATE_NODE_IDENTITY),
                "rotate_node_identity must be a visible action");
        assertTrue(
                NodeContractV1.isManifestAction(NodeCommandKind.GET_NODE_DIAGNOSTICS),
                "get_node_diagnostics must be a visible action");
        assertTrue(
                NodeContractV1.isManifestAction(NodeCommandKind.CLEANUP_NODE_RESIDUALS),
                "cleanup_node_residuals must be a visible action");
        assertTrue(
                NodeContractV1.isManifestAction(NodeCommandKind.GET_CONFIGURATION),
                "get_configuration must be a visible action");
        assertTrue(
                NodeContractV1.isManifestAction(NodeCommandKind.APPLY_CONFIGURATION),
                "apply_configuration must be a visible action");

        CommandEnvelope envelope = NodeContractV1.commandEnvelope(1, 7, "get_snapshot");
        assertTrue(envelope.requestId == 7, "request id must be preserved");
        assertSame(NodeCommandKind.GET_SNAPSHOT, envelope.command, "command identity");
        assertSame(
                NodeCommandKind.GET_NODE_DIAGNOSTICS,
                NodeContractV1.commandEnvelope(1, 8, "get_node_diagnostics").command,
                "runtime diagnostics envelope");
        assertSame(
                NodeCommandKind.CLEANUP_NODE_RESIDUALS,
                NodeContractV1.commandEnvelope(1, 9, "cleanup_node_residuals").command,
                "runtime cleanup envelope");

        expectIllegalArgument(
                () -> NodeContractV1.commandEnvelope(2, 1, "get_snapshot"),
                "unsupported schema version");
        expectIllegalArgument(
                () -> NodeContractV1.commandEnvelope(1, -1, "get_snapshot"),
                "negative request id");
        expectIllegalArgument(
                () -> NodeContractV1.commandEnvelope(1, 1, "get_ui_manifest"),
                "bootstrap command in an action envelope");
        expectIllegalArgument(
                () -> NodeContractV1.commandEnvelope(1, 1, "not_a_command"),
                "unknown envelope command");

        for (NodeCommandKind command : NodeCommandKind.values()) {
            boolean expected = command != NodeCommandKind.GET_UI_MANIFEST;
            assertTrue(
                    NodeContractV1.isManifestAction(command) == expected,
                    "unexpected manifest action classification for " + command.wireName());
        }

        NodeContractV1.NodeCommand.GetSnapshot getSnapshot =
                new NodeContractV1.NodeCommand.GetSnapshot();
        assertSame(
                NodeCommandKind.GET_SNAPSHOT,
                getSnapshot.command,
                "get_snapshot command DTO");
        NodeContractV1.NodeCommand.GetNodeDiagnostics getDiagnostics =
                new NodeContractV1.NodeCommand.GetNodeDiagnostics();
        assertSame(
                NodeCommandKind.GET_NODE_DIAGNOSTICS,
                getDiagnostics.command,
                "get_node_diagnostics command DTO");
        NodeContractV1.NodeCommand.CleanupNodeResiduals cleanup =
                new NodeContractV1.NodeCommand.CleanupNodeResiduals(
                        new NodeContractV1.NodeCommand.CleanupNodeResiduals.CleanupNodeResidualsPayload(
                                false, Collections.emptyList()));
        assertSame(
                NodeCommandKind.CLEANUP_NODE_RESIDUALS,
                cleanup.command,
                "cleanup_node_residuals command DTO");
        NodeContractV1.NodeCreateNetworkCommand createNetwork =
                new NodeContractV1.NodeCreateNetworkCommand(
                        true,
                        Collections.emptyList(),
                        null,
                        null,
                        null,
                        null,
                        null,
                        "network",
                        "invite_only",
                        "en",
                        "lab",
                        "A test network",
                        "CN",
                        41001,
                        41000,
                        Collections.emptyList(),
                        NodeContractV1.NetworkVisibility.UNLISTED);
        NodeContractV1.NodeCommand.CreateNetwork createCommand =
                new NodeContractV1.NodeCommand.CreateNetwork(createNetwork);
        assertSame(
                NodeCommandKind.CREATE_NETWORK,
                createCommand.command,
                "create_network command DTO");
        assertSame(createNetwork, createCommand.payload, "create_network payload identity");
        NodeContractV1.NodeRotateNodeIdentityCommand rotateIdentity =
                new NodeContractV1.NodeRotateNodeIdentityCommand("fingerprint");
        NodeContractV1.NodeCommand.RotateNodeIdentity rotateCommand =
                new NodeContractV1.NodeCommand.RotateNodeIdentity(rotateIdentity);
        assertSame(
                NodeCommandKind.ROTATE_NODE_IDENTITY,
                rotateCommand.command,
                "rotate_node_identity command DTO");
        assertSame(
                rotateIdentity,
                rotateCommand.payload,
                "rotate_node_identity payload identity");
        NodeContractV1.NodeSetLocalExitCapabilityCommand localExitCommandPayload =
                new NodeContractV1.NodeSetLocalExitCapabilityCommand(true);
        NodeContractV1.NodeCommand.SetLocalExitCapability localExitCommand =
                new NodeContractV1.NodeCommand.SetLocalExitCapability(localExitCommandPayload);
        assertSame(
                NodeCommandKind.SET_LOCAL_EXIT_CAPABILITY,
                localExitCommand.command,
                "set_local_exit_capability command DTO");
        assertSame(
                localExitCommandPayload,
                localExitCommand.payload,
                "set_local_exit_capability payload identity");

        NodeContractV1.NodeCommandOutcome.Completed completed =
                new NodeContractV1.NodeCommandOutcome.Completed(
                        new NodeContractV1.NodeCommandOutcome.Completed.CompletedData("done"));
        assertSame("completed", completed.status, "completed outcome tag");
        assertSame("done", completed.data.message, "completed outcome payload");

        new NodeContractV1.NodeCommandOutcome.Snapshot(null);
        new NodeContractV1.NodeCommandOutcome.UiManifest(null);
        new NodeContractV1.NodeCommandOutcome.Diagnostics(null);
        new NodeContractV1.NodeCommandOutcome.Cleanup(null);
        new NodeContractV1.NodeCommandOutcome.Probe(null);
        new NodeContractV1.NodeCommandOutcome.Configuration(null);
        new NodeContractV1.NodeCommandOutcome.ConfigurationPreview(null);
        new NodeContractV1.NodeCommandOutcome.ConfigurationApplied(null);
        new NodeContractV1.NodeCommandOutcome.ConfigurationHistory(null);
        new NodeContractV1.NodeCommandOutcome.IdentityRotated(
                new NodeContractV1.NodeIdentityRotationSnapshot(
                        0L, 0L, "new", "new-node", "old", "old-node", false, false));
        NodeContractV1.NodeCommandOutcome.LocalExitCapability localExitOutcome =
                new NodeContractV1.NodeCommandOutcome.LocalExitCapability(null);
        assertSame(
                "local_exit_capability", localExitOutcome.status,
                "local_exit_capability outcome tag");
        new NodeContractV1.NodeCommandOutcome.Accepted(null);
        new NodeContractV1.NodeCommandOutcome.Rejected(null);

        new NodeContractV1.NodeEventKind.StateChanged(null);
        new NodeContractV1.NodeEventKind.NetworkChanged(null);
        new NodeContractV1.NodeEventKind.EmbeddedServiceChanged(null);
        new NodeContractV1.NodeEventKind.LogAppended(null);
        new NodeContractV1.NodeEventKind.RoutingChanged(null);
        new NodeContractV1.NodeEventKind.RoleChanged(null);
        new NodeContractV1.NodeEventKind.CommandCompleted(null);

        assertSame(
                "default", new NodeContractV1.NodeNetworkSelection.Default().kind,
                "default network selection tag");
        assertSame(
                "all", new NodeContractV1.NodeNetworkSelection.All().kind,
                "all network selection tag");
        assertSame(
                "none", new NodeContractV1.NodeNetworkSelection.None().kind,
                "none network selection tag");
        assertSame(
                "named", new NodeContractV1.NodeNetworkSelection.Named(null).kind,
                "named network selection tag");

        assertSame(
                "direct", new NodeContractV1.NodeRoutingDefaultAction.Direct().kind,
                "direct routing action tag");
        assertSame(
                "exit", new NodeContractV1.NodeRoutingDefaultAction.Exit("exit-node").kind,
                "exit routing action tag");
        assertSame(
                "network",
                new NodeContractV1.NodeRoutingTargetSnapshot.Network("home").kind,
                "network routing target tag");
        assertSame(
                "exit",
                new NodeContractV1.NodeRoutingTargetSnapshot.Exit("exit-node").kind,
                "exit routing target tag");

        new NodeContractV1.NodeWsMessage.Snapshot(1, null);
        new NodeContractV1.NodeWsMessage.Event(null, 1);
        new NodeContractV1.NodeWsMessage.Resync(null, 1, null);
        new NodeContractV1.NodeWsMessage.Error(
                null, null, "error", "error.operation_internal", null, null, null, false, 1);

        for (String field : new String[] {
            "candidates", "credential", "effective_from_unix_ms", "expected_revision",
            "expires_at_unix_ms", "network_name"
        }) {
            NodeContractV1.NodeUpdateCoordinatorCandidatesCommand.class.getDeclaredField(field);
        }
        for (String field : new String[] {
            "fencing_token", "leader", "network_id", "term"
        }) {
            NodeContractV1.CoordinatorWriteCredential.class.getDeclaredField(field);
        }
        for (String field : new String[] {
            "lease_expires_at_unix_ms", "max_clock_skew_ms", "observed_clock_skew_ms",
            "reason_code", "remaining_ms", "state"
        }) {
            NodeContractV1.CoordinatorLeaseStatus.class.getDeclaredField(field);
        }
        NodeContractV1.CoordinatorCandidateStatus.class.getDeclaredField("lease");
        NodeContractV1.CoordinatorCandidateStatus.class.getDeclaredField("latest_election");
        for (String field : new String[] {
            "candidate_config_revision", "candidate_group", "expires_at_unix_ms", "leader",
            "network_id", "proposal_id", "proposed_at_unix_ms", "proposer",
            "proposer_public_key", "schema_version", "signature", "term"
        }) {
            NodeContractV1.CoordinatorElectionProposal.class.getDeclaredField(field);
        }
        for (String field : new String[] {
            "deadline_unix_ms", "participants", "proposal", "rejection_reason", "result_term",
            "schema_version", "stage", "started_at_unix_ms", "updated_at_unix_ms", "votes"
        }) {
            NodeContractV1.CoordinatorElectionSession.class.getDeclaredField(field);
        }
        for (String field : new String[] {
            "authority", "candidate_config_revision", "candidate_group", "expires_at_unix_ms",
            "fencing_token", "issued_at_unix_ms", "leader", "majority_proof", "majority_votes",
            "network_id", "phase", "term"
        }) {
            NodeContractV1.CoordinatorTerm.class.getDeclaredField(field);
        }
        for (String field : new String[] {
            "candidate_config_revision", "candidate_group", "expires_at_unix_ms",
            "issued_at_unix_ms", "leader", "network_id", "schema_version", "signature", "term",
            "voter", "voter_public_key"
        }) {
            NodeContractV1.CoordinatorElectionVote.class.getDeclaredField(field);
        }
        assertSame(
                CoordinatorElectionStage.COMMITTED,
                CoordinatorElectionStage.fromWireName("committed"),
                "committed election stage wire value");

        System.out.println("NodeContractV1Test: passed");
    }

    private NodeContractV1Test() {}
}
