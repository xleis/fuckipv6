// Generated from contracts/schema/*.schema.json. Do not edit by hand.
// Regenerate with scripts/generate-contracts.sh.
package cc.xlei.fuckipv6.contract;

import java.util.List;
import java.util.Map;

/** Versioned Android-safe DTO and enum bindings for the node wire contract. */
public final class NodeContractV1 {
    public static final int SCHEMA_VERSION = 1;
    public static final String[] DTO_CLASS_NAMES = {
        "ActivityEntry", "ConfigurationApplyResult", "ConfigurationDiffEntry", "ConfigurationDocument", "ConfigurationHistoryEntry", "ConfigurationPreview", "ConfigurationValidationIssue", "ConfirmedTrafficDirectFallback", "DomainActorRef", "DomainObjectRef", "EffectiveAccess", "ExitNodeCapabilities", "FieldPrivacyPolicy", "FieldVisibilityRule", "InvitationPreview", "InvitationRecord", "InvitationRedeemRequest", "InvitationRedemption", "InvitationStatusView", "IssuedInvitation", "JoinApplicantProfile", "JoinRequest", "JoinRequestGrantPreview", "MemberLifecycleChange", "MemberRemovalPreview", "MembershipTerms", "NetworkAcl", "NetworkAclGrant", "NetworkAdminProfileProjection", "NetworkDiscoveryDocument", "NetworkDiscoveryRecord", "NetworkEndpoint", "NetworkLocalSecretProjection", "NetworkMember", "NetworkMemberDirectory", "NetworkMemberPresence", "NetworkMemberProfileProjection", "NetworkProfileApplyResult", "NetworkProfileDiffEntry", "NetworkProfilePreview", "NetworkProfileProjections", "NetworkProfileUpdate", "NetworkProfileWarning", "NetworkPublicProfileDocument", "NetworkPublicProfileProjection", "NetworkPublicProfileRecord", "NodeAccessControlSnapshot", "NodeAcknowledgeMembershipPolicyCommand", "NodeApiCommandResponse", "NodeApiErrorResponse", "NodeApiManifestResponse", "NodeApiObjectResponse", "NodeApiOperationResponse", "NodeApiSnapshotResponse", "NodeApplyConfigurationCommand", "NodeApplyPolicyCommand", "NodeApplyTrafficPolicyCommand", "NodeBatchReviewJoinRequestsCommand", "NodeBlacklistSnapshot", "NodeCapabilityRegistrySnapshot", "NodeCapabilitySnapshot", "NodeChangeMemberLifecycleCommand", "NodeCleanupFailureSnapshot", "NodeCleanupStepSnapshot", "NodeClientBlacklistCommand", "NodeCommandHttpRequest", "NodeCommandHttpResponse", "NodeCommandRequest", "NodeCommandResult", "NodeConfigurationSnapshot", "NodeContributionVeto", "NodeCreateInvitationCommand", "NodeCreateNetworkCommand", "NodeCreatedNetwork", "NodeDiagnosticCheck", "NodeDiagnosticRecoveryAction", "NodeDiagnosticsSnapshot", "NodeDiscoveryIndexError", "NodeDiscoveryIndexMetrics", "NodeDiscoveryIndexRecordSnapshot", "NodeDiscoveryIndexSnapshot", "NodeEmbeddedServiceSnapshot", "NodeEvent", "NodeEventCursor", "NodeGetInvitationStatusCommand", "NodeHttpContract", "NodeHttpErrorResponse", "NodeIdentityRotationSnapshot", "NodeIdentitySnapshot", "NodeInterfaceAddress", "NodeInterfaceSnapshot", "NodeJoinNetworkCommand", "NodeJoinRequestListResult", "NodeJoinRequestReviewAck", "NodeJoinRequestReviewItem", "NodeJoinRequestReviewOutcome", "NodeLeaveNetworkCommand", "NodeLifecycleSnapshot", "NodeListInvitationsCommand", "NodeListJoinRequestsCommand", "NodeLocalAccessPolicy", "NodeLogEntry", "NodeManifestResponse", "NodeMemberRemovalPreviewCommand", "NodeNetworkAllocation", "NodeNetworkAllocationSnapshot", "NodeNetworkConfigSnapshot", "NodeNetworkDiscoveryCommand", "NodeNetworkJoinPreview", "NodeNetworkJoinResult", "NodeNetworkLeaveResult", "NodeNetworkMembersCommand", "NodeNetworkMembershipSnapshot", "NodeNetworkProfileCommand", "NodeNetworkProfileSnapshot", "NodeNetworkSnapshot", "NodeOperationSnapshot", "NodePeerProbeSnapshot", "NodePeerSnapshot", "NodePermissionSnapshot", "NodePreviewConfigurationCommand", "NodePreviewInvitationCommand", "NodePreviewJoinNetworkCommand", "NodePreviewPolicyCommand", "NodePreviewPolicyRollbackCommand", "NodePreviewTrafficPolicyCommand", "NodePreviewTrafficPolicyRollbackCommand", "NodeProbePeerCommand", "NodeProblem", "NodeProblemDetails", "NodeRedeemInvitationCommand", "NodeResidualCleanupResult", "NodeResubmitJoinRequestCommand", "NodeReviewJoinRequestCommand", "NodeRevokeInvitationCommand", "NodeRollbackConfigurationCommand", "NodeRollbackPolicyCommand", "NodeRollbackTrafficPolicyCommand", "NodeRotateNodeIdentityCommand", "NodeRoutingSummarySnapshot", "NodeRuntimeResidueSnapshot", "NodeSetNetworkRoleCommand", "NodeSetNodeDisplayNameCommand", "NodeSignedNetworkProfileSnapshot", "NodeSnapshot", "NodeSnapshotResponse", "NodeSubmitJoinRequestCommand", "NodeTestTrafficPolicyCommand", "NodeTrafficSnapshot", "NodeUiAction", "NodeUiAudit", "NodeUiConfirmation", "NodeUiField", "NodeUiManifest", "NodeUiPage", "NodeUpdateNetworkAllocationCommand", "NodeUpdateRoutingCommand", "NodeValidationIssue", "NodeWithdrawJoinRequestCommand", "OperationError", "OperationRecovery", "PermissionRule", "PolicyAffectedObject", "PolicyApplyResult", "PolicyCurrent", "PolicyDiffEntry", "PolicyHistoryEntry", "PolicyHistorySnapshot", "PolicyPrecedenceExplanation", "PolicyPreview", "PolicyRiskSummary", "PolicyRollbackResult", "PolicyRuleExplanation", "PolicySnapshot", "PolicyTestInput", "PolicyTestRequest", "PolicyTestResult", "PolicyValidationIssue", "PortRange", "ProjectedActivityEntry", "ProjectedNodeCommandResult", "ProjectedNodeEmbeddedServiceSnapshot", "ProjectedNodeEvent", "ProjectedNodeIdentitySnapshot", "ProjectedNodeInterfaceSnapshot", "ProjectedNodeNetworkJoinResult", "ProjectedNodeNetworkLeaveResult", "ProjectedNodeNetworkSnapshot", "ProjectedNodePeerSnapshot", "ProjectedNodeSnapshot", "TopologyBoundary", "TopologyCluster", "TopologyConnection", "TopologyNode", "TopologyNodeNetwork", "TopologySnapshot", "TrafficDefaultPolicy", "TrafficFailureDecision", "TrafficFailurePolicy", "TrafficFlow", "TrafficMatch", "TrafficPolicy", "TrafficPolicyAffectedObject", "TrafficPolicyApplyResult", "TrafficPolicyCurrent", "TrafficPolicyDiffEntry", "TrafficPolicyDraft", "TrafficPolicyHistoryEntry", "TrafficPolicyHistorySnapshot", "TrafficPolicyPreview", "TrafficPolicyRollbackResult", "TrafficPolicyTestComparison", "TrafficPolicyTestDelta", "TrafficPolicyTestInput", "TrafficRule", "TrafficRuleRisk", "TunCapabilities", "UiModeResolution", "ApplicationIdentity", "NodeApiEvent", "NodeApiObject", "NodeCommand", "NodeCommandOutcome", "NodeEventKind", "NodeNetworkScope", "NodeNetworkSelection", "NodeRoutingDefaultAction", "NodeRoutingTargetSnapshot", "NodeWsMessage", "PermissionCondition", "ProjectedNodeCommandOutcome", "ProjectedNodeEventKind", "SubjectSelector", "TrafficAction", "TrafficMatchRisk", "TrafficPolicyTestContextSource", "TrafficPolicyTestDecision"
    };

    public enum AclAction {
        ALLOW("allow"),
        DENY("deny");

        private final String wireName;

        AclAction(String wireName) {
            this.wireName = wireName;
        }

        public String wireName() {
            return wireName;
        }

        public static AclAction fromWireName(String value) {
            for (AclAction candidate : values()) {
                if (candidate.wireName.equals(value)) {
                    return candidate;
                }
            }
            throw new IllegalArgumentException("unknown AclAction wire value: " + value);
        }
    }

    public enum ActivityCategory {
        USER_ACTIVITY("user_activity"),
        SYSTEM_EVENT("system_event"),
        SECURITY_EVENT("security_event"),
        OPERATION_FAILURE("operation_failure");

        private final String wireName;

        ActivityCategory(String wireName) {
            this.wireName = wireName;
        }

        public String wireName() {
            return wireName;
        }

        public static ActivityCategory fromWireName(String value) {
            for (ActivityCategory candidate : values()) {
                if (candidate.wireName.equals(value)) {
                    return candidate;
                }
            }
            throw new IllegalArgumentException("unknown ActivityCategory wire value: " + value);
        }
    }

    public enum ActivitySeverity {
        INFO("info"),
        NOTICE("notice"),
        WARNING("warning"),
        CRITICAL("critical");

        private final String wireName;

        ActivitySeverity(String wireName) {
            this.wireName = wireName;
        }

        public String wireName() {
            return wireName;
        }

        public static ActivitySeverity fromWireName(String value) {
            for (ActivitySeverity candidate : values()) {
                if (candidate.wireName.equals(value)) {
                    return candidate;
                }
            }
            throw new IllegalArgumentException("unknown ActivitySeverity wire value: " + value);
        }
    }

    public enum AuditOutcome {
        SUCCEEDED("succeeded"),
        FAILED("failed"),
        DENIED("denied");

        private final String wireName;

        AuditOutcome(String wireName) {
            this.wireName = wireName;
        }

        public String wireName() {
            return wireName;
        }

        public static AuditOutcome fromWireName(String value) {
            for (AuditOutcome candidate : values()) {
                if (candidate.wireName.equals(value)) {
                    return candidate;
                }
            }
            throw new IllegalArgumentException("unknown AuditOutcome wire value: " + value);
        }
    }

    public enum CapabilityKind {
        COORDINATOR("coordinator"),
        RELAY("relay"),
        EXIT("exit"),
        FILE("file");

        private final String wireName;

        CapabilityKind(String wireName) {
            this.wireName = wireName;
        }

        public String wireName() {
            return wireName;
        }

        public static CapabilityKind fromWireName(String value) {
            for (CapabilityKind candidate : values()) {
                if (candidate.wireName.equals(value)) {
                    return candidate;
                }
            }
            throw new IllegalArgumentException("unknown CapabilityKind wire value: " + value);
        }
    }

    public enum CapabilityState {
        DISABLED("disabled"),
        STARTING("starting"),
        ENABLED("enabled"),
        DEGRADED("degraded"),
        UNAVAILABLE("unavailable"),
        UNSUPPORTED("unsupported"),
        UNKNOWN("unknown");

        private final String wireName;

        CapabilityState(String wireName) {
            this.wireName = wireName;
        }

        public String wireName() {
            return wireName;
        }

        public static CapabilityState fromWireName(String value) {
            for (CapabilityState candidate : values()) {
                if (candidate.wireName.equals(value)) {
                    return candidate;
                }
            }
            throw new IllegalArgumentException("unknown CapabilityState wire value: " + value);
        }
    }

    public enum ConfigurationChangeAction {
        APPLY("apply"),
        ROLLBACK("rollback");

        private final String wireName;

        ConfigurationChangeAction(String wireName) {
            this.wireName = wireName;
        }

        public String wireName() {
            return wireName;
        }

        public static ConfigurationChangeAction fromWireName(String value) {
            for (ConfigurationChangeAction candidate : values()) {
                if (candidate.wireName.equals(value)) {
                    return candidate;
                }
            }
            throw new IllegalArgumentException("unknown ConfigurationChangeAction wire value: " + value);
        }
    }

    public enum ConfigurationImpact {
        IMMEDIATE("immediate"),
        RESTART_REQUIRED("restart_required");

        private final String wireName;

        ConfigurationImpact(String wireName) {
            this.wireName = wireName;
        }

        public String wireName() {
            return wireName;
        }

        public static ConfigurationImpact fromWireName(String value) {
            for (ConfigurationImpact candidate : values()) {
                if (candidate.wireName.equals(value)) {
                    return candidate;
                }
            }
            throw new IllegalArgumentException("unknown ConfigurationImpact wire value: " + value);
        }
    }

    public enum ConfigurationValidationCode {
        INVALID_TYPE("invalid_type"),
        MISSING_REQUIRED("missing_required"),
        UNKNOWN_NETWORK("unknown_network"),
        MISSING_NETWORK("missing_network"),
        INVALID_VALUE("invalid_value"),
        UNSUPPORTED("unsupported");

        private final String wireName;

        ConfigurationValidationCode(String wireName) {
            this.wireName = wireName;
        }

        public String wireName() {
            return wireName;
        }

        public static ConfigurationValidationCode fromWireName(String value) {
            for (ConfigurationValidationCode candidate : values()) {
                if (candidate.wireName.equals(value)) {
                    return candidate;
                }
            }
            throw new IllegalArgumentException("unknown ConfigurationValidationCode wire value: " + value);
        }
    }

    public enum ContributionKind {
        RELAY("relay"),
        EXIT("exit"),
        FILE("file");

        private final String wireName;

        ContributionKind(String wireName) {
            this.wireName = wireName;
        }

        public String wireName() {
            return wireName;
        }

        public static ContributionKind fromWireName(String value) {
            for (ContributionKind candidate : values()) {
                if (candidate.wireName.equals(value)) {
                    return candidate;
                }
            }
            throw new IllegalArgumentException("unknown ContributionKind wire value: " + value);
        }
    }

    public enum DiscoveryTransport {
        QUIC("quic"),
        TCP("tcp"),
        WEBSOCKET("websocket"),
        LOCAL("local");

        private final String wireName;

        DiscoveryTransport(String wireName) {
            this.wireName = wireName;
        }

        public String wireName() {
            return wireName;
        }

        public static DiscoveryTransport fromWireName(String value) {
            for (DiscoveryTransport candidate : values()) {
                if (candidate.wireName.equals(value)) {
                    return candidate;
                }
            }
            throw new IllegalArgumentException("unknown DiscoveryTransport wire value: " + value);
        }
    }

    public enum DisplayEnvironment {
        LINUX_X11("linux_x11"),
        LINUX_WAYLAND("linux_wayland"),
        WINDOWS_DESKTOP("windows_desktop"),
        MACOS_DESKTOP("macos_desktop"),
        HEADLESS("headless");

        private final String wireName;

        DisplayEnvironment(String wireName) {
            this.wireName = wireName;
        }

        public String wireName() {
            return wireName;
        }

        public static DisplayEnvironment fromWireName(String value) {
            for (DisplayEnvironment candidate : values()) {
                if (candidate.wireName.equals(value)) {
                    return candidate;
                }
            }
            throw new IllegalArgumentException("unknown DisplayEnvironment wire value: " + value);
        }
    }

    public enum EffectiveAccessReason {
        ALLOWED("allowed"),
        CROSS_NETWORK_DENIED("cross_network_denied"),
        NETWORK_SCOPE_RESTRICTED("network_scope_restricted"),
        LOCAL_CONTRIBUTION_VETO("local_contribution_veto"),
        AUTHORIZATION_EXPIRED("authorization_expired"),
        AUTHORIZATION_REVOKED("authorization_revoked"),
        RECENT_REAUTHENTICATION_REQUIRED("recent_reauthentication_required"),
        EXPLICIT_DENY("explicit_deny"),
        ROLE_CEILING_EXCEEDED("role_ceiling_exceeded"),
        TEMPORARY_COORDINATOR_READ_ONLY("temporary_coordinator_read_only"),
        DEFAULT_DENIED("default_denied"),
        INVALID_POLICY("invalid_policy");

        private final String wireName;

        EffectiveAccessReason(String wireName) {
            this.wireName = wireName;
        }

        public String wireName() {
            return wireName;
        }

        public static EffectiveAccessReason fromWireName(String value) {
            for (EffectiveAccessReason candidate : values()) {
                if (candidate.wireName.equals(value)) {
                    return candidate;
                }
            }
            throw new IllegalArgumentException("unknown EffectiveAccessReason wire value: " + value);
        }
    }

    public enum EffectivePolicyLayer {
        LOCAL("local"),
        NETWORK("network"),
        REQUEST("request");

        private final String wireName;

        EffectivePolicyLayer(String wireName) {
            this.wireName = wireName;
        }

        public String wireName() {
            return wireName;
        }

        public static EffectivePolicyLayer fromWireName(String value) {
            for (EffectivePolicyLayer candidate : values()) {
                if (candidate.wireName.equals(value)) {
                    return candidate;
                }
            }
            throw new IllegalArgumentException("unknown EffectivePolicyLayer wire value: " + value);
        }
    }

    public enum ExitContributionObligation {
        REQUIRED("required"),
        NOT_REQUIRED("not_required");

        private final String wireName;

        ExitContributionObligation(String wireName) {
            this.wireName = wireName;
        }

        public String wireName() {
            return wireName;
        }

        public static ExitContributionObligation fromWireName(String value) {
            for (ExitContributionObligation candidate : values()) {
                if (candidate.wireName.equals(value)) {
                    return candidate;
                }
            }
            throw new IllegalArgumentException("unknown ExitContributionObligation wire value: " + value);
        }
    }

    public enum ExitContributionScope {
        NOT_APPLICABLE("not_applicable"),
        JOINING_MEMBERS("joining_members"),
        ALL_MEMBERS("all_members");

        private final String wireName;

        ExitContributionScope(String wireName) {
            this.wireName = wireName;
        }

        public String wireName() {
            return wireName;
        }

        public static ExitContributionScope fromWireName(String value) {
            for (ExitContributionScope candidate : values()) {
                if (candidate.wireName.equals(value)) {
                    return candidate;
                }
            }
            throw new IllegalArgumentException("unknown ExitContributionScope wire value: " + value);
        }
    }

    public enum ExitNodePlatform {
        LINUX("linux"),
        WINDOWS("windows"),
        MACOS("macos"),
        OTHER("other");

        private final String wireName;

        ExitNodePlatform(String wireName) {
            this.wireName = wireName;
        }

        public String wireName() {
            return wireName;
        }

        public static ExitNodePlatform fromWireName(String value) {
            for (ExitNodePlatform candidate : values()) {
                if (candidate.wireName.equals(value)) {
                    return candidate;
                }
            }
            throw new IllegalArgumentException("unknown ExitNodePlatform wire value: " + value);
        }
    }

    public enum ExitNodeUnsupportedReason {
        PLATFORM_NOT_LINUX("platform_not_linux"),
        PRIVILEGES_REQUIRED("privileges_required"),
        ROUTING_TOOL_UNAVAILABLE("routing_tool_unavailable"),
        IPTABLES_UNAVAILABLE("iptables_unavailable"),
        IPV4_FORWARDING_UNAVAILABLE("ipv4_forwarding_unavailable");

        private final String wireName;

        ExitNodeUnsupportedReason(String wireName) {
            this.wireName = wireName;
        }

        public String wireName() {
            return wireName;
        }

        public static ExitNodeUnsupportedReason fromWireName(String value) {
            for (ExitNodeUnsupportedReason candidate : values()) {
                if (candidate.wireName.equals(value)) {
                    return candidate;
                }
            }
            throw new IllegalArgumentException("unknown ExitNodeUnsupportedReason wire value: " + value);
        }
    }

    public enum FieldVisibility {
        PUBLIC("public"),
        NETWORK("network"),
        ADMINISTRATORS("administrators"),
        OWNER_ONLY("owner_only");

        private final String wireName;

        FieldVisibility(String wireName) {
            this.wireName = wireName;
        }

        public String wireName() {
            return wireName;
        }

        public static FieldVisibility fromWireName(String value) {
            for (FieldVisibility candidate : values()) {
                if (candidate.wireName.equals(value)) {
                    return candidate;
                }
            }
            throw new IllegalArgumentException("unknown FieldVisibility wire value: " + value);
        }
    }

    public enum InvitationState {
        ACTIVE("active"),
        EXHAUSTED("exhausted"),
        EXPIRED("expired"),
        REVOKED("revoked");

        private final String wireName;

        InvitationState(String wireName) {
            this.wireName = wireName;
        }

        public String wireName() {
            return wireName;
        }

        public static InvitationState fromWireName(String value) {
            for (InvitationState candidate : values()) {
                if (candidate.wireName.equals(value)) {
                    return candidate;
                }
            }
            throw new IllegalArgumentException("unknown InvitationState wire value: " + value);
        }
    }

    public enum JoinRequestReasonVisibility {
        OWNERS_AND_ADMINISTRATORS("owners_and_administrators");

        private final String wireName;

        JoinRequestReasonVisibility(String wireName) {
            this.wireName = wireName;
        }

        public String wireName() {
            return wireName;
        }

        public static JoinRequestReasonVisibility fromWireName(String value) {
            for (JoinRequestReasonVisibility candidate : values()) {
                if (candidate.wireName.equals(value)) {
                    return candidate;
                }
            }
            throw new IllegalArgumentException("unknown JoinRequestReasonVisibility wire value: " + value);
        }
    }

    public enum JoinRequestReviewAction {
        APPROVE("approve"),
        REJECT("reject"),
        REQUEST_INFORMATION("request_information");

        private final String wireName;

        JoinRequestReviewAction(String wireName) {
            this.wireName = wireName;
        }

        public String wireName() {
            return wireName;
        }

        public static JoinRequestReviewAction fromWireName(String value) {
            for (JoinRequestReviewAction candidate : values()) {
                if (candidate.wireName.equals(value)) {
                    return candidate;
                }
            }
            throw new IllegalArgumentException("unknown JoinRequestReviewAction wire value: " + value);
        }
    }

    public enum JoinRequestState {
        PENDING("pending"),
        NEEDS_INFORMATION("needs_information"),
        APPROVED("approved"),
        REJECTED("rejected"),
        WITHDRAWN("withdrawn"),
        EXPIRED("expired");

        private final String wireName;

        JoinRequestState(String wireName) {
            this.wireName = wireName;
        }

        public String wireName() {
            return wireName;
        }

        public static JoinRequestState fromWireName(String value) {
            for (JoinRequestState candidate : values()) {
                if (candidate.wireName.equals(value)) {
                    return candidate;
                }
            }
            throw new IllegalArgumentException("unknown JoinRequestState wire value: " + value);
        }
    }

    public enum MemberLifecycleAction {
        INVITE("invite"),
        APPROVE("approve"),
        REJECT("reject"),
        SUSPEND("suspend"),
        RESUME("resume"),
        EXIT("exit"),
        REMOVE("remove"),
        UNDO_REMOVE("undo_remove");

        private final String wireName;

        MemberLifecycleAction(String wireName) {
            this.wireName = wireName;
        }

        public String wireName() {
            return wireName;
        }

        public static MemberLifecycleAction fromWireName(String value) {
            for (MemberLifecycleAction candidate : values()) {
                if (candidate.wireName.equals(value)) {
                    return candidate;
                }
            }
            throw new IllegalArgumentException("unknown MemberLifecycleAction wire value: " + value);
        }
    }

    public enum MemberLifecycleStatus {
        INVITED("invited"),
        PENDING_APPROVAL("pending_approval"),
        APPROVED("approved"),
        ACTIVE("active"),
        DEGRADED("degraded"),
        SUSPENDED("suspended"),
        EXITED("exited"),
        REMOVED("removed"),
        EXPIRED("expired"),
        REJECTED("rejected");

        private final String wireName;

        MemberLifecycleStatus(String wireName) {
            this.wireName = wireName;
        }

        public String wireName() {
            return wireName;
        }

        public static MemberLifecycleStatus fromWireName(String value) {
            for (MemberLifecycleStatus candidate : values()) {
                if (candidate.wireName.equals(value)) {
                    return candidate;
                }
            }
            throw new IllegalArgumentException("unknown MemberLifecycleStatus wire value: " + value);
        }
    }

    public enum MembershipRole {
        OWNER("owner"),
        ADMINISTRATOR("administrator"),
        MEMBER("member");

        private final String wireName;

        MembershipRole(String wireName) {
            this.wireName = wireName;
        }

        public String wireName() {
            return wireName;
        }

        public static MembershipRole fromWireName(String value) {
            for (MembershipRole candidate : values()) {
                if (candidate.wireName.equals(value)) {
                    return candidate;
                }
            }
            throw new IllegalArgumentException("unknown MembershipRole wire value: " + value);
        }
    }

    public enum MembershipState {
        DISCOVERED("discovered"),
        APPLYING("applying"),
        PENDING_APPROVAL("pending_approval"),
        ACCEPTED("accepted"),
        KEY_READY("key_ready"),
        CONNECTING("connecting"),
        ACTIVE("active"),
        DEGRADED("degraded"),
        LEAVING("leaving"),
        LEFT("left"),
        REJECTED("rejected"),
        SUSPENDED("suspended"),
        CANCELLED("cancelled");

        private final String wireName;

        MembershipState(String wireName) {
            this.wireName = wireName;
        }

        public String wireName() {
            return wireName;
        }

        public static MembershipState fromWireName(String value) {
            for (MembershipState candidate : values()) {
                if (candidate.wireName.equals(value)) {
                    return candidate;
                }
            }
            throw new IllegalArgumentException("unknown MembershipState wire value: " + value);
        }
    }

    public enum NetworkJoinMode {
        OPEN("open"),
        APPROVAL_REQUIRED("approval_required"),
        INVITE_ONLY("invite_only");

        private final String wireName;

        NetworkJoinMode(String wireName) {
            this.wireName = wireName;
        }

        public String wireName() {
            return wireName;
        }

        public static NetworkJoinMode fromWireName(String value) {
            for (NetworkJoinMode candidate : values()) {
                if (candidate.wireName.equals(value)) {
                    return candidate;
                }
            }
            throw new IllegalArgumentException("unknown NetworkJoinMode wire value: " + value);
        }
    }

    public enum NetworkMemberGroup {
        OWNER("owner"),
        ADMINISTRATOR("administrator"),
        MEMBER("member");

        private final String wireName;

        NetworkMemberGroup(String wireName) {
            this.wireName = wireName;
        }

        public String wireName() {
            return wireName;
        }

        public static NetworkMemberGroup fromWireName(String value) {
            for (NetworkMemberGroup candidate : values()) {
                if (candidate.wireName.equals(value)) {
                    return candidate;
                }
            }
            throw new IllegalArgumentException("unknown NetworkMemberGroup wire value: " + value);
        }
    }

    public enum NetworkMemberObservationSource {
        LIVE_SESSION("live_session"),
        DISCONNECT_OBSERVED("disconnect_observed"),
        NOT_OBSERVED("not_observed");

        private final String wireName;

        NetworkMemberObservationSource(String wireName) {
            this.wireName = wireName;
        }

        public String wireName() {
            return wireName;
        }

        public static NetworkMemberObservationSource fromWireName(String value) {
            for (NetworkMemberObservationSource candidate : values()) {
                if (candidate.wireName.equals(value)) {
                    return candidate;
                }
            }
            throw new IllegalArgumentException("unknown NetworkMemberObservationSource wire value: " + value);
        }
    }

    public enum NetworkMemberOnlineState {
        ONLINE("online"),
        OFFLINE("offline"),
        UNKNOWN("unknown");

        private final String wireName;

        NetworkMemberOnlineState(String wireName) {
            this.wireName = wireName;
        }

        public String wireName() {
            return wireName;
        }

        public static NetworkMemberOnlineState fromWireName(String value) {
            for (NetworkMemberOnlineState candidate : values()) {
                if (candidate.wireName.equals(value)) {
                    return candidate;
                }
            }
            throw new IllegalArgumentException("unknown NetworkMemberOnlineState wire value: " + value);
        }
    }

    public enum NetworkMemberStatus {
        PENDING("pending"),
        APPROVED("approved"),
        ACTIVE("active"),
        DEGRADED("degraded"),
        SUSPENDED("suspended"),
        EXITED("exited"),
        REMOVED("removed"),
        EXPIRED("expired");

        private final String wireName;

        NetworkMemberStatus(String wireName) {
            this.wireName = wireName;
        }

        public String wireName() {
            return wireName;
        }

        public static NetworkMemberStatus fromWireName(String value) {
            for (NetworkMemberStatus candidate : values()) {
                if (candidate.wireName.equals(value)) {
                    return candidate;
                }
            }
            throw new IllegalArgumentException("unknown NetworkMemberStatus wire value: " + value);
        }
    }

    public enum NetworkMode {
        IPV4_ONLY("ipv4_only"),
        IPV6_ONLY("ipv6_only"),
        DUAL_STACK("dual_stack");

        private final String wireName;

        NetworkMode(String wireName) {
            this.wireName = wireName;
        }

        public String wireName() {
            return wireName;
        }

        public static NetworkMode fromWireName(String value) {
            for (NetworkMode candidate : values()) {
                if (candidate.wireName.equals(value)) {
                    return candidate;
                }
            }
            throw new IllegalArgumentException("unknown NetworkMode wire value: " + value);
        }
    }

    public enum NetworkProfileWarningCode {
        UNSIGNED_LEGACY("unsigned_legacy"),
        PROFILE_EXPIRED("profile_expired"),
        REVISION_CONFLICT("revision_conflict"),
        CAPABILITY_NOT_OBSERVED("capability_not_observed"),
        CAPABILITY_UNDECLARED("capability_undeclared");

        private final String wireName;

        NetworkProfileWarningCode(String wireName) {
            this.wireName = wireName;
        }

        public String wireName() {
            return wireName;
        }

        public static NetworkProfileWarningCode fromWireName(String value) {
            for (NetworkProfileWarningCode candidate : values()) {
                if (candidate.wireName.equals(value)) {
                    return candidate;
                }
            }
            throw new IllegalArgumentException("unknown NetworkProfileWarningCode wire value: " + value);
        }
    }

    public enum NetworkRole {
        OWNER("owner"),
        ADMINISTRATOR("administrator"),
        MEMBER("member"),
        TEMPORARY_COORDINATOR("temporary_coordinator");

        private final String wireName;

        NetworkRole(String wireName) {
            this.wireName = wireName;
        }

        public String wireName() {
            return wireName;
        }

        public static NetworkRole fromWireName(String value) {
            for (NetworkRole candidate : values()) {
                if (candidate.wireName.equals(value)) {
                    return candidate;
                }
            }
            throw new IllegalArgumentException("unknown NetworkRole wire value: " + value);
        }
    }

    public enum NetworkVisibility {
        PUBLIC("public"),
        UNLISTED("unlisted"),
        PRIVATE("private");

        private final String wireName;

        NetworkVisibility(String wireName) {
            this.wireName = wireName;
        }

        public String wireName() {
            return wireName;
        }

        public static NetworkVisibility fromWireName(String value) {
            for (NetworkVisibility candidate : values()) {
                if (candidate.wireName.equals(value)) {
                    return candidate;
                }
            }
            throw new IllegalArgumentException("unknown NetworkVisibility wire value: " + value);
        }
    }

    public enum NodeAccessModel {
        TRUSTED_CONTROL_PLANE("trusted_control_plane");

        private final String wireName;

        NodeAccessModel(String wireName) {
            this.wireName = wireName;
        }

        public String wireName() {
            return wireName;
        }

        public static NodeAccessModel fromWireName(String value) {
            for (NodeAccessModel candidate : values()) {
                if (candidate.wireName.equals(value)) {
                    return candidate;
                }
            }
            throw new IllegalArgumentException("unknown NodeAccessModel wire value: " + value);
        }
    }

    public enum NodeCleanupFailureReason {
        STILL_RUNNING("still_running"),
        OWNER_UNAVAILABLE("owner_unavailable"),
        TIMED_OUT("timed_out"),
        INCOMPLETE("incomplete"),
        UNKNOWN("unknown");

        private final String wireName;

        NodeCleanupFailureReason(String wireName) {
            this.wireName = wireName;
        }

        public String wireName() {
            return wireName;
        }

        public static NodeCleanupFailureReason fromWireName(String value) {
            for (NodeCleanupFailureReason candidate : values()) {
                if (candidate.wireName.equals(value)) {
                    return candidate;
                }
            }
            throw new IllegalArgumentException("unknown NodeCleanupFailureReason wire value: " + value);
        }
    }

    public enum NodeCleanupStepStatus {
        REMOVED("removed"),
        NOT_OWNED("not_owned"),
        DEFERRED("deferred"),
        FAILED("failed"),
        UNKNOWN("unknown");

        private final String wireName;

        NodeCleanupStepStatus(String wireName) {
            this.wireName = wireName;
        }

        public String wireName() {
            return wireName;
        }

        public static NodeCleanupStepStatus fromWireName(String value) {
            for (NodeCleanupStepStatus candidate : values()) {
                if (candidate.wireName.equals(value)) {
                    return candidate;
                }
            }
            throw new IllegalArgumentException("unknown NodeCleanupStepStatus wire value: " + value);
        }
    }

    public enum NodeCommandKind {
        ACKNOWLEDGE_MEMBERSHIP_POLICY("acknowledge_membership_policy"),
        APPLY_CONFIGURATION("apply_configuration"),
        APPLY_POLICY("apply_policy"),
        APPLY_TRAFFIC_POLICY("apply_traffic_policy"),
        BAN_CLIENT("ban_client"),
        BATCH_REVIEW_JOIN_REQUESTS("batch_review_join_requests"),
        CHANGE_MEMBER_LIFECYCLE("change_member_lifecycle"),
        CLEANUP_NODE_RESIDUALS("cleanup_node_residuals"),
        CREATE_INVITATION("create_invitation"),
        CREATE_NETWORK("create_network"),
        GET_BLACKLIST("get_blacklist"),
        GET_CONFIGURATION("get_configuration"),
        GET_CONFIGURATION_HISTORY("get_configuration_history"),
        GET_INVITATION_STATUS("get_invitation_status"),
        GET_NETWORK_ALLOCATION("get_network_allocation"),
        GET_NODE_DIAGNOSTICS("get_node_diagnostics"),
        GET_POLICY("get_policy"),
        GET_POLICY_HISTORY("get_policy_history"),
        GET_SNAPSHOT("get_snapshot"),
        GET_TRAFFIC_POLICY("get_traffic_policy"),
        GET_TRAFFIC_POLICY_HISTORY("get_traffic_policy_history"),
        GET_UI_MANIFEST("get_ui_manifest"),
        JOIN_NETWORK("join_network"),
        LEAVE_NETWORK("leave_network"),
        LIST_INVITATIONS("list_invitations"),
        LIST_JOIN_REQUESTS("list_join_requests"),
        LIST_NETWORK_MEMBERS("list_network_members"),
        PREVIEW_CONFIGURATION("preview_configuration"),
        PREVIEW_INVITATION("preview_invitation"),
        PREVIEW_JOIN_NETWORK("preview_join_network"),
        PREVIEW_MEMBER_REMOVAL("preview_member_removal"),
        PREVIEW_NETWORK_PROFILE("preview_network_profile"),
        PREVIEW_POLICY("preview_policy"),
        PREVIEW_POLICY_ROLLBACK("preview_policy_rollback"),
        PREVIEW_TRAFFIC_POLICY("preview_traffic_policy"),
        PREVIEW_TRAFFIC_POLICY_ROLLBACK("preview_traffic_policy_rollback"),
        PROBE_PEER("probe_peer"),
        PUBLISH_NETWORK_DISCOVERY("publish_network_discovery"),
        REBUILD_TUN("rebuild_tun"),
        REDEEM_INVITATION("redeem_invitation"),
        REFRESH_DISCOVERY_INDEX("refresh_discovery_index"),
        RESTART_NETWORKS("restart_networks"),
        RESTART_NODE("restart_node"),
        RESUBMIT_JOIN_REQUEST("resubmit_join_request"),
        RETRY_DISCOVERY_INDEX("retry_discovery_index"),
        REVIEW_JOIN_REQUEST("review_join_request"),
        REVOKE_INVITATION("revoke_invitation"),
        ROLLBACK_CONFIGURATION("rollback_configuration"),
        ROLLBACK_POLICY("rollback_policy"),
        ROLLBACK_TRAFFIC_POLICY("rollback_traffic_policy"),
        ROTATE_NODE_IDENTITY("rotate_node_identity"),
        SET_NETWORK_ROLE("set_network_role"),
        SET_NODE_DISPLAY_NAME("set_node_display_name"),
        SHUTDOWN("shutdown"),
        START_NETWORKS("start_networks"),
        START_NODE("start_node"),
        STOP_NETWORKS("stop_networks"),
        STOP_NODE("stop_node"),
        SUBMIT_JOIN_REQUEST("submit_join_request"),
        TEST_POLICY("test_policy"),
        TEST_TRAFFIC_POLICY("test_traffic_policy"),
        UNBAN_CLIENT("unban_client"),
        UNPUBLISH_NETWORK_DISCOVERY("unpublish_network_discovery"),
        UPDATE_NETWORK_ALLOCATION("update_network_allocation"),
        UPDATE_NETWORK_DISCOVERY("update_network_discovery"),
        UPDATE_NETWORK_PROFILE("update_network_profile"),
        UPDATE_ROUTING("update_routing"),
        WITHDRAW_JOIN_REQUEST("withdraw_join_request");

        private final String wireName;

        NodeCommandKind(String wireName) {
            this.wireName = wireName;
        }

        public String wireName() {
            return wireName;
        }

        public static NodeCommandKind fromWireName(String value) {
            for (NodeCommandKind candidate : values()) {
                if (candidate.wireName.equals(value)) {
                    return candidate;
                }
            }
            throw new IllegalArgumentException("unknown NodeCommandKind wire value: " + value);
        }
    }

    public enum NodeControlHealth {
        UNKNOWN("unknown"),
        AVAILABLE("available"),
        UNAVAILABLE("unavailable");

        private final String wireName;

        NodeControlHealth(String wireName) {
            this.wireName = wireName;
        }

        public String wireName() {
            return wireName;
        }

        public static NodeControlHealth fromWireName(String value) {
            for (NodeControlHealth candidate : values()) {
                if (candidate.wireName.equals(value)) {
                    return candidate;
                }
            }
            throw new IllegalArgumentException("unknown NodeControlHealth wire value: " + value);
        }
    }

    public enum NodeDiagnosticLayer {
        NETWORK("network"),
        DNS("dns"),
        TUN("tun"),
        COORDINATOR("coordinator"),
        RELAY("relay"),
        EXIT("exit");

        private final String wireName;

        NodeDiagnosticLayer(String wireName) {
            this.wireName = wireName;
        }

        public String wireName() {
            return wireName;
        }

        public static NodeDiagnosticLayer fromWireName(String value) {
            for (NodeDiagnosticLayer candidate : values()) {
                if (candidate.wireName.equals(value)) {
                    return candidate;
                }
            }
            throw new IllegalArgumentException("unknown NodeDiagnosticLayer wire value: " + value);
        }
    }

    public enum NodeDiagnosticState {
        NOT_TESTED("not_tested"),
        PASSED("passed"),
        FAILED("failed"),
        UNSUPPORTED("unsupported"),
        NO_PERMISSION("no_permission");

        private final String wireName;

        NodeDiagnosticState(String wireName) {
            this.wireName = wireName;
        }

        public String wireName() {
            return wireName;
        }

        public static NodeDiagnosticState fromWireName(String value) {
            for (NodeDiagnosticState candidate : values()) {
                if (candidate.wireName.equals(value)) {
                    return candidate;
                }
            }
            throw new IllegalArgumentException("unknown NodeDiagnosticState wire value: " + value);
        }
    }

    public enum NodeDiscoveryIndexErrorCategory {
        OFFLINE("offline"),
        TIMEOUT("timeout"),
        TLS("tls"),
        SIGNATURE("signature"),
        PARTIAL_CORRUPTION("partial_corruption"),
        INCOMPATIBLE_VERSION("incompatible_version"),
        UNSUPPORTED("unsupported"),
        REJECTED("rejected"),
        INVALID_RESPONSE("invalid_response"),
        STORAGE("storage"),
        SERVER("server"),
        CANCELLED("cancelled");

        private final String wireName;

        NodeDiscoveryIndexErrorCategory(String wireName) {
            this.wireName = wireName;
        }

        public String wireName() {
            return wireName;
        }

        public static NodeDiscoveryIndexErrorCategory fromWireName(String value) {
            for (NodeDiscoveryIndexErrorCategory candidate : values()) {
                if (candidate.wireName.equals(value)) {
                    return candidate;
                }
            }
            throw new IllegalArgumentException("unknown NodeDiscoveryIndexErrorCategory wire value: " + value);
        }
    }

    public enum NodeDiscoveryIndexSource {
        NONE("none"),
        CACHE("cache"),
        NETWORK("network"),
        JOIN("join");

        private final String wireName;

        NodeDiscoveryIndexSource(String wireName) {
            this.wireName = wireName;
        }

        public String wireName() {
            return wireName;
        }

        public static NodeDiscoveryIndexSource fromWireName(String value) {
            for (NodeDiscoveryIndexSource candidate : values()) {
                if (candidate.wireName.equals(value)) {
                    return candidate;
                }
            }
            throw new IllegalArgumentException("unknown NodeDiscoveryIndexSource wire value: " + value);
        }
    }

    public enum NodeDiscoveryIndexState {
        IDLE("idle"),
        PENDING("pending"),
        READY("ready"),
        DEGRADED("degraded"),
        FAILED("failed"),
        UNSUPPORTED("unsupported");

        private final String wireName;

        NodeDiscoveryIndexState(String wireName) {
            this.wireName = wireName;
        }

        public String wireName() {
            return wireName;
        }

        public static NodeDiscoveryIndexState fromWireName(String value) {
            for (NodeDiscoveryIndexState candidate : values()) {
                if (candidate.wireName.equals(value)) {
                    return candidate;
                }
            }
            throw new IllegalArgumentException("unknown NodeDiscoveryIndexState wire value: " + value);
        }
    }

    public enum NodeEmbeddedServiceStatus {
        STARTING("starting"),
        RUNNING("running"),
        STOPPING("stopping"),
        STOPPED("stopped"),
        FAILED("failed");

        private final String wireName;

        NodeEmbeddedServiceStatus(String wireName) {
            this.wireName = wireName;
        }

        public String wireName() {
            return wireName;
        }

        public static NodeEmbeddedServiceStatus fromWireName(String value) {
            for (NodeEmbeddedServiceStatus candidate : values()) {
                if (candidate.wireName.equals(value)) {
                    return candidate;
                }
            }
            throw new IllegalArgumentException("unknown NodeEmbeddedServiceStatus wire value: " + value);
        }
    }

    public enum NodeErrorCategory {
        AUTHENTICATION("authentication"),
        AUTHORIZATION("authorization"),
        VALIDATION("validation"),
        STATE("state"),
        CAPABILITY("capability"),
        DEPENDENCY("dependency"),
        INTERNAL("internal");

        private final String wireName;

        NodeErrorCategory(String wireName) {
            this.wireName = wireName;
        }

        public String wireName() {
            return wireName;
        }

        public static NodeErrorCategory fromWireName(String value) {
            for (NodeErrorCategory candidate : values()) {
                if (candidate.wireName.equals(value)) {
                    return candidate;
                }
            }
            throw new IllegalArgumentException("unknown NodeErrorCategory wire value: " + value);
        }
    }

    public enum NodeEventReplayReason {
        CURSOR_AHEAD("cursor_ahead"),
        HISTORY_GAP("history_gap"),
        SNAPSHOT_ROLLED_BACK("snapshot_rolled_back"),
        LAGGED("lagged"),
        REVISION_GAP("revision_gap");

        private final String wireName;

        NodeEventReplayReason(String wireName) {
            this.wireName = wireName;
        }

        public String wireName() {
            return wireName;
        }

        public static NodeEventReplayReason fromWireName(String value) {
            for (NodeEventReplayReason candidate : values()) {
                if (candidate.wireName.equals(value)) {
                    return candidate;
                }
            }
            throw new IllegalArgumentException("unknown NodeEventReplayReason wire value: " + value);
        }
    }

    public enum NodeHttpErrorCode {
        SCHEMA_VERSION_MISMATCH("schema_version_mismatch"),
        INVALID_REQUEST("invalid_request"),
        PAYLOAD_TOO_LARGE("payload_too_large"),
        RATE_LIMITED("rate_limited"),
        NOT_FOUND("not_found"),
        AUTHENTICATION_REQUIRED("authentication_required"),
        PERMISSION_DENIED("permission_denied"),
        OPERATION_CONFLICT("operation_conflict"),
        OPERATION_CANCELLED("operation_cancelled"),
        OPERATION_TIMED_OUT("operation_timed_out"),
        RESYNC_REQUIRED("resync_required"),
        CAPABILITY_UNAVAILABLE("capability_unavailable"),
        DEPENDENCY_UNAVAILABLE("dependency_unavailable"),
        BACKEND_REJECTED("backend_rejected"),
        BACKEND_UNAVAILABLE("backend_unavailable"),
        CONTRACT_VIOLATION("contract_violation"),
        INTERNAL("internal");

        private final String wireName;

        NodeHttpErrorCode(String wireName) {
            this.wireName = wireName;
        }

        public String wireName() {
            return wireName;
        }

        public static NodeHttpErrorCode fromWireName(String value) {
            for (NodeHttpErrorCode candidate : values()) {
                if (candidate.wireName.equals(value)) {
                    return candidate;
                }
            }
            throw new IllegalArgumentException("unknown NodeHttpErrorCode wire value: " + value);
        }
    }

    public enum NodeInterfaceKind {
        LOOPBACK("loopback"),
        PHYSICAL("physical"),
        VIRTUAL("virtual"),
        TUN("tun"),
        UNKNOWN("unknown");

        private final String wireName;

        NodeInterfaceKind(String wireName) {
            this.wireName = wireName;
        }

        public String wireName() {
            return wireName;
        }

        public static NodeInterfaceKind fromWireName(String value) {
            for (NodeInterfaceKind candidate : values()) {
                if (candidate.wireName.equals(value)) {
                    return candidate;
                }
            }
            throw new IllegalArgumentException("unknown NodeInterfaceKind wire value: " + value);
        }
    }

    public enum NodeInterfaceOwnership {
        SYSTEM("system"),
        PRODUCT("product"),
        UNKNOWN("unknown");

        private final String wireName;

        NodeInterfaceOwnership(String wireName) {
            this.wireName = wireName;
        }

        public String wireName() {
            return wireName;
        }

        public static NodeInterfaceOwnership fromWireName(String value) {
            for (NodeInterfaceOwnership candidate : values()) {
                if (candidate.wireName.equals(value)) {
                    return candidate;
                }
            }
            throw new IllegalArgumentException("unknown NodeInterfaceOwnership wire value: " + value);
        }
    }

    public enum NodeInterfaceStatus {
        UP("up"),
        DOWN("down"),
        UNKNOWN("unknown");

        private final String wireName;

        NodeInterfaceStatus(String wireName) {
            this.wireName = wireName;
        }

        public String wireName() {
            return wireName;
        }

        public static NodeInterfaceStatus fromWireName(String value) {
            for (NodeInterfaceStatus candidate : values()) {
                if (candidate.wireName.equals(value)) {
                    return candidate;
                }
            }
            throw new IllegalArgumentException("unknown NodeInterfaceStatus wire value: " + value);
        }
    }

    public enum NodeLogLevel {
        TRACE("trace"),
        DEBUG("debug"),
        INFO("info"),
        WARN("warn"),
        ERROR("error");

        private final String wireName;

        NodeLogLevel(String wireName) {
            this.wireName = wireName;
        }

        public String wireName() {
            return wireName;
        }

        public static NodeLogLevel fromWireName(String value) {
            for (NodeLogLevel candidate : values()) {
                if (candidate.wireName.equals(value)) {
                    return candidate;
                }
            }
            throw new IllegalArgumentException("unknown NodeLogLevel wire value: " + value);
        }
    }

    public enum NodeNetworkCreationNextStep {
        INVITE_MEMBERS("invite_members"),
        CONFIGURE_POLICY("configure_policy"),
        CONTRIBUTE_CAPABILITIES("contribute_capabilities");

        private final String wireName;

        NodeNetworkCreationNextStep(String wireName) {
            this.wireName = wireName;
        }

        public String wireName() {
            return wireName;
        }

        public static NodeNetworkCreationNextStep fromWireName(String value) {
            for (NodeNetworkCreationNextStep candidate : values()) {
                if (candidate.wireName.equals(value)) {
                    return candidate;
                }
            }
            throw new IllegalArgumentException("unknown NodeNetworkCreationNextStep wire value: " + value);
        }
    }

    public enum NodeNetworkRole {
        MEMBER("member"),
        DISCOVERY("discovery"),
        RELAY("relay"),
        EXIT_NODE("exit_node");

        private final String wireName;

        NodeNetworkRole(String wireName) {
            this.wireName = wireName;
        }

        public String wireName() {
            return wireName;
        }

        public static NodeNetworkRole fromWireName(String value) {
            for (NodeNetworkRole candidate : values()) {
                if (candidate.wireName.equals(value)) {
                    return candidate;
                }
            }
            throw new IllegalArgumentException("unknown NodeNetworkRole wire value: " + value);
        }
    }

    public enum NodeNetworkStatus {
        OFFLINE("offline"),
        CONNECTING("connecting"),
        ONLINE("online"),
        DEGRADED("degraded"),
        STOPPING("stopping"),
        FAILED("failed");

        private final String wireName;

        NodeNetworkStatus(String wireName) {
            this.wireName = wireName;
        }

        public String wireName() {
            return wireName;
        }

        public static NodeNetworkStatus fromWireName(String value) {
            for (NodeNetworkStatus candidate : values()) {
                if (candidate.wireName.equals(value)) {
                    return candidate;
                }
            }
            throw new IllegalArgumentException("unknown NodeNetworkStatus wire value: " + value);
        }
    }

    public enum NodePermission {
        VIEW_STATE("view_state"),
        MANAGE_NETWORKS("manage_networks"),
        CHANGE_ROUTING("change_routing"),
        MANAGE_ROLES("manage_roles"),
        PROBE_PEERS("probe_peers"),
        STOP_NODE("stop_node"),
        VIEW_LOGS("view_logs");

        private final String wireName;

        NodePermission(String wireName) {
            this.wireName = wireName;
        }

        public String wireName() {
            return wireName;
        }

        public static NodePermission fromWireName(String value) {
            for (NodePermission candidate : values()) {
                if (candidate.wireName.equals(value)) {
                    return candidate;
                }
            }
            throw new IllegalArgumentException("unknown NodePermission wire value: " + value);
        }
    }

    public enum NodePhase {
        STOPPED("stopped"),
        STARTING("starting"),
        RUNNING("running"),
        STOPPING("stopping"),
        FAILED("failed");

        private final String wireName;

        NodePhase(String wireName) {
            this.wireName = wireName;
        }

        public String wireName() {
            return wireName;
        }

        public static NodePhase fromWireName(String value) {
            for (NodePhase candidate : values()) {
                if (candidate.wireName.equals(value)) {
                    return candidate;
                }
            }
            throw new IllegalArgumentException("unknown NodePhase wire value: " + value);
        }
    }

    public enum NodeProblemCode {
        AUTHENTICATION_REQUIRED("authentication_required"),
        INVALID_REQUEST("invalid_request"),
        NOT_FOUND("not_found"),
        CONFLICT("conflict"),
        UNAVAILABLE("unavailable"),
        PERMISSION_DENIED("permission_denied"),
        CANCELLED("cancelled"),
        TIMED_OUT("timed_out"),
        INTERNAL("internal"),
        UNSUPPORTED("unsupported");

        private final String wireName;

        NodeProblemCode(String wireName) {
            this.wireName = wireName;
        }

        public String wireName() {
            return wireName;
        }

        public static NodeProblemCode fromWireName(String value) {
            for (NodeProblemCode candidate : values()) {
                if (candidate.wireName.equals(value)) {
                    return candidate;
                }
            }
            throw new IllegalArgumentException("unknown NodeProblemCode wire value: " + value);
        }
    }

    public enum NodeRoutingMode {
        MESH_ONLY("mesh_only"),
        BYPASS_MAINLAND("bypass_mainland"),
        GLOBAL_PROXY("global_proxy");

        private final String wireName;

        NodeRoutingMode(String wireName) {
            this.wireName = wireName;
        }

        public String wireName() {
            return wireName;
        }

        public static NodeRoutingMode fromWireName(String value) {
            for (NodeRoutingMode candidate : values()) {
                if (candidate.wireName.equals(value)) {
                    return candidate;
                }
            }
            throw new IllegalArgumentException("unknown NodeRoutingMode wire value: " + value);
        }
    }

    public enum NodeRoutingSummaryStatus {
        CURRENT("current"),
        STALE("stale"),
        INVALID("invalid"),
        APPLY_FAILED("apply_failed");

        private final String wireName;

        NodeRoutingSummaryStatus(String wireName) {
            this.wireName = wireName;
        }

        public String wireName() {
            return wireName;
        }

        public static NodeRoutingSummaryStatus fromWireName(String value) {
            for (NodeRoutingSummaryStatus candidate : values()) {
                if (candidate.wireName.equals(value)) {
                    return candidate;
                }
            }
            throw new IllegalArgumentException("unknown NodeRoutingSummaryStatus wire value: " + value);
        }
    }

    public enum NodeUiActionBlockReason {
        NODE_NOT_RUNNING("node_not_running"),
        NO_ONLINE_NETWORK("no_online_network"),
        NO_NETWORK_SELECTED("no_network_selected"),
        PERMISSION_DENIED("permission_denied"),
        PLATFORM_UNSUPPORTED("platform_unsupported"),
        EXIT_NODE_UNSUPPORTED("exit_node_unsupported"),
        TUN_PLATFORM_UNSUPPORTED("tun_platform_unsupported"),
        TUN_PERMISSION_DENIED("tun_permission_denied"),
        TUN_DEVICE_UNAVAILABLE("tun_device_unavailable"),
        TUN_TOOL_UNAVAILABLE("tun_tool_unavailable"),
        TUN_UNAVAILABLE("tun_unavailable"),
        BACKEND_UNAVAILABLE("backend_unavailable");

        private final String wireName;

        NodeUiActionBlockReason(String wireName) {
            this.wireName = wireName;
        }

        public String wireName() {
            return wireName;
        }

        public static NodeUiActionBlockReason fromWireName(String value) {
            for (NodeUiActionBlockReason candidate : values()) {
                if (candidate.wireName.equals(value)) {
                    return candidate;
                }
            }
            throw new IllegalArgumentException("unknown NodeUiActionBlockReason wire value: " + value);
        }
    }

    public enum NodeUiAuditPolicy {
        NONE("none"),
        RECORD_OUTCOME("record_outcome");

        private final String wireName;

        NodeUiAuditPolicy(String wireName) {
            this.wireName = wireName;
        }

        public String wireName() {
            return wireName;
        }

        public static NodeUiAuditPolicy fromWireName(String value) {
            for (NodeUiAuditPolicy candidate : values()) {
                if (candidate.wireName.equals(value)) {
                    return candidate;
                }
            }
            throw new IllegalArgumentException("unknown NodeUiAuditPolicy wire value: " + value);
        }
    }

    public enum NodeUiAvailability {
        ALWAYS("always"),
        NODE_RUNNING("node_running"),
        NETWORK_ONLINE("network_online"),
        NETWORK_SELECTED("network_selected");

        private final String wireName;

        NodeUiAvailability(String wireName) {
            this.wireName = wireName;
        }

        public String wireName() {
            return wireName;
        }

        public static NodeUiAvailability fromWireName(String value) {
            for (NodeUiAvailability candidate : values()) {
                if (candidate.wireName.equals(value)) {
                    return candidate;
                }
            }
            throw new IllegalArgumentException("unknown NodeUiAvailability wire value: " + value);
        }
    }

    public enum NodeUiBackendStatus {
        IMPLEMENTED("implemented"),
        UNAVAILABLE("unavailable");

        private final String wireName;

        NodeUiBackendStatus(String wireName) {
            this.wireName = wireName;
        }

        public String wireName() {
            return wireName;
        }

        public static NodeUiBackendStatus fromWireName(String value) {
            for (NodeUiBackendStatus candidate : values()) {
                if (candidate.wireName.equals(value)) {
                    return candidate;
                }
            }
            throw new IllegalArgumentException("unknown NodeUiBackendStatus wire value: " + value);
        }
    }

    public enum NodeUiInputKind {
        BOOLEAN("boolean"),
        CLIENT_ID("client_id"),
        CONFIGURATION_JSON("configuration_json"),
        POLICY_JSON("policy_json"),
        POLICY_SNAPSHOT_JSON("policy_snapshot_json"),
        TRAFFIC_POLICY_JSON("traffic_policy_json"),
        TRAFFIC_POLICY_TEST_JSON("traffic_policy_test_json"),
        POLICY_NETWORK_IDS("policy_network_ids"),
        DISPLAY_NAME("display_name"),
        EXTERNAL_HOST("external_host"),
        FINGERPRINT("fingerprint"),
        NETWORK_MODE("network_mode"),
        NETWORK_NAME("network_name"),
        NETWORK_SELECTION("network_selection"),
        PEER_TARGET("peer_target"),
        PURPOSE("purpose"),
        ICON("icon"),
        LANGUAGE("language"),
        REGION("region"),
        TAGS("tags"),
        CAPABILITY_DECLARATIONS("capability_declarations"),
        VISIBILITY("visibility"),
        JOIN_MODE("join_mode"),
        EXIT_CONTRIBUTION("exit_contribution"),
        EXIT_CONTRIBUTION_SCOPE("exit_contribution_scope"),
        JOIN_REQUEST_ID("join_request_id"),
        JOIN_REQUEST_NOTE("join_request_note"),
        JOIN_REQUEST_REASON("join_request_reason"),
        JOIN_REQUEST_REVIEW_ACTION("join_request_review_action"),
        JOIN_REQUEST_REVIEWS("join_request_reviews"),
        NODE_ID("node_id"),
        MEMBER_LIFECYCLE_ACTION("member_lifecycle_action"),
        MEMBER_LIFECYCLE_REASON("member_lifecycle_reason"),
        INVITATION_CODE("invitation_code"),
        COORDINATOR_URL("coordinator_url"),
        PORT("port"),
        REVISION("revision"),
        ROLE("role"),
        ROUTING_MODE("routing_mode"),
        URL_OR_TICKET("url_or_ticket"),
        DISCOVERY_INDEX_URL("discovery_index_url"),
        TIMEOUT_MS("timeout_ms");

        private final String wireName;

        NodeUiInputKind(String wireName) {
            this.wireName = wireName;
        }

        public String wireName() {
            return wireName;
        }

        public static NodeUiInputKind fromWireName(String value) {
            for (NodeUiInputKind candidate : values()) {
                if (candidate.wireName.equals(value)) {
                    return candidate;
                }
            }
            throw new IllegalArgumentException("unknown NodeUiInputKind wire value: " + value);
        }
    }

    public enum NodeUiPageState {
        INITIAL_LOADING("initial_loading"),
        ERROR("error"),
        MAINTENANCE("maintenance"),
        PERMISSION_DENIED("permission_denied"),
        UNSUPPORTED("unsupported"),
        OFFLINE("offline"),
        CONTROL_UNAVAILABLE("control_unavailable"),
        NODE_FAILED("node_failed"),
        DEGRADED("degraded"),
        STALE("stale"),
        REFRESHING("refreshing"),
        EMPTY("empty"),
        READY("ready");

        private final String wireName;

        NodeUiPageState(String wireName) {
            this.wireName = wireName;
        }

        public String wireName() {
            return wireName;
        }

        public static NodeUiPageState fromWireName(String value) {
            for (NodeUiPageState candidate : values()) {
                if (candidate.wireName.equals(value)) {
                    return candidate;
                }
            }
            throw new IllegalArgumentException("unknown NodeUiPageState wire value: " + value);
        }
    }

    public enum NodeUiValidation {
        NONE("none"),
        NON_EMPTY("non_empty"),
        NETWORK_NAME("network_name"),
        PORT("port"),
        DISTINCT_PORTS("distinct_ports"),
        NETWORK_MODE("network_mode"),
        NETWORK_SELECTION("network_selection"),
        ROLE("role"),
        ROUTING_MODE("routing_mode"),
        POSITIVE_TIMEOUT("positive_timeout"),
        REVISION("revision"),
        CONFIGURATION_JSON("configuration_json"),
        POLICY_JSON("policy_json"),
        POLICY_SNAPSHOT_JSON("policy_snapshot_json"),
        TRAFFIC_POLICY_JSON("traffic_policy_json"),
        TRAFFIC_POLICY_TEST_JSON("traffic_policy_test_json"),
        POLICY_NETWORK_IDS("policy_network_ids"),
        DISPLAY_NAME("display_name"),
        FINGERPRINT("fingerprint"),
        NETWORK_PURPOSE("network_purpose"),
        NETWORK_ICON("network_icon"),
        NETWORK_LANGUAGE("network_language"),
        REGION("region"),
        TAGS("tags"),
        CAPABILITY_DECLARATIONS("capability_declarations"),
        VISIBILITY("visibility"),
        JOIN_MODE("join_mode"),
        EXIT_CONTRIBUTION("exit_contribution"),
        EXIT_CONTRIBUTION_SCOPE("exit_contribution_scope"),
        JOIN_REQUEST_ID("join_request_id"),
        JOIN_REQUEST_NOTE("join_request_note"),
        JOIN_REQUEST_REASON("join_request_reason"),
        JOIN_REQUEST_REVIEW_ACTION("join_request_review_action"),
        JOIN_REQUEST_REVIEWS("join_request_reviews"),
        NODE_ID("node_id"),
        MEMBER_LIFECYCLE_ACTION("member_lifecycle_action"),
        MEMBER_LIFECYCLE_REASON("member_lifecycle_reason"),
        INVITATION_CODE("invitation_code"),
        COORDINATOR_URL("coordinator_url");

        private final String wireName;

        NodeUiValidation(String wireName) {
            this.wireName = wireName;
        }

        public String wireName() {
            return wireName;
        }

        public static NodeUiValidation fromWireName(String value) {
            for (NodeUiValidation candidate : values()) {
                if (candidate.wireName.equals(value)) {
                    return candidate;
                }
            }
            throw new IllegalArgumentException("unknown NodeUiValidation wire value: " + value);
        }
    }

    public enum NodeWsResyncReason {
        LAGGED("lagged"),
        REVISION_GAP("revision_gap");

        private final String wireName;

        NodeWsResyncReason(String wireName) {
            this.wireName = wireName;
        }

        public String wireName() {
            return wireName;
        }

        public static NodeWsResyncReason fromWireName(String value) {
            for (NodeWsResyncReason candidate : values()) {
                if (candidate.wireName.equals(value)) {
                    return candidate;
                }
            }
            throw new IllegalArgumentException("unknown NodeWsResyncReason wire value: " + value);
        }
    }

    public enum ObjectKind {
        NODE("node"),
        NETWORK("network"),
        MEMBERSHIP("membership"),
        GROUP("group"),
        SECURITY_POLICY("security_policy"),
        TRAFFIC_POLICY("traffic_policy"),
        CAPABILITY("capability"),
        EXIT_GRANT("exit_grant"),
        COORDINATOR_TERM("coordinator_term"),
        FILE_SHARE("file_share"),
        OPERATION("operation"),
        AUDIT("audit");

        private final String wireName;

        ObjectKind(String wireName) {
            this.wireName = wireName;
        }

        public String wireName() {
            return wireName;
        }

        public static ObjectKind fromWireName(String value) {
            for (ObjectKind candidate : values()) {
                if (candidate.wireName.equals(value)) {
                    return candidate;
                }
            }
            throw new IllegalArgumentException("unknown ObjectKind wire value: " + value);
        }
    }

    public enum OperationKind {
        NODE_START("node_start"),
        NODE_STOP("node_stop"),
        NODE_RESTART("node_restart"),
        NODE_CLEANUP("node_cleanup"),
        DIAGNOSTICS("diagnostics"),
        NETWORK_CREATE("network_create"),
        NETWORK_PROFILE_UPDATE("network_profile_update"),
        DISCOVERY_INDEX_UPDATE("discovery_index_update"),
        NETWORK_APPLY("network_apply"),
        TUN_REBUILD("tun_rebuild"),
        NETWORK_LEAVE("network_leave"),
        MEMBERSHIP_APPROVE("membership_approve"),
        MEMBERSHIP_REJECT("membership_reject"),
        MEMBERSHIP_REMOVE("membership_remove"),
        MEMBERSHIP_INVITE("membership_invite"),
        MEMBERSHIP_SUSPEND("membership_suspend"),
        MEMBERSHIP_RESUME("membership_resume"),
        MEMBERSHIP_EXIT("membership_exit"),
        MEMBERSHIP_UNDO_REMOVE("membership_undo_remove"),
        CONFIGURATION_HISTORY("configuration_history"),
        POLICY_TEST("policy_test"),
        POLICY_APPLY("policy_apply"),
        POLICY_ROLLBACK("policy_rollback"),
        TRAFFIC_RULES_APPLY("traffic_rules_apply"),
        NETWORK_ROLE_UPDATE("network_role_update"),
        IDENTITY_UPDATE("identity_update"),
        CLIENT_BLACKLIST_UPDATE("client_blacklist_update"),
        NETWORK_ALLOCATION_UPDATE("network_allocation_update"),
        CAPABILITY_ENABLE("capability_enable"),
        CAPABILITY_DISABLE("capability_disable"),
        EXIT_SELECT("exit_select"),
        EXIT_REVOKE("exit_revoke"),
        COORDINATOR_CANDIDATES_UPDATE("coordinator_candidates_update"),
        COORDINATOR_RECOVER("coordinator_recover"),
        FILE_SHARE_UPDATE("file_share_update"),
        FILE_UPLOAD_START("file_upload_start"),
        FILE_DOWNLOAD_START("file_download_start"),
        OWNER_PASSKEY_REGISTER("owner_passkey_register"),
        OWNER_PASSKEY_REVOKE("owner_passkey_revoke"),
        CERTIFICATE_RELOAD("certificate_reload");

        private final String wireName;

        OperationKind(String wireName) {
            this.wireName = wireName;
        }

        public String wireName() {
            return wireName;
        }

        public static OperationKind fromWireName(String value) {
            for (OperationKind candidate : values()) {
                if (candidate.wireName.equals(value)) {
                    return candidate;
                }
            }
            throw new IllegalArgumentException("unknown OperationKind wire value: " + value);
        }
    }

    public enum OperationState {
        QUEUED("queued"),
        RUNNING("running"),
        WAITING_FOR_REAUTH("waiting_for_reauth"),
        SUCCEEDED("succeeded"),
        FAILED("failed"),
        CANCELLED("cancelled"),
        TIMED_OUT("timed_out");

        private final String wireName;

        OperationState(String wireName) {
            this.wireName = wireName;
        }

        public String wireName() {
            return wireName;
        }

        public static OperationState fromWireName(String value) {
            for (OperationState candidate : values()) {
                if (candidate.wireName.equals(value)) {
                    return candidate;
                }
            }
            throw new IllegalArgumentException("unknown OperationState wire value: " + value);
        }
    }

    public enum PermissionEffect {
        ALLOW("allow"),
        DENY("deny");

        private final String wireName;

        PermissionEffect(String wireName) {
            this.wireName = wireName;
        }

        public String wireName() {
            return wireName;
        }

        public static PermissionEffect fromWireName(String value) {
            for (PermissionEffect candidate : values()) {
                if (candidate.wireName.equals(value)) {
                    return candidate;
                }
            }
            throw new IllegalArgumentException("unknown PermissionEffect wire value: " + value);
        }
    }

    public enum PermissionIntent {
        READ_ONLY("read_only"),
        MUTATING("mutating");

        private final String wireName;

        PermissionIntent(String wireName) {
            this.wireName = wireName;
        }

        public String wireName() {
            return wireName;
        }

        public static PermissionIntent fromWireName(String value) {
            for (PermissionIntent candidate : values()) {
                if (candidate.wireName.equals(value)) {
                    return candidate;
                }
            }
            throw new IllegalArgumentException("unknown PermissionIntent wire value: " + value);
        }
    }

    public enum PolicyAction {
        READ("read"),
        CREATE("create"),
        UPDATE("update"),
        DELETE("delete"),
        APPROVE("approve"),
        REJECT("reject"),
        REMOVE("remove"),
        ENABLE("enable"),
        DISABLE("disable"),
        USE("use"),
        RECOVER("recover"),
        EXPORT("export"),
        TEST("test");

        private final String wireName;

        PolicyAction(String wireName) {
            this.wireName = wireName;
        }

        public String wireName() {
            return wireName;
        }

        public static PolicyAction fromWireName(String value) {
            for (PolicyAction candidate : values()) {
                if (candidate.wireName.equals(value)) {
                    return candidate;
                }
            }
            throw new IllegalArgumentException("unknown PolicyAction wire value: " + value);
        }
    }

    public enum PolicyChangeAction {
        APPLY("apply"),
        ROLLBACK("rollback");

        private final String wireName;

        PolicyChangeAction(String wireName) {
            this.wireName = wireName;
        }

        public String wireName() {
            return wireName;
        }

        public static PolicyChangeAction fromWireName(String value) {
            for (PolicyChangeAction candidate : values()) {
                if (candidate.wireName.equals(value)) {
                    return candidate;
                }
            }
            throw new IllegalArgumentException("unknown PolicyChangeAction wire value: " + value);
        }
    }

    public enum PolicyChangeKind {
        LOCAL_POLICY_ADDED("local_policy_added"),
        LOCAL_POLICY_UPDATED("local_policy_updated"),
        LOCAL_POLICY_REMOVED("local_policy_removed"),
        NETWORK_ACL_ADDED("network_acl_added"),
        NETWORK_ACL_UPDATED("network_acl_updated"),
        NETWORK_ACL_REMOVED("network_acl_removed"),
        PERMISSION_RULE_ADDED("permission_rule_added"),
        PERMISSION_RULE_UPDATED("permission_rule_updated"),
        PERMISSION_RULE_REMOVED("permission_rule_removed"),
        GRANT_ADDED("grant_added"),
        GRANT_UPDATED("grant_updated"),
        GRANT_REMOVED("grant_removed"),
        VETO_ADDED("veto_added"),
        VETO_UPDATED("veto_updated"),
        VETO_REMOVED("veto_removed"),
        NETWORK_SCOPE_CHANGED("network_scope_changed"),
        VALIDITY_CHANGED("validity_changed");

        private final String wireName;

        PolicyChangeKind(String wireName) {
            this.wireName = wireName;
        }

        public String wireName() {
            return wireName;
        }

        public static PolicyChangeKind fromWireName(String value) {
            for (PolicyChangeKind candidate : values()) {
                if (candidate.wireName.equals(value)) {
                    return candidate;
                }
            }
            throw new IllegalArgumentException("unknown PolicyChangeKind wire value: " + value);
        }
    }

    public enum PolicyObjectKind {
        LOCAL_POLICY("local_policy"),
        NETWORK_ACL("network_acl"),
        PERMISSION_RULE("permission_rule"),
        NETWORK_ACL_GRANT("network_acl_grant"),
        CONTRIBUTION_VETO("contribution_veto"),
        NETWORK_SCOPE("network_scope");

        private final String wireName;

        PolicyObjectKind(String wireName) {
            this.wireName = wireName;
        }

        public String wireName() {
            return wireName;
        }

        public static PolicyObjectKind fromWireName(String value) {
            for (PolicyObjectKind candidate : values()) {
                if (candidate.wireName.equals(value)) {
                    return candidate;
                }
            }
            throw new IllegalArgumentException("unknown PolicyObjectKind wire value: " + value);
        }
    }

    public enum PolicyPrecedence {
        CROSS_NETWORK_DENIED("cross_network_denied"),
        INVALID_OR_EXPIRED_POLICY("invalid_or_expired_policy"),
        NETWORK_SCOPE_RESTRICTED("network_scope_restricted"),
        LOCAL_CONTRIBUTION_VETO("local_contribution_veto"),
        EXPLICIT_DENY("explicit_deny"),
        TEMPORARY_COORDINATOR_READ_ONLY("temporary_coordinator_read_only"),
        ROLE_CEILING_EXCEEDED("role_ceiling_exceeded"),
        AUTHORIZATION_REVOKED("authorization_revoked"),
        AUTHORIZATION_EXPIRED("authorization_expired"),
        RECENT_REAUTHENTICATION_REQUIRED("recent_reauthentication_required"),
        DEFAULT_DENIED("default_denied"),
        ALLOW("allow");

        private final String wireName;

        PolicyPrecedence(String wireName) {
            this.wireName = wireName;
        }

        public String wireName() {
            return wireName;
        }

        public static PolicyPrecedence fromWireName(String value) {
            for (PolicyPrecedence candidate : values()) {
                if (candidate.wireName.equals(value)) {
                    return candidate;
                }
            }
            throw new IllegalArgumentException("unknown PolicyPrecedence wire value: " + value);
        }
    }

    public enum PolicyRiskLevel {
        NONE("none"),
        ELEVATED("elevated"),
        HIGH("high");

        private final String wireName;

        PolicyRiskLevel(String wireName) {
            this.wireName = wireName;
        }

        public String wireName() {
            return wireName;
        }

        public static PolicyRiskLevel fromWireName(String value) {
            for (PolicyRiskLevel candidate : values()) {
                if (candidate.wireName.equals(value)) {
                    return candidate;
                }
            }
            throw new IllegalArgumentException("unknown PolicyRiskLevel wire value: " + value);
        }
    }

    public enum PolicyRiskReason {
        ALLOW_RULE_ADDED("allow_rule_added"),
        DENY_RULE_REMOVED("deny_rule_removed"),
        PERMISSION_RULE_EXPANDED("permission_rule_expanded"),
        NETWORK_ACL_GRANT_ADDED("network_acl_grant_added"),
        NETWORK_ACL_GRANT_EXPANDED("network_acl_grant_expanded"),
        GRANT_REAUTHENTICATION_REMOVED("grant_reauthentication_removed"),
        GRANT_VALIDITY_EXTENDED("grant_validity_extended"),
        LOCAL_POLICY_REMOVED("local_policy_removed"),
        NETWORK_ACL_REMOVED("network_acl_removed"),
        NETWORK_SCOPE_EXPANDED("network_scope_expanded"),
        CONTRIBUTION_VETO_REMOVED("contribution_veto_removed"),
        POLICY_VALIDITY_EXTENDED("policy_validity_extended");

        private final String wireName;

        PolicyRiskReason(String wireName) {
            this.wireName = wireName;
        }

        public String wireName() {
            return wireName;
        }

        public static PolicyRiskReason fromWireName(String value) {
            for (PolicyRiskReason candidate : values()) {
                if (candidate.wireName.equals(value)) {
                    return candidate;
                }
            }
            throw new IllegalArgumentException("unknown PolicyRiskReason wire value: " + value);
        }
    }

    public enum PolicyRuleSource {
        ROLE_BASELINE("role_baseline"),
        LOCAL_POLICY_RULE("local_policy_rule"),
        LOCAL_CONTRIBUTION_VETO("local_contribution_veto"),
        LOCAL_NETWORK_SCOPE("local_network_scope"),
        NETWORK_ACL_RULE("network_acl_rule"),
        NETWORK_ACL_GRANT("network_acl_grant"),
        REQUEST("request"),
        ACCESS_CONTEXT("access_context");

        private final String wireName;

        PolicyRuleSource(String wireName) {
            this.wireName = wireName;
        }

        public String wireName() {
            return wireName;
        }

        public static PolicyRuleSource fromWireName(String value) {
            for (PolicyRuleSource candidate : values()) {
                if (candidate.wireName.equals(value)) {
                    return candidate;
                }
            }
            throw new IllegalArgumentException("unknown PolicyRuleSource wire value: " + value);
        }
    }

    public enum PolicyValidationSource {
        LOCAL_POLICY("local_policy"),
        NETWORK_ACL("network_acl"),
        TEST_INPUT("test_input");

        private final String wireName;

        PolicyValidationSource(String wireName) {
            this.wireName = wireName;
        }

        public String wireName() {
            return wireName;
        }

        public static PolicyValidationSource fromWireName(String value) {
            for (PolicyValidationSource candidate : values()) {
                if (candidate.wireName.equals(value)) {
                    return candidate;
                }
            }
            throw new IllegalArgumentException("unknown PolicyValidationSource wire value: " + value);
        }
    }

    public enum PrivacyField {
        MEMBER_NODE_ID("member_node_id"),
        MEMBER_DISPLAY_NAME("member_display_name"),
        MEMBER_IDENTITY_FINGERPRINT("member_identity_fingerprint"),
        MEMBER_ROLE("member_role"),
        MEMBER_GROUPS("member_groups"),
        MEMBER_CAPABILITIES("member_capabilities"),
        MEMBER_STATUS("member_status"),
        MEMBER_LIFECYCLE_STATE("member_lifecycle_state"),
        MEMBER_REVISION("member_revision"),
        MEMBER_ACCESS_EPOCH("member_access_epoch"),
        MEMBER_AVAILABLE_ACTIONS("member_available_actions"),
        MEMBER_PRESENCE("member_presence"),
        MEMBER_JOINED_AT("member_joined_at"),
        NODE_DISPLAY_NAME("node_display_name"),
        NODE_PUBLIC_KEY("node_public_key"),
        NODE_PLATFORM_AND_VERSION("node_platform_and_version"),
        NODE_ONLINE_STATE("node_online_state"),
        NODE_ENDPOINTS("node_endpoints"),
        NODE_REAL_NETWORK_ADDRESSES("node_real_network_addresses"),
        NODE_TRAFFIC_STATISTICS("node_traffic_statistics"),
        NODE_LOCAL_PROCESSES("node_local_processes"),
        NODE_LOCAL_PATHS("node_local_paths"),
        NODE_DETAILED_LOGS("node_detailed_logs"),
        TOPOLOGY_NODE_ID("topology_node_id"),
        TOPOLOGY_DISPLAY_NAME("topology_display_name"),
        TOPOLOGY_ROLE("topology_role"),
        TOPOLOGY_STATUS("topology_status"),
        TOPOLOGY_PRESENCE("topology_presence"),
        TOPOLOGY_HEALTH("topology_health"),
        TOPOLOGY_CAPABILITIES("topology_capabilities"),
        TOPOLOGY_NETWORK_MEMBERSHIP("topology_network_membership"),
        TOPOLOGY_CONNECTIONS("topology_connections"),
        ACTIVITY_ACTOR("activity_actor"),
        ACTIVITY_ACTION("activity_action"),
        ACTIVITY_RESOURCE("activity_resource"),
        ACTIVITY_OUTCOME("activity_outcome"),
        ACTIVITY_OCCURRED_AT("activity_occurred_at"),
        ACTIVITY_SUMMARY("activity_summary"),
        ACTIVITY_DETAILS("activity_details"),
        FILE_ID("file_id"),
        FILE_NETWORK_ID("file_network_id"),
        FILE_DISPLAY_NAME("file_display_name"),
        FILE_OWNER("file_owner"),
        FILE_MODE("file_mode"),
        FILE_REVISION("file_revision"),
        FILE_NETWORK_ACL("file_network_acl"),
        FILE_LOCAL_ACL("file_local_acl"),
        FILE_QUOTA("file_quota"),
        FILE_PATH_POLICY("file_path_policy"),
        FILE_TIMESTAMPS("file_timestamps"),
        FILE_AUDIT_SETTINGS("file_audit_settings");

        private final String wireName;

        PrivacyField(String wireName) {
            this.wireName = wireName;
        }

        public String wireName() {
            return wireName;
        }

        public static PrivacyField fromWireName(String value) {
            for (PrivacyField candidate : values()) {
                if (candidate.wireName.equals(value)) {
                    return candidate;
                }
            }
            throw new IllegalArgumentException("unknown PrivacyField wire value: " + value);
        }
    }

    public enum ResourceKind {
        NETWORK("network"),
        OWNERSHIP("ownership"),
        MEMBERSHIP("membership"),
        GROUP("group"),
        SECURITY_POLICY("security_policy"),
        TRAFFIC_POLICY("traffic_policy"),
        CAPABILITY("capability"),
        EXIT_GRANT("exit_grant"),
        FILE_SHARE("file_share"),
        AUDIT("audit"),
        COORDINATOR_TERM("coordinator_term"),
        KEYS("keys");

        private final String wireName;

        ResourceKind(String wireName) {
            this.wireName = wireName;
        }

        public String wireName() {
            return wireName;
        }

        public static ResourceKind fromWireName(String value) {
            for (ResourceKind candidate : values()) {
                if (candidate.wireName.equals(value)) {
                    return candidate;
                }
            }
            throw new IllegalArgumentException("unknown ResourceKind wire value: " + value);
        }
    }

    public enum TopologyConnectionState {
        ACTIVE("active"),
        IDLE("idle"),
        STALE("stale");

        private final String wireName;

        TopologyConnectionState(String wireName) {
            this.wireName = wireName;
        }

        public String wireName() {
            return wireName;
        }

        public static TopologyConnectionState fromWireName(String value) {
            for (TopologyConnectionState candidate : values()) {
                if (candidate.wireName.equals(value)) {
                    return candidate;
                }
            }
            throw new IllegalArgumentException("unknown TopologyConnectionState wire value: " + value);
        }
    }

    public enum TopologyNodeHealth {
        HEALTHY("healthy"),
        UNKNOWN("unknown"),
        DEGRADED("degraded"),
        UNREACHABLE("unreachable");

        private final String wireName;

        TopologyNodeHealth(String wireName) {
            this.wireName = wireName;
        }

        public String wireName() {
            return wireName;
        }

        public static TopologyNodeHealth fromWireName(String value) {
            for (TopologyNodeHealth candidate : values()) {
                if (candidate.wireName.equals(value)) {
                    return candidate;
                }
            }
            throw new IllegalArgumentException("unknown TopologyNodeHealth wire value: " + value);
        }
    }

    public enum TrafficDecisionReason {
        FIRST_MATCH("first_match"),
        DEFAULT("default");

        private final String wireName;

        TrafficDecisionReason(String wireName) {
            this.wireName = wireName;
        }

        public String wireName() {
            return wireName;
        }

        public static TrafficDecisionReason fromWireName(String value) {
            for (TrafficDecisionReason candidate : values()) {
                if (candidate.wireName.equals(value)) {
                    return candidate;
                }
            }
            throw new IllegalArgumentException("unknown TrafficDecisionReason wire value: " + value);
        }
    }

    public enum TrafficDefaultConfirmedBy {
        OWNER("owner"),
        ADMINISTRATOR("administrator");

        private final String wireName;

        TrafficDefaultConfirmedBy(String wireName) {
            this.wireName = wireName;
        }

        public String wireName() {
            return wireName;
        }

        public static TrafficDefaultConfirmedBy fromWireName(String value) {
            for (TrafficDefaultConfirmedBy candidate : values()) {
                if (candidate.wireName.equals(value)) {
                    return candidate;
                }
            }
            throw new IllegalArgumentException("unknown TrafficDefaultConfirmedBy wire value: " + value);
        }
    }

    public enum TrafficFailureDecisionReason {
        DEFAULT_FAIL_CLOSED("default_fail_closed"),
        NOT_ALLOWED_BY_POLICY("not_allowed_by_policy"),
        EXPLICIT_DIRECT_FALLBACK("explicit_direct_fallback"),
        UNSAFE_FAILURE_CLASS("unsafe_failure_class"),
        REJECT_RULE("reject_rule"),
        FIXED_EXIT("fixed_exit"),
        FIXED_NETWORK("fixed_network"),
        DIRECT_ACTION_HAS_NO_ALTERNATE("direct_action_has_no_alternate");

        private final String wireName;

        TrafficFailureDecisionReason(String wireName) {
            this.wireName = wireName;
        }

        public String wireName() {
            return wireName;
        }

        public static TrafficFailureDecisionReason fromWireName(String value) {
            for (TrafficFailureDecisionReason candidate : values()) {
                if (candidate.wireName.equals(value)) {
                    return candidate;
                }
            }
            throw new IllegalArgumentException("unknown TrafficFailureDecisionReason wire value: " + value);
        }
    }

    public enum TrafficFailureKind {
        UNKNOWN_APPLICATION("unknown_application"),
        DNS_RESOLUTION_TIMEOUT("dns_resolution_timeout"),
        DNS_POLLUTION_DETECTED("dns_pollution_detected"),
        EXIT_UNAVAILABLE("exit_unavailable"),
        NETWORK_SELECTION_UNAVAILABLE("network_selection_unavailable"),
        CONNECTION_TIMEOUT("connection_timeout"),
        PARTIAL_CONNECTION_FAILURE("partial_connection_failure");

        private final String wireName;

        TrafficFailureKind(String wireName) {
            this.wireName = wireName;
        }

        public String wireName() {
            return wireName;
        }

        public static TrafficFailureKind fromWireName(String value) {
            for (TrafficFailureKind candidate : values()) {
                if (candidate.wireName.equals(value)) {
                    return candidate;
                }
            }
            throw new IllegalArgumentException("unknown TrafficFailureKind wire value: " + value);
        }
    }

    public enum TrafficFailureOutcome {
        FAIL_CLOSED("fail_closed"),
        DIRECT_FALLBACK("direct_fallback");

        private final String wireName;

        TrafficFailureOutcome(String wireName) {
            this.wireName = wireName;
        }

        public String wireName() {
            return wireName;
        }

        public static TrafficFailureOutcome fromWireName(String value) {
            for (TrafficFailureOutcome candidate : values()) {
                if (candidate.wireName.equals(value)) {
                    return candidate;
                }
            }
            throw new IllegalArgumentException("unknown TrafficFailureOutcome wire value: " + value);
        }
    }

    public enum TrafficFailureRecoveryAction {
        RETRY_REQUESTED_ROUTE("retry_requested_route");

        private final String wireName;

        TrafficFailureRecoveryAction(String wireName) {
            this.wireName = wireName;
        }

        public String wireName() {
            return wireName;
        }

        public static TrafficFailureRecoveryAction fromWireName(String value) {
            for (TrafficFailureRecoveryAction candidate : values()) {
                if (candidate.wireName.equals(value)) {
                    return candidate;
                }
            }
            throw new IllegalArgumentException("unknown TrafficFailureRecoveryAction wire value: " + value);
        }
    }

    public enum TrafficPolicyChangeAction {
        APPLY("apply"),
        ROLLBACK("rollback");

        private final String wireName;

        TrafficPolicyChangeAction(String wireName) {
            this.wireName = wireName;
        }

        public String wireName() {
            return wireName;
        }

        public static TrafficPolicyChangeAction fromWireName(String value) {
            for (TrafficPolicyChangeAction candidate : values()) {
                if (candidate.wireName.equals(value)) {
                    return candidate;
                }
            }
            throw new IllegalArgumentException("unknown TrafficPolicyChangeAction wire value: " + value);
        }
    }

    public enum TrafficPolicyChangeKind {
        RULE_ADDED("rule_added"),
        RULE_REMOVED("rule_removed"),
        RULE_UPDATED("rule_updated"),
        RULES_REORDERED("rules_reordered"),
        DEFAULT_POLICY_CHANGED("default_policy_changed"),
        FAILURE_POLICY_CHANGED("failure_policy_changed");

        private final String wireName;

        TrafficPolicyChangeKind(String wireName) {
            this.wireName = wireName;
        }

        public String wireName() {
            return wireName;
        }

        public static TrafficPolicyChangeKind fromWireName(String value) {
            for (TrafficPolicyChangeKind candidate : values()) {
                if (candidate.wireName.equals(value)) {
                    return candidate;
                }
            }
            throw new IllegalArgumentException("unknown TrafficPolicyChangeKind wire value: " + value);
        }
    }

    public enum TrafficPolicyHistoryOutcome {
        APPLIED("applied"),
        ROLLED_BACK("rolled_back"),
        FAILED("failed"),
        RECOVERED("recovered");

        private final String wireName;

        TrafficPolicyHistoryOutcome(String wireName) {
            this.wireName = wireName;
        }

        public String wireName() {
            return wireName;
        }

        public static TrafficPolicyHistoryOutcome fromWireName(String value) {
            for (TrafficPolicyHistoryOutcome candidate : values()) {
                if (candidate.wireName.equals(value)) {
                    return candidate;
                }
            }
            throw new IllegalArgumentException("unknown TrafficPolicyHistoryOutcome wire value: " + value);
        }
    }

    public enum TrafficPolicyObjectKind {
        RULE("rule"),
        DEFAULT_POLICY("default_policy"),
        FAILURE_POLICY("failure_policy"),
        NETWORK("network"),
        EXIT("exit");

        private final String wireName;

        TrafficPolicyObjectKind(String wireName) {
            this.wireName = wireName;
        }

        public String wireName() {
            return wireName;
        }

        public static TrafficPolicyObjectKind fromWireName(String value) {
            for (TrafficPolicyObjectKind candidate : values()) {
                if (candidate.wireName.equals(value)) {
                    return candidate;
                }
            }
            throw new IllegalArgumentException("unknown TrafficPolicyObjectKind wire value: " + value);
        }
    }

    public enum TrafficProtocol {
        TCP("tcp"),
        UDP("udp"),
        ICMP("icmp");

        private final String wireName;

        TrafficProtocol(String wireName) {
            this.wireName = wireName;
        }

        public String wireName() {
            return wireName;
        }

        public static TrafficProtocol fromWireName(String value) {
            for (TrafficProtocol candidate : values()) {
                if (candidate.wireName.equals(value)) {
                    return candidate;
                }
            }
            throw new IllegalArgumentException("unknown TrafficProtocol wire value: " + value);
        }
    }

    public enum TunCapabilityReason {
        PLATFORM_UNSUPPORTED("platform_unsupported"),
        PERMISSION_DENIED("permission_denied"),
        DEVICE_UNAVAILABLE("device_unavailable"),
        TOOL_UNAVAILABLE("tool_unavailable"),
        UNKNOWN("unknown");

        private final String wireName;

        TunCapabilityReason(String wireName) {
            this.wireName = wireName;
        }

        public String wireName() {
            return wireName;
        }

        public static TunCapabilityReason fromWireName(String value) {
            for (TunCapabilityReason candidate : values()) {
                if (candidate.wireName.equals(value)) {
                    return candidate;
                }
            }
            throw new IllegalArgumentException("unknown TunCapabilityReason wire value: " + value);
        }
    }

    public enum TunPlatform {
        LINUX("linux"),
        MACOS("macos"),
        WINDOWS("windows"),
        OTHER("other");

        private final String wireName;

        TunPlatform(String wireName) {
            this.wireName = wireName;
        }

        public String wireName() {
            return wireName;
        }

        public static TunPlatform fromWireName(String value) {
            for (TunPlatform candidate : values()) {
                if (candidate.wireName.equals(value)) {
                    return candidate;
                }
            }
            throw new IllegalArgumentException("unknown TunPlatform wire value: " + value);
        }
    }

    public enum UiMode {
        GPUI("gpui"),
        WEB_UI("web_ui"),
        HEADLESS("headless");

        private final String wireName;

        UiMode(String wireName) {
            this.wireName = wireName;
        }

        public String wireName() {
            return wireName;
        }

        public static UiMode fromWireName(String value) {
            for (UiMode candidate : values()) {
                if (candidate.wireName.equals(value)) {
                    return candidate;
                }
            }
            throw new IllegalArgumentException("unknown UiMode wire value: " + value);
        }
    }

    public enum UiModeRequest {
        AUTO("auto"),
        GPUI("gpui"),
        WEB_UI("web_ui"),
        HEADLESS("headless");

        private final String wireName;

        UiModeRequest(String wireName) {
            this.wireName = wireName;
        }

        public String wireName() {
            return wireName;
        }

        public static UiModeRequest fromWireName(String value) {
            for (UiModeRequest candidate : values()) {
                if (candidate.wireName.equals(value)) {
                    return candidate;
                }
            }
            throw new IllegalArgumentException("unknown UiModeRequest wire value: " + value);
        }
    }

    /** DTO for ActivityEntry. */
    public static final class ActivityEntry {
        public final String action;
        public final String actor;
        public final String actor_role;
        public final String audit_id;
        public final ActivityCategory category;
        public final Map<String, Object> details;
        public final String network_id;
        public final String object_id;
        public final String object_kind;
        public final Long occurred_at_unix_ms;
        public final String operation_id;
        public final AuditOutcome outcome;
        public final String reason;
        public final String resource;
        public final ActivitySeverity severity;
        public final String summary;

        public ActivityEntry(String action, String actor, String actor_role, String audit_id, ActivityCategory category, Map<String, Object> details, String network_id, String object_id, String object_kind, Long occurred_at_unix_ms, String operation_id, AuditOutcome outcome, String reason, String resource, ActivitySeverity severity, String summary) {
            this.action = action;
            this.actor = actor;
            this.actor_role = actor_role;
            this.audit_id = audit_id;
            this.category = category;
            this.details = details;
            this.network_id = network_id;
            this.object_id = object_id;
            this.object_kind = object_kind;
            this.occurred_at_unix_ms = occurred_at_unix_ms;
            this.operation_id = operation_id;
            this.outcome = outcome;
            this.reason = reason;
            this.resource = resource;
            this.severity = severity;
            this.summary = summary;
        }
    }

    /** DTO for ConfigurationApplyResult. */
    public static final class ConfigurationApplyResult {
        public final ConfigurationHistoryEntry audit;
        public final List<ConfigurationDiffEntry> diffs;
        public final ConfigurationDocument document;
        public final Boolean no_op;
        public final Long previous_revision;
        public final Long revision;

        public ConfigurationApplyResult(ConfigurationHistoryEntry audit, List<ConfigurationDiffEntry> diffs, ConfigurationDocument document, Boolean no_op, Long previous_revision, Long revision) {
            this.audit = audit;
            this.diffs = diffs;
            this.document = document;
            this.no_op = no_op;
            this.previous_revision = previous_revision;
            this.revision = revision;
        }
    }

    /** DTO for ConfigurationDiffEntry. */
    public static final class ConfigurationDiffEntry {
        public final Object after;
        public final Object before;
        public final String impact;
        public final String path;

        public ConfigurationDiffEntry(Object after, Object before, String impact, String path) {
            this.after = after;
            this.before = before;
            this.impact = impact;
            this.path = path;
        }
    }

    /** DTO for ConfigurationDocument. */
    public static final class ConfigurationDocument {
        public final Map<String, Object> networks;
        public final Long revision;
        public final Integer schema_version;

        public ConfigurationDocument(Map<String, Object> networks, Long revision, Integer schema_version) {
            this.networks = networks;
            this.revision = revision;
            this.schema_version = schema_version;
        }
    }

    /** DTO for ConfigurationHistoryEntry. */
    public static final class ConfigurationHistoryEntry {
        public final ConfigurationChangeAction action;
        public final String actor;
        public final List<String> changed_paths;
        public final String occurred_at;
        public final Long previous_revision;
        public final Long revision;
        public final String source;
        public final Long target_revision;
        public final String transaction_id;

        public ConfigurationHistoryEntry(ConfigurationChangeAction action, String actor, List<String> changed_paths, String occurred_at, Long previous_revision, Long revision, String source, Long target_revision, String transaction_id) {
            this.action = action;
            this.actor = actor;
            this.changed_paths = changed_paths;
            this.occurred_at = occurred_at;
            this.previous_revision = previous_revision;
            this.revision = revision;
            this.source = source;
            this.target_revision = target_revision;
            this.transaction_id = transaction_id;
        }
    }

    /** DTO for ConfigurationPreview. */
    public static final class ConfigurationPreview {
        public final Boolean can_apply;
        public final ConfigurationDocument candidate;
        public final Long candidate_revision;
        public final ConfigurationDocument current;
        public final Long current_revision;
        public final List<ConfigurationDiffEntry> diffs;
        public final Long expected_revision;
        public final Boolean has_changes;
        public final Boolean revision_conflict;
        public final List<ConfigurationValidationIssue> validation_errors;

        public ConfigurationPreview(Boolean can_apply, ConfigurationDocument candidate, Long candidate_revision, ConfigurationDocument current, Long current_revision, List<ConfigurationDiffEntry> diffs, Long expected_revision, Boolean has_changes, Boolean revision_conflict, List<ConfigurationValidationIssue> validation_errors) {
            this.can_apply = can_apply;
            this.candidate = candidate;
            this.candidate_revision = candidate_revision;
            this.current = current;
            this.current_revision = current_revision;
            this.diffs = diffs;
            this.expected_revision = expected_revision;
            this.has_changes = has_changes;
            this.revision_conflict = revision_conflict;
            this.validation_errors = validation_errors;
        }
    }

    /** DTO for ConfigurationValidationIssue. */
    public static final class ConfigurationValidationIssue {
        public final ConfigurationValidationCode code;
        public final String message;
        public final String path;

        public ConfigurationValidationIssue(ConfigurationValidationCode code, String message, String path) {
            this.code = code;
            this.message = message;
            this.path = path;
        }
    }

    /** DTO for ConfirmedTrafficDirectFallback. */
    public static final class ConfirmedTrafficDirectFallback {
        public final List<String> allowed_failures;
        public final Long confirmed_at_unix_ms;
        public final TrafficDefaultConfirmedBy confirmed_by;

        public ConfirmedTrafficDirectFallback(List<String> allowed_failures, Long confirmed_at_unix_ms, TrafficDefaultConfirmedBy confirmed_by) {
            this.allowed_failures = allowed_failures;
            this.confirmed_at_unix_ms = confirmed_at_unix_ms;
            this.confirmed_by = confirmed_by;
        }
    }

    /** DTO for DomainActorRef. */
    public static final class DomainActorRef {
        public final String display;
        public final String node_id;

        public DomainActorRef(String display, String node_id) {
            this.display = display;
            this.node_id = node_id;
        }
    }

    /** DTO for DomainObjectRef. */
    public static final class DomainObjectRef {
        public final String id;
        public final ObjectKind kind;

        public DomainObjectRef(String id, ObjectKind kind) {
            this.id = id;
            this.kind = kind;
        }
    }

    /** DTO for EffectiveAccess. */
    public static final class EffectiveAccess {
        public final Boolean allowed;
        public final String code;
        public final Long evaluated_at_unix_ms;
        public final List<String> expired_grant_ids;
        public final EffectivePolicyLayer layer;
        public final Long local_policy_revision;
        public final List<String> matched_allow_rule_ids;
        public final List<String> matched_deny_rule_ids;
        public final Long network_acl_revision;
        public final EffectiveAccessReason reason;
        public final List<String> reauthentication_required_grant_ids;
        public final Boolean reauthentication_satisfied;
        public final List<String> revoked_grant_ids;
        public final String rule_id;

        public EffectiveAccess(Boolean allowed, String code, Long evaluated_at_unix_ms, List<String> expired_grant_ids, EffectivePolicyLayer layer, Long local_policy_revision, List<String> matched_allow_rule_ids, List<String> matched_deny_rule_ids, Long network_acl_revision, EffectiveAccessReason reason, List<String> reauthentication_required_grant_ids, Boolean reauthentication_satisfied, List<String> revoked_grant_ids, String rule_id) {
            this.allowed = allowed;
            this.code = code;
            this.evaluated_at_unix_ms = evaluated_at_unix_ms;
            this.expired_grant_ids = expired_grant_ids;
            this.layer = layer;
            this.local_policy_revision = local_policy_revision;
            this.matched_allow_rule_ids = matched_allow_rule_ids;
            this.matched_deny_rule_ids = matched_deny_rule_ids;
            this.network_acl_revision = network_acl_revision;
            this.reason = reason;
            this.reauthentication_required_grant_ids = reauthentication_required_grant_ids;
            this.reauthentication_satisfied = reauthentication_satisfied;
            this.revoked_grant_ids = revoked_grant_ids;
            this.rule_id = rule_id;
        }
    }

    /** DTO for ExitNodeCapabilities. */
    public static final class ExitNodeCapabilities {
        public final Boolean cleanup;
        public final Boolean ipv4_forwarding;
        public final Boolean ipv6_forwarding;
        public final Boolean nat;
        public final ExitNodePlatform platform;
        public final Integer schema_version;
        public final Boolean supported;
        public final List<ExitNodeUnsupportedReason> unsupported_reasons;

        public ExitNodeCapabilities(Boolean cleanup, Boolean ipv4_forwarding, Boolean ipv6_forwarding, Boolean nat, ExitNodePlatform platform, Integer schema_version, Boolean supported, List<ExitNodeUnsupportedReason> unsupported_reasons) {
            this.cleanup = cleanup;
            this.ipv4_forwarding = ipv4_forwarding;
            this.ipv6_forwarding = ipv6_forwarding;
            this.nat = nat;
            this.platform = platform;
            this.schema_version = schema_version;
            this.supported = supported;
            this.unsupported_reasons = unsupported_reasons;
        }
    }

    /** DTO for FieldPrivacyPolicy. */
    public static final class FieldPrivacyPolicy {
        public final Long effective_at_unix_ms;
        public final Long expires_at_unix_ms;
        public final Long issued_at_unix_ms;
        public final String network_id;
        public final Long revision;
        public final List<FieldVisibilityRule> rules;

        public FieldPrivacyPolicy(Long effective_at_unix_ms, Long expires_at_unix_ms, Long issued_at_unix_ms, String network_id, Long revision, List<FieldVisibilityRule> rules) {
            this.effective_at_unix_ms = effective_at_unix_ms;
            this.expires_at_unix_ms = expires_at_unix_ms;
            this.issued_at_unix_ms = issued_at_unix_ms;
            this.network_id = network_id;
            this.revision = revision;
            this.rules = rules;
        }
    }

    /** DTO for FieldVisibilityRule. */
    public static final class FieldVisibilityRule {
        public final PrivacyField field;
        public final FieldVisibility visibility;

        public FieldVisibilityRule(PrivacyField field, FieldVisibility visibility) {
            this.field = field;
            this.visibility = visibility;
        }
    }

    /** DTO for InvitationPreview. */
    public static final class InvitationPreview {
        public final Boolean can_accept;
        public final List<CapabilityKind> capabilities;
        public final Boolean creates_permanent_membership;
        public final String denial_key;
        public final Long expires_at_unix_ms;
        public final String invitation_id;
        public final Long invitation_revision;
        public final String network_id;
        public final String network_name;
        public final Long network_revision;
        public final List<String> permission_changes;
        public final String preview_digest;
        public final Long remaining_uses;
        public final Boolean requires_recent_reauthentication;
        public final MembershipRole role;
        public final InvitationState state;
        public final MembershipTerms terms;
        public final String terms_hash;

        public InvitationPreview(Boolean can_accept, List<CapabilityKind> capabilities, Boolean creates_permanent_membership, String denial_key, Long expires_at_unix_ms, String invitation_id, Long invitation_revision, String network_id, String network_name, Long network_revision, List<String> permission_changes, String preview_digest, Long remaining_uses, Boolean requires_recent_reauthentication, MembershipRole role, InvitationState state, MembershipTerms terms, String terms_hash) {
            this.can_accept = can_accept;
            this.capabilities = capabilities;
            this.creates_permanent_membership = creates_permanent_membership;
            this.denial_key = denial_key;
            this.expires_at_unix_ms = expires_at_unix_ms;
            this.invitation_id = invitation_id;
            this.invitation_revision = invitation_revision;
            this.network_id = network_id;
            this.network_name = network_name;
            this.network_revision = network_revision;
            this.permission_changes = permission_changes;
            this.preview_digest = preview_digest;
            this.remaining_uses = remaining_uses;
            this.requires_recent_reauthentication = requires_recent_reauthentication;
            this.role = role;
            this.state = state;
            this.terms = terms;
            this.terms_hash = terms_hash;
        }
    }

    /** DTO for InvitationRecord. */
    public static final class InvitationRecord {
        public final List<CapabilityKind> capabilities;
        public final Long created_at_unix_ms;
        public final String created_by;
        public final Long expires_at_unix_ms;
        public final String invitation_id;
        public final Long max_uses;
        public final String network_id;
        public final Long revision;
        public final Long revoked_at_unix_ms;
        public final String revoked_by;
        public final MembershipRole role;
        public final Long used_count;

        public InvitationRecord(List<CapabilityKind> capabilities, Long created_at_unix_ms, String created_by, Long expires_at_unix_ms, String invitation_id, Long max_uses, String network_id, Long revision, Long revoked_at_unix_ms, String revoked_by, MembershipRole role, Long used_count) {
            this.capabilities = capabilities;
            this.created_at_unix_ms = created_at_unix_ms;
            this.created_by = created_by;
            this.expires_at_unix_ms = expires_at_unix_ms;
            this.invitation_id = invitation_id;
            this.max_uses = max_uses;
            this.network_id = network_id;
            this.revision = revision;
            this.revoked_at_unix_ms = revoked_at_unix_ms;
            this.revoked_by = revoked_by;
            this.role = role;
            this.used_count = used_count;
        }
    }

    /** DTO for InvitationRedeemRequest. */
    public static final class InvitationRedeemRequest {
        public final Long acknowledged_at_unix_ms;
        public final String code;
        public final Boolean exit_contribution_acknowledged;
        public final Long expected_invitation_revision;
        public final String invitation_id;
        public final String network_id;
        public final String preview_digest;
        public final String terms_hash;

        public InvitationRedeemRequest(Long acknowledged_at_unix_ms, String code, Boolean exit_contribution_acknowledged, Long expected_invitation_revision, String invitation_id, String network_id, String preview_digest, String terms_hash) {
            this.acknowledged_at_unix_ms = acknowledged_at_unix_ms;
            this.code = code;
            this.exit_contribution_acknowledged = exit_contribution_acknowledged;
            this.expected_invitation_revision = expected_invitation_revision;
            this.invitation_id = invitation_id;
            this.network_id = network_id;
            this.preview_digest = preview_digest;
            this.terms_hash = terms_hash;
        }
    }

    /** DTO for InvitationRedemption. */
    public static final class InvitationRedemption {
        public final Boolean already_redeemed;
        public final List<CapabilityKind> capabilities;
        public final Boolean grants_permanent_membership;
        public final String invitation_id;
        public final Long invitation_revision;
        public final String invited_by;
        public final Long joined_at_unix_ms;
        public final String network_id;
        public final Long network_revision;
        public final String node_id;
        public final MembershipRole role;

        public InvitationRedemption(Boolean already_redeemed, List<CapabilityKind> capabilities, Boolean grants_permanent_membership, String invitation_id, Long invitation_revision, String invited_by, Long joined_at_unix_ms, String network_id, Long network_revision, String node_id, MembershipRole role) {
            this.already_redeemed = already_redeemed;
            this.capabilities = capabilities;
            this.grants_permanent_membership = grants_permanent_membership;
            this.invitation_id = invitation_id;
            this.invitation_revision = invitation_revision;
            this.invited_by = invited_by;
            this.joined_at_unix_ms = joined_at_unix_ms;
            this.network_id = network_id;
            this.network_revision = network_revision;
            this.node_id = node_id;
            this.role = role;
        }
    }

    /** DTO for InvitationStatusView. */
    public static final class InvitationStatusView {
        public final InvitationRecord invitation;
        public final Long remaining_uses;
        public final InvitationState state;

        public InvitationStatusView(InvitationRecord invitation, Long remaining_uses, InvitationState state) {
            this.invitation = invitation;
            this.remaining_uses = remaining_uses;
            this.state = state;
        }
    }

    /** DTO for IssuedInvitation. */
    public static final class IssuedInvitation {
        public final String code;
        public final InvitationStatusView status;

        public IssuedInvitation(String code, InvitationStatusView status) {
            this.code = code;
            this.status = status;
        }
    }

    /** DTO for JoinApplicantProfile. */
    public static final class JoinApplicantProfile {
        public final String display_name;
        public final String node_id;
        public final String public_key_fingerprint;

        public JoinApplicantProfile(String display_name, String node_id, String public_key_fingerprint) {
            this.display_name = display_name;
            this.node_id = node_id;
            this.public_key_fingerprint = public_key_fingerprint;
        }
    }

    /** DTO for JoinRequest. */
    public static final class JoinRequest {
        public final JoinApplicantProfile applicant;
        public final Long approved_at_unix_ms;
        public final Long created_at_unix_ms;
        public final Long decided_at_unix_ms;
        public final Long expires_at_unix_ms;
        public final JoinRequestGrantPreview grant_preview;
        public final String information_request;
        public final String network_id;
        public final String reason;
        public final Long reason_retention_until_unix_ms;
        public final JoinRequestReasonVisibility reason_visibility;
        public final String request_id;
        public final String reviewer_node_id;
        public final Long revision;
        public final Integer schema_version;
        public final JoinRequestState state;
        public final MembershipTerms terms;
        public final Long updated_at_unix_ms;

        public JoinRequest(JoinApplicantProfile applicant, Long approved_at_unix_ms, Long created_at_unix_ms, Long decided_at_unix_ms, Long expires_at_unix_ms, JoinRequestGrantPreview grant_preview, String information_request, String network_id, String reason, Long reason_retention_until_unix_ms, JoinRequestReasonVisibility reason_visibility, String request_id, String reviewer_node_id, Long revision, Integer schema_version, JoinRequestState state, MembershipTerms terms, Long updated_at_unix_ms) {
            this.applicant = applicant;
            this.approved_at_unix_ms = approved_at_unix_ms;
            this.created_at_unix_ms = created_at_unix_ms;
            this.decided_at_unix_ms = decided_at_unix_ms;
            this.expires_at_unix_ms = expires_at_unix_ms;
            this.grant_preview = grant_preview;
            this.information_request = information_request;
            this.network_id = network_id;
            this.reason = reason;
            this.reason_retention_until_unix_ms = reason_retention_until_unix_ms;
            this.reason_visibility = reason_visibility;
            this.request_id = request_id;
            this.reviewer_node_id = reviewer_node_id;
            this.revision = revision;
            this.schema_version = schema_version;
            this.state = state;
            this.terms = terms;
            this.updated_at_unix_ms = updated_at_unix_ms;
        }
    }

    /** DTO for JoinRequestGrantPreview. */
    public static final class JoinRequestGrantPreview {
        public final List<CapabilityKind> capabilities;
        public final ExitContributionObligation exit_contribution;
        public final String exit_contribution_scope;
        public final Boolean file_access;
        public final Boolean grants_data_plane_access;
        public final MembershipRole role;

        public JoinRequestGrantPreview(List<CapabilityKind> capabilities, ExitContributionObligation exit_contribution, String exit_contribution_scope, Boolean file_access, Boolean grants_data_plane_access, MembershipRole role) {
            this.capabilities = capabilities;
            this.exit_contribution = exit_contribution;
            this.exit_contribution_scope = exit_contribution_scope;
            this.file_access = file_access;
            this.grants_data_plane_access = grants_data_plane_access;
            this.role = role;
        }
    }

    /** DTO for MemberLifecycleChange. */
    public static final class MemberLifecycleChange {
        public final Long access_epoch;
        public final Boolean access_revoked;
        public final MemberLifecycleAction action;
        public final String audit_id;
        public final Boolean idempotent;
        public final String network_id;
        public final String operation_id;
        public final Boolean requires_reapproval;
        public final Boolean requires_reauthentication;
        public final Long revision;
        public final MemberLifecycleStatus status;
        public final String target_node_id;

        public MemberLifecycleChange(Long access_epoch, Boolean access_revoked, MemberLifecycleAction action, String audit_id, Boolean idempotent, String network_id, String operation_id, Boolean requires_reapproval, Boolean requires_reauthentication, Long revision, MemberLifecycleStatus status, String target_node_id) {
            this.access_epoch = access_epoch;
            this.access_revoked = access_revoked;
            this.action = action;
            this.audit_id = audit_id;
            this.idempotent = idempotent;
            this.network_id = network_id;
            this.operation_id = operation_id;
            this.requires_reapproval = requires_reapproval;
            this.requires_reauthentication = requires_reauthentication;
            this.revision = revision;
            this.status = status;
            this.target_node_id = target_node_id;
        }
    }

    /** DTO for MemberRemovalPreview. */
    public static final class MemberRemovalPreview {
        public final Long access_epoch;
        public final List<CapabilityKind> active_capabilities;
        public final Boolean can_remove;
        public final Boolean credentials_revoked;
        public final String denial_key;
        public final String display_name;
        public final Boolean file_access_revoked;
        public final List<String> impact_keys;
        public final Boolean incomplete_transfers_resume_blocked;
        public final MemberLifecycleStatus lifecycle_state;
        public final Boolean local_files_deleted;
        public final Boolean network_access_revoked;
        public final String network_id;
        public final Boolean old_access_epoch_invalidated;
        public final NetworkMemberPresence presence;
        public final Long revision;
        public final MembershipRole role;
        public final Boolean safe_undo_requires_reapproval;
        public final Boolean safe_undo_restores_old_credentials;
        public final NetworkMemberStatus status;
        public final String target_node_id;

        public MemberRemovalPreview(Long access_epoch, List<CapabilityKind> active_capabilities, Boolean can_remove, Boolean credentials_revoked, String denial_key, String display_name, Boolean file_access_revoked, List<String> impact_keys, Boolean incomplete_transfers_resume_blocked, MemberLifecycleStatus lifecycle_state, Boolean local_files_deleted, Boolean network_access_revoked, String network_id, Boolean old_access_epoch_invalidated, NetworkMemberPresence presence, Long revision, MembershipRole role, Boolean safe_undo_requires_reapproval, Boolean safe_undo_restores_old_credentials, NetworkMemberStatus status, String target_node_id) {
            this.access_epoch = access_epoch;
            this.active_capabilities = active_capabilities;
            this.can_remove = can_remove;
            this.credentials_revoked = credentials_revoked;
            this.denial_key = denial_key;
            this.display_name = display_name;
            this.file_access_revoked = file_access_revoked;
            this.impact_keys = impact_keys;
            this.incomplete_transfers_resume_blocked = incomplete_transfers_resume_blocked;
            this.lifecycle_state = lifecycle_state;
            this.local_files_deleted = local_files_deleted;
            this.network_access_revoked = network_access_revoked;
            this.network_id = network_id;
            this.old_access_epoch_invalidated = old_access_epoch_invalidated;
            this.presence = presence;
            this.revision = revision;
            this.role = role;
            this.safe_undo_requires_reapproval = safe_undo_requires_reapproval;
            this.safe_undo_restores_old_credentials = safe_undo_restores_old_credentials;
            this.status = status;
            this.target_node_id = target_node_id;
        }
    }

    /** DTO for MembershipTerms. */
    public static final class MembershipTerms {
        public final ExitContributionObligation exit_contribution;
        public final String exit_contribution_scope;
        public final Long member_limit;
        public final Long network_policy_revision;
        public final Long privacy_policy_revision;

        public MembershipTerms(ExitContributionObligation exit_contribution, String exit_contribution_scope, Long member_limit, Long network_policy_revision, Long privacy_policy_revision) {
            this.exit_contribution = exit_contribution;
            this.exit_contribution_scope = exit_contribution_scope;
            this.member_limit = member_limit;
            this.network_policy_revision = network_policy_revision;
            this.privacy_policy_revision = privacy_policy_revision;
        }
    }

    /** DTO for NetworkAcl. */
    public static final class NetworkAcl {
        public final Long expires_at_unix_ms;
        public final List<NetworkAclGrant> grants;
        public final Long issued_at_unix_ms;
        public final String network_id;
        public final Long revision;
        public final List<PermissionRule> rules;

        public NetworkAcl(Long expires_at_unix_ms, List<NetworkAclGrant> grants, Long issued_at_unix_ms, String network_id, Long revision, List<PermissionRule> rules) {
            this.expires_at_unix_ms = expires_at_unix_ms;
            this.grants = grants;
            this.issued_at_unix_ms = issued_at_unix_ms;
            this.network_id = network_id;
            this.revision = revision;
            this.rules = rules;
        }
    }

    /** DTO for NetworkAclGrant. */
    public static final class NetworkAclGrant {
        public final List<PolicyAction> actions;
        public final Long expires_at_unix_ms;
        public final String grant_id;
        public final Long issued_at_unix_ms;
        public final Boolean requires_recent_reauthentication;
        public final List<ResourceKind> resources;
        public final Long revoked_at_unix_ms;
        public final String revoked_by;
        public final List<SubjectSelector> subjects;

        public NetworkAclGrant(List<PolicyAction> actions, Long expires_at_unix_ms, String grant_id, Long issued_at_unix_ms, Boolean requires_recent_reauthentication, List<ResourceKind> resources, Long revoked_at_unix_ms, String revoked_by, List<SubjectSelector> subjects) {
            this.actions = actions;
            this.expires_at_unix_ms = expires_at_unix_ms;
            this.grant_id = grant_id;
            this.issued_at_unix_ms = issued_at_unix_ms;
            this.requires_recent_reauthentication = requires_recent_reauthentication;
            this.resources = resources;
            this.revoked_at_unix_ms = revoked_at_unix_ms;
            this.revoked_by = revoked_by;
            this.subjects = subjects;
        }
    }

    /** DTO for NetworkAdminProfileProjection. */
    public static final class NetworkAdminProfileProjection {
        public final Long administrator_count;
        public final Long member_count;
        public final Long policy_revision;
        public final NetworkPublicProfileDocument profile;
        public final Long revision;

        public NetworkAdminProfileProjection(Long administrator_count, Long member_count, Long policy_revision, NetworkPublicProfileDocument profile, Long revision) {
            this.administrator_count = administrator_count;
            this.member_count = member_count;
            this.policy_revision = policy_revision;
            this.profile = profile;
            this.revision = revision;
        }
    }

    /** DTO for NetworkDiscoveryDocument. */
    public static final class NetworkDiscoveryDocument {
        public final List<String> administrators;
        public final List<CapabilityKind> capability_summary;
        public final List<NetworkEndpoint> endpoints;
        public final ExitContributionObligation exit_contribution;
        public final String exit_contribution_scope;
        public final Long expires_at_unix_ms;
        public final Long issued_at_unix_ms;
        public final String join_mode;
        public final String language;
        public final Long member_count;
        public final Long member_limit;
        public final String name;
        public final String network_id;
        public final String owner;
        public final Long policy_revision;
        public final List<String> privacy_changes;
        public final Long privacy_policy_revision;
        public final String public_key;
        public final String publisher;
        public final String purpose;
        public final String region;
        public final Long revision;
        public final Long revoked_at_unix_ms;
        public final Integer schema_version;
        public final List<String> tags;
        public final NetworkVisibility visibility;

        public NetworkDiscoveryDocument(List<String> administrators, List<CapabilityKind> capability_summary, List<NetworkEndpoint> endpoints, ExitContributionObligation exit_contribution, String exit_contribution_scope, Long expires_at_unix_ms, Long issued_at_unix_ms, String join_mode, String language, Long member_count, Long member_limit, String name, String network_id, String owner, Long policy_revision, List<String> privacy_changes, Long privacy_policy_revision, String public_key, String publisher, String purpose, String region, Long revision, Long revoked_at_unix_ms, Integer schema_version, List<String> tags, NetworkVisibility visibility) {
            this.administrators = administrators;
            this.capability_summary = capability_summary;
            this.endpoints = endpoints;
            this.exit_contribution = exit_contribution;
            this.exit_contribution_scope = exit_contribution_scope;
            this.expires_at_unix_ms = expires_at_unix_ms;
            this.issued_at_unix_ms = issued_at_unix_ms;
            this.join_mode = join_mode;
            this.language = language;
            this.member_count = member_count;
            this.member_limit = member_limit;
            this.name = name;
            this.network_id = network_id;
            this.owner = owner;
            this.policy_revision = policy_revision;
            this.privacy_changes = privacy_changes;
            this.privacy_policy_revision = privacy_policy_revision;
            this.public_key = public_key;
            this.publisher = publisher;
            this.purpose = purpose;
            this.region = region;
            this.revision = revision;
            this.revoked_at_unix_ms = revoked_at_unix_ms;
            this.schema_version = schema_version;
            this.tags = tags;
            this.visibility = visibility;
        }
    }

    /** DTO for NetworkDiscoveryRecord. */
    public static final class NetworkDiscoveryRecord {
        public final NetworkDiscoveryDocument document;
        public final String signature;

        public NetworkDiscoveryRecord(NetworkDiscoveryDocument document, String signature) {
            this.document = document;
            this.signature = signature;
        }
    }

    /** DTO for NetworkEndpoint. */
    public static final class NetworkEndpoint {
        public final String host;
        public final Integer port;
        public final DiscoveryTransport transport;

        public NetworkEndpoint(String host, Integer port, DiscoveryTransport transport) {
            this.host = host;
            this.port = port;
            this.transport = transport;
        }
    }

    /** DTO for NetworkLocalSecretProjection. */
    public static final class NetworkLocalSecretProjection {
        public final Boolean identity_present;
        public final String public_key_fingerprint;
        public final Boolean secret_material_included;
        public final String storage;

        public NetworkLocalSecretProjection(Boolean identity_present, String public_key_fingerprint, Boolean secret_material_included, String storage) {
            this.identity_present = identity_present;
            this.public_key_fingerprint = public_key_fingerprint;
            this.secret_material_included = secret_material_included;
            this.storage = storage;
        }
    }

    /** DTO for NetworkMember. */
    public static final class NetworkMember {
        public final Long access_epoch;
        public final List<MemberLifecycleAction> available_actions;
        public final List<CapabilityKind> capabilities;
        public final String display_name;
        public final List<NetworkMemberGroup> groups;
        public final String identity_fingerprint;
        public final Long joined_at_unix_ms;
        public final MemberLifecycleStatus lifecycle_state;
        public final String node_id;
        public final NetworkMemberPresence presence;
        public final Long revision;
        public final MembershipRole role;
        public final NetworkMemberStatus status;

        public NetworkMember(Long access_epoch, List<MemberLifecycleAction> available_actions, List<CapabilityKind> capabilities, String display_name, List<NetworkMemberGroup> groups, String identity_fingerprint, Long joined_at_unix_ms, MemberLifecycleStatus lifecycle_state, String node_id, NetworkMemberPresence presence, Long revision, MembershipRole role, NetworkMemberStatus status) {
            this.access_epoch = access_epoch;
            this.available_actions = available_actions;
            this.capabilities = capabilities;
            this.display_name = display_name;
            this.groups = groups;
            this.identity_fingerprint = identity_fingerprint;
            this.joined_at_unix_ms = joined_at_unix_ms;
            this.lifecycle_state = lifecycle_state;
            this.node_id = node_id;
            this.presence = presence;
            this.revision = revision;
            this.role = role;
            this.status = status;
        }
    }

    /** DTO for NetworkMemberDirectory. */
    public static final class NetworkMemberDirectory {
        public final Boolean can_manage_members;
        public final Long generated_at_unix_ms;
        public final List<NetworkMember> members;
        public final String network_id;
        public final MembershipRole viewer_role;

        public NetworkMemberDirectory(Boolean can_manage_members, Long generated_at_unix_ms, List<NetworkMember> members, String network_id, MembershipRole viewer_role) {
            this.can_manage_members = can_manage_members;
            this.generated_at_unix_ms = generated_at_unix_ms;
            this.members = members;
            this.network_id = network_id;
            this.viewer_role = viewer_role;
        }
    }

    /** DTO for NetworkMemberPresence. */
    public static final class NetworkMemberPresence {
        public final Long observed_at_unix_ms;
        public final NetworkMemberObservationSource source;
        public final NetworkMemberOnlineState state;

        public NetworkMemberPresence(Long observed_at_unix_ms, NetworkMemberObservationSource source, NetworkMemberOnlineState state) {
            this.observed_at_unix_ms = observed_at_unix_ms;
            this.source = source;
            this.state = state;
        }
    }

    /** DTO for NetworkMemberProfileProjection. */
    public static final class NetworkMemberProfileProjection {
        public final Long administrator_count;
        public final Long member_count;
        public final NetworkPublicProfileDocument profile;

        public NetworkMemberProfileProjection(Long administrator_count, Long member_count, NetworkPublicProfileDocument profile) {
            this.administrator_count = administrator_count;
            this.member_count = member_count;
            this.profile = profile;
        }
    }

    /** DTO for NetworkProfileApplyResult. */
    public static final class NetworkProfileApplyResult {
        public final Long applied_at_unix_ms;
        public final String audit_action;
        public final List<String> changed_paths;
        public final String network_name;
        public final Long previous_revision;
        public final NetworkProfileProjections projections;
        public final Long revision;
        public final NetworkPublicProfileRecord signed_profile;
        public final List<NetworkProfileWarning> warnings;

        public NetworkProfileApplyResult(Long applied_at_unix_ms, String audit_action, List<String> changed_paths, String network_name, Long previous_revision, NetworkProfileProjections projections, Long revision, NetworkPublicProfileRecord signed_profile, List<NetworkProfileWarning> warnings) {
            this.applied_at_unix_ms = applied_at_unix_ms;
            this.audit_action = audit_action;
            this.changed_paths = changed_paths;
            this.network_name = network_name;
            this.previous_revision = previous_revision;
            this.projections = projections;
            this.revision = revision;
            this.signed_profile = signed_profile;
            this.warnings = warnings;
        }
    }

    /** DTO for NetworkProfileDiffEntry. */
    public static final class NetworkProfileDiffEntry {
        public final String after;
        public final String before;
        public final String path;

        public NetworkProfileDiffEntry(String after, String before, String path) {
            this.after = after;
            this.before = before;
            this.path = path;
        }
    }

    /** DTO for NetworkProfilePreview. */
    public static final class NetworkProfilePreview {
        public final Boolean can_apply;
        public final Long candidate_revision;
        public final Boolean conflict;
        public final Long current_revision;
        public final List<NetworkProfileDiffEntry> diff;
        public final Long expected_revision;
        public final String network_name;
        public final NetworkProfileProjections projections;
        public final List<NetworkProfileWarning> warnings;

        public NetworkProfilePreview(Boolean can_apply, Long candidate_revision, Boolean conflict, Long current_revision, List<NetworkProfileDiffEntry> diff, Long expected_revision, String network_name, NetworkProfileProjections projections, List<NetworkProfileWarning> warnings) {
            this.can_apply = can_apply;
            this.candidate_revision = candidate_revision;
            this.conflict = conflict;
            this.current_revision = current_revision;
            this.diff = diff;
            this.expected_revision = expected_revision;
            this.network_name = network_name;
            this.projections = projections;
            this.warnings = warnings;
        }
    }

    /** DTO for NetworkProfileProjections. */
    public static final class NetworkProfileProjections {
        public final NetworkAdminProfileProjection administrator_view;
        public final NetworkLocalSecretProjection local_secret;
        public final NetworkMemberProfileProjection member_view;
        public final NetworkPublicProfileProjection public_card;

        public NetworkProfileProjections(NetworkAdminProfileProjection administrator_view, NetworkLocalSecretProjection local_secret, NetworkMemberProfileProjection member_view, NetworkPublicProfileProjection public_card) {
            this.administrator_view = administrator_view;
            this.local_secret = local_secret;
            this.member_view = member_view;
            this.public_card = public_card;
        }
    }

    /** DTO for NetworkProfileUpdate. */
    public static final class NetworkProfileUpdate {
        public final List<CapabilityKind> capability_declarations;
        public final ExitContributionObligation exit_contribution;
        public final String exit_contribution_scope;
        public final String icon;
        public final String join_mode;
        public final String language;
        public final String purpose;
        public final String region;
        public final List<String> tags;
        public final NetworkVisibility visibility;

        public NetworkProfileUpdate(List<CapabilityKind> capability_declarations, ExitContributionObligation exit_contribution, String exit_contribution_scope, String icon, String join_mode, String language, String purpose, String region, List<String> tags, NetworkVisibility visibility) {
            this.capability_declarations = capability_declarations;
            this.exit_contribution = exit_contribution;
            this.exit_contribution_scope = exit_contribution_scope;
            this.icon = icon;
            this.join_mode = join_mode;
            this.language = language;
            this.purpose = purpose;
            this.region = region;
            this.tags = tags;
            this.visibility = visibility;
        }
    }

    /** DTO for NetworkProfileWarning. */
    public static final class NetworkProfileWarning {
        public final CapabilityKind capability;
        public final NetworkProfileWarningCode code;
        public final String field;
        public final String message;

        public NetworkProfileWarning(CapabilityKind capability, NetworkProfileWarningCode code, String field, String message) {
            this.capability = capability;
            this.code = code;
            this.field = field;
            this.message = message;
        }
    }

    /** DTO for NetworkPublicProfileDocument. */
    public static final class NetworkPublicProfileDocument {
        public final List<CapabilityKind> capability_declarations;
        public final Long expires_at_unix_ms;
        public final String icon;
        public final Long issued_at_unix_ms;
        public final String join_mode;
        public final String language;
        public final String name;
        public final String network_id;
        public final String owner;
        public final String public_key;
        public final String purpose;
        public final String region;
        public final Long revision;
        public final Integer schema_version;
        public final List<String> tags;
        public final NetworkVisibility visibility;

        public NetworkPublicProfileDocument(List<CapabilityKind> capability_declarations, Long expires_at_unix_ms, String icon, Long issued_at_unix_ms, String join_mode, String language, String name, String network_id, String owner, String public_key, String purpose, String region, Long revision, Integer schema_version, List<String> tags, NetworkVisibility visibility) {
            this.capability_declarations = capability_declarations;
            this.expires_at_unix_ms = expires_at_unix_ms;
            this.icon = icon;
            this.issued_at_unix_ms = issued_at_unix_ms;
            this.join_mode = join_mode;
            this.language = language;
            this.name = name;
            this.network_id = network_id;
            this.owner = owner;
            this.public_key = public_key;
            this.purpose = purpose;
            this.region = region;
            this.revision = revision;
            this.schema_version = schema_version;
            this.tags = tags;
            this.visibility = visibility;
        }
    }

    /** DTO for NetworkPublicProfileProjection. */
    public static final class NetworkPublicProfileProjection {
        public final NetworkPublicProfileDocument candidate;
        public final NetworkPublicProfileRecord currently_published;
        public final List<String> withheld_categories;

        public NetworkPublicProfileProjection(NetworkPublicProfileDocument candidate, NetworkPublicProfileRecord currently_published, List<String> withheld_categories) {
            this.candidate = candidate;
            this.currently_published = currently_published;
            this.withheld_categories = withheld_categories;
        }
    }

    /** DTO for NetworkPublicProfileRecord. */
    public static final class NetworkPublicProfileRecord {
        public final NetworkPublicProfileDocument document;
        public final String signature;

        public NetworkPublicProfileRecord(NetworkPublicProfileDocument document, String signature) {
            this.document = document;
            this.signature = signature;
        }
    }

    /** DTO for NodeAccessControlSnapshot. */
    public static final class NodeAccessControlSnapshot {
        public final List<FieldPrivacyPolicy> field_privacy_policies;
        public final NodeLocalAccessPolicy local_policy;
        public final List<NetworkAcl> network_acls;

        public NodeAccessControlSnapshot(List<FieldPrivacyPolicy> field_privacy_policies, NodeLocalAccessPolicy local_policy, List<NetworkAcl> network_acls) {
            this.field_privacy_policies = field_privacy_policies;
            this.local_policy = local_policy;
            this.network_acls = network_acls;
        }
    }

    /** DTO for NodeAcknowledgeMembershipPolicyCommand. */
    public static final class NodeAcknowledgeMembershipPolicyCommand {
        public final Long acknowledged_at_unix_ms;
        public final Boolean exit_contribution_acknowledged;
        public final String expected_terms_hash;
        public final String network_id;

        public NodeAcknowledgeMembershipPolicyCommand(Long acknowledged_at_unix_ms, Boolean exit_contribution_acknowledged, String expected_terms_hash, String network_id) {
            this.acknowledged_at_unix_ms = acknowledged_at_unix_ms;
            this.exit_contribution_acknowledged = exit_contribution_acknowledged;
            this.expected_terms_hash = expected_terms_hash;
            this.network_id = network_id;
        }
    }

    /** DTO for NodeApiCommandResponse. */
    public static final class NodeApiCommandResponse {
        public final NodeOperationSnapshot operation;
        public final Long request_id;
        public final ProjectedNodeCommandResult result;
        public final Integer schema_version;
        public final String server_time;

        public NodeApiCommandResponse(NodeOperationSnapshot operation, Long request_id, ProjectedNodeCommandResult result, Integer schema_version, String server_time) {
            this.operation = operation;
            this.request_id = request_id;
            this.result = result;
            this.schema_version = schema_version;
            this.server_time = server_time;
        }
    }

    /** DTO for NodeApiErrorResponse. */
    public static final class NodeApiErrorResponse {
        public final NodeErrorCategory category;
        public final NodeHttpErrorCode code;
        public final NodeEventCursor latest_cursor;
        public final String message;
        public final String message_key;
        public final DomainObjectRef object;
        public final Map<String, Object> params;
        public final NodeProblem problem;
        public final OperationRecovery recovery;
        public final Long request_id;
        public final NodeEventCursor requested_cursor;
        public final Integer requested_schema_version;
        public final NodeEventReplayReason resync_reason;
        public final Boolean retryable;
        public final Integer schema_version;
        public final String server_time;
        public final Integer supported_schema_version;

        public NodeApiErrorResponse(NodeErrorCategory category, NodeHttpErrorCode code, NodeEventCursor latest_cursor, String message, String message_key, DomainObjectRef object, Map<String, Object> params, NodeProblem problem, OperationRecovery recovery, Long request_id, NodeEventCursor requested_cursor, Integer requested_schema_version, NodeEventReplayReason resync_reason, Boolean retryable, Integer schema_version, String server_time, Integer supported_schema_version) {
            this.category = category;
            this.code = code;
            this.latest_cursor = latest_cursor;
            this.message = message;
            this.message_key = message_key;
            this.object = object;
            this.params = params;
            this.problem = problem;
            this.recovery = recovery;
            this.request_id = request_id;
            this.requested_cursor = requested_cursor;
            this.requested_schema_version = requested_schema_version;
            this.resync_reason = resync_reason;
            this.retryable = retryable;
            this.schema_version = schema_version;
            this.server_time = server_time;
            this.supported_schema_version = supported_schema_version;
        }
    }

    /** DTO for NodeApiManifestResponse. */
    public static final class NodeApiManifestResponse {
        public final NodeUiManifest manifest;
        public final Long request_id;
        public final Integer schema_version;
        public final String server_time;

        public NodeApiManifestResponse(NodeUiManifest manifest, Long request_id, Integer schema_version, String server_time) {
            this.manifest = manifest;
            this.request_id = request_id;
            this.schema_version = schema_version;
            this.server_time = server_time;
        }
    }

    /** DTO for NodeApiObjectResponse. */
    public static final class NodeApiObjectResponse {
        public final NodeApiObject object;
        public final Long request_id;
        public final Integer schema_version;
        public final String server_time;

        public NodeApiObjectResponse(NodeApiObject object, Long request_id, Integer schema_version, String server_time) {
            this.object = object;
            this.request_id = request_id;
            this.schema_version = schema_version;
            this.server_time = server_time;
        }
    }

    /** DTO for NodeApiOperationResponse. */
    public static final class NodeApiOperationResponse {
        public final NodeOperationSnapshot operation;
        public final Long request_id;
        public final Integer schema_version;
        public final String server_time;

        public NodeApiOperationResponse(NodeOperationSnapshot operation, Long request_id, Integer schema_version, String server_time) {
            this.operation = operation;
            this.request_id = request_id;
            this.schema_version = schema_version;
            this.server_time = server_time;
        }
    }

    /** DTO for NodeApiSnapshotResponse. */
    public static final class NodeApiSnapshotResponse {
        public final Long request_id;
        public final Integer schema_version;
        public final String server_time;
        public final ProjectedNodeSnapshot snapshot;

        public NodeApiSnapshotResponse(Long request_id, Integer schema_version, String server_time, ProjectedNodeSnapshot snapshot) {
            this.request_id = request_id;
            this.schema_version = schema_version;
            this.server_time = server_time;
            this.snapshot = snapshot;
        }
    }

    /** DTO for NodeApplyConfigurationCommand. */
    public static final class NodeApplyConfigurationCommand {
        public final Long expected_revision;
        public final Map<String, Object> networks;

        public NodeApplyConfigurationCommand(Long expected_revision, Map<String, Object> networks) {
            this.expected_revision = expected_revision;
            this.networks = networks;
        }
    }

    /** DTO for NodeApplyPolicyCommand. */
    public static final class NodeApplyPolicyCommand {
        public final PolicySnapshot candidate;
        public final Boolean confirm_privilege_expansion;
        public final Long expected_revision;
        public final Boolean includes_local_policy;
        public final List<String> network_ids;

        public NodeApplyPolicyCommand(PolicySnapshot candidate, Boolean confirm_privilege_expansion, Long expected_revision, Boolean includes_local_policy, List<String> network_ids) {
            this.candidate = candidate;
            this.confirm_privilege_expansion = confirm_privilege_expansion;
            this.expected_revision = expected_revision;
            this.includes_local_policy = includes_local_policy;
            this.network_ids = network_ids;
        }
    }

    /** DTO for NodeApplyTrafficPolicyCommand. */
    public static final class NodeApplyTrafficPolicyCommand {
        public final Boolean confirm_risk;
        public final TrafficPolicyDraft draft;
        public final Long expected_revision;

        public NodeApplyTrafficPolicyCommand(Boolean confirm_risk, TrafficPolicyDraft draft, Long expected_revision) {
            this.confirm_risk = confirm_risk;
            this.draft = draft;
            this.expected_revision = expected_revision;
        }
    }

    /** DTO for NodeBatchReviewJoinRequestsCommand. */
    public static final class NodeBatchReviewJoinRequestsCommand {
        public final String network_id;
        public final List<NodeJoinRequestReviewItem> reviews;

        public NodeBatchReviewJoinRequestsCommand(String network_id, List<NodeJoinRequestReviewItem> reviews) {
            this.network_id = network_id;
            this.reviews = reviews;
        }
    }

    /** DTO for NodeBlacklistSnapshot. */
    public static final class NodeBlacklistSnapshot {
        public final Boolean changed;
        public final List<String> clients;
        public final String network_name;

        public NodeBlacklistSnapshot(Boolean changed, List<String> clients, String network_name) {
            this.changed = changed;
            this.clients = clients;
            this.network_name = network_name;
        }
    }

    /** DTO for NodeCapabilityRegistrySnapshot. */
    public static final class NodeCapabilityRegistrySnapshot {
        public final List<NodeCapabilitySnapshot> capabilities;

        public NodeCapabilityRegistrySnapshot(List<NodeCapabilitySnapshot> capabilities) {
            this.capabilities = capabilities;
        }
    }

    /** DTO for NodeCapabilitySnapshot. */
    public static final class NodeCapabilitySnapshot {
        public final CapabilityKind kind;
        public final String state;

        public NodeCapabilitySnapshot(CapabilityKind kind, String state) {
            this.kind = kind;
            this.state = state;
        }
    }

    /** DTO for NodeChangeMemberLifecycleCommand. */
    public static final class NodeChangeMemberLifecycleCommand {
        public final MemberLifecycleAction action;
        public final Long expected_revision;
        public final String network_id;
        public final String reason;
        public final MembershipRole role;
        public final String target_node_id;

        public NodeChangeMemberLifecycleCommand(MemberLifecycleAction action, Long expected_revision, String network_id, String reason, MembershipRole role, String target_node_id) {
            this.action = action;
            this.expected_revision = expected_revision;
            this.network_id = network_id;
            this.reason = reason;
            this.role = role;
            this.target_node_id = target_node_id;
        }
    }

    /** DTO for NodeCleanupFailureSnapshot. */
    public static final class NodeCleanupFailureSnapshot {
        public final String network_name;
        public final NodeCleanupFailureReason reason;

        public NodeCleanupFailureSnapshot(String network_name, NodeCleanupFailureReason reason) {
            this.network_name = network_name;
            this.reason = reason;
        }
    }

    /** DTO for NodeCleanupStepSnapshot. */
    public static final class NodeCleanupStepSnapshot {
        public final String detail;
        public final String network_name;
        public final String resource;
        public final NodeCleanupStepStatus status;

        public NodeCleanupStepSnapshot(String detail, String network_name, String resource, NodeCleanupStepStatus status) {
            this.detail = detail;
            this.network_name = network_name;
            this.resource = resource;
            this.status = status;
        }
    }

    /** DTO for NodeClientBlacklistCommand. */
    public static final class NodeClientBlacklistCommand {
        public final String client_id;
        public final String network_name;

        public NodeClientBlacklistCommand(String client_id, String network_name) {
            this.client_id = client_id;
            this.network_name = network_name;
        }
    }

    /** DTO for NodeCommandHttpRequest. */
    public static final class NodeCommandHttpRequest {
        public final String idempotency_key;
        public final NodeCommandRequest request;
        public final Integer schema_version;

        public NodeCommandHttpRequest(String idempotency_key, NodeCommandRequest request, Integer schema_version) {
            this.idempotency_key = idempotency_key;
            this.request = request;
            this.schema_version = schema_version;
        }
    }

    /** DTO for NodeCommandHttpResponse. */
    public static final class NodeCommandHttpResponse {
        public final ProjectedNodeCommandResult result;
        public final Integer schema_version;

        public NodeCommandHttpResponse(ProjectedNodeCommandResult result, Integer schema_version) {
            this.result = result;
            this.schema_version = schema_version;
        }
    }

    /** DTO for NodeCommandRequest. */
    public static final class NodeCommandRequest {
        public final NodeCommand command;
        public final Long request_id;

        public NodeCommandRequest(NodeCommand command, Long request_id) {
            this.command = command;
            this.request_id = request_id;
        }
    }

    /** DTO for NodeCommandResult. */
    public static final class NodeCommandResult {
        public final NodeCommandOutcome outcome;
        public final Long request_id;

        public NodeCommandResult(NodeCommandOutcome outcome, Long request_id) {
            this.outcome = outcome;
            this.request_id = request_id;
        }
    }

    /** DTO for NodeConfigurationSnapshot. */
    public static final class NodeConfigurationSnapshot {
        public final List<NodeNetworkConfigSnapshot> networks;

        public NodeConfigurationSnapshot(List<NodeNetworkConfigSnapshot> networks) {
            this.networks = networks;
        }
    }

    /** DTO for NodeContributionVeto. */
    public static final class NodeContributionVeto {
        public final List<ContributionKind> contributions;
        public final String network_id;
        public final String veto_id;

        public NodeContributionVeto(List<ContributionKind> contributions, String network_id, String veto_id) {
            this.contributions = contributions;
            this.network_id = network_id;
            this.veto_id = veto_id;
        }
    }

    /** DTO for NodeCreateInvitationCommand. */
    public static final class NodeCreateInvitationCommand {
        public final List<CapabilityKind> capabilities;
        public final Long expires_in_ms;
        public final Long max_uses;
        public final String network_id;
        public final MembershipRole role;

        public NodeCreateInvitationCommand(List<CapabilityKind> capabilities, Long expires_in_ms, Long max_uses, String network_id, MembershipRole role) {
            this.capabilities = capabilities;
            this.expires_in_ms = expires_in_ms;
            this.max_uses = max_uses;
            this.network_id = network_id;
            this.role = role;
        }
    }

    /** DTO for NodeCreateNetworkCommand. */
    public static final class NodeCreateNetworkCommand {
        public final Boolean auto_connect;
        public final List<CapabilityKind> capability_declarations;
        public final ExitContributionObligation exit_contribution;
        public final String exit_contribution_scope;
        public final String external_host;
        public final String icon;
        public final String join_mode;
        public final String language;
        public final String name;
        public final String purpose;
        public final String region;
        public final Integer relay_port;
        public final Integer signaling_port;
        public final List<String> tags;
        public final NetworkVisibility visibility;

        public NodeCreateNetworkCommand(Boolean auto_connect, List<CapabilityKind> capability_declarations, ExitContributionObligation exit_contribution, String exit_contribution_scope, String external_host, String icon, String join_mode, String language, String name, String purpose, String region, Integer relay_port, Integer signaling_port, List<String> tags, NetworkVisibility visibility) {
            this.auto_connect = auto_connect;
            this.capability_declarations = capability_declarations;
            this.exit_contribution = exit_contribution;
            this.exit_contribution_scope = exit_contribution_scope;
            this.external_host = external_host;
            this.icon = icon;
            this.join_mode = join_mode;
            this.language = language;
            this.name = name;
            this.purpose = purpose;
            this.region = region;
            this.relay_port = relay_port;
            this.signaling_port = signaling_port;
            this.tags = tags;
            this.visibility = visibility;
        }
    }

    /** DTO for NodeCreatedNetwork. */
    public static final class NodeCreatedNetwork {
        public final Boolean config_saved;
        public final String invitation_ticket;
        public final String network_id;
        public final String network_name;
        public final List<NodeNetworkCreationNextStep> next_steps;
        public final MembershipRole owner_role;
        public final String public_key;
        public final Boolean started;

        public NodeCreatedNetwork(Boolean config_saved, String invitation_ticket, String network_id, String network_name, List<NodeNetworkCreationNextStep> next_steps, MembershipRole owner_role, String public_key, Boolean started) {
            this.config_saved = config_saved;
            this.invitation_ticket = invitation_ticket;
            this.network_id = network_id;
            this.network_name = network_name;
            this.next_steps = next_steps;
            this.owner_role = owner_role;
            this.public_key = public_key;
            this.started = started;
        }
    }

    /** DTO for NodeDiagnosticCheck. */
    public static final class NodeDiagnosticCheck {
        public final Long duration_ms;
        public final String error_code;
        public final NodeDiagnosticLayer layer;
        public final String message_key;
        public final Long started_at_unix_ms;
        public final NodeDiagnosticState state;
        public final String suggestion_key;

        public NodeDiagnosticCheck(Long duration_ms, String error_code, NodeDiagnosticLayer layer, String message_key, Long started_at_unix_ms, NodeDiagnosticState state, String suggestion_key) {
            this.duration_ms = duration_ms;
            this.error_code = error_code;
            this.layer = layer;
            this.message_key = message_key;
            this.started_at_unix_ms = started_at_unix_ms;
            this.state = state;
            this.suggestion_key = suggestion_key;
        }
    }

    /** DTO for NodeDiagnosticRecoveryAction. */
    public static final class NodeDiagnosticRecoveryAction {
        public final String action_id;
        public final Boolean confirmation_required;
        public final String impact_key;

        public NodeDiagnosticRecoveryAction(String action_id, Boolean confirmation_required, String impact_key) {
            this.action_id = action_id;
            this.confirmation_required = confirmation_required;
            this.impact_key = impact_key;
        }
    }

    /** DTO for NodeDiagnosticsSnapshot. */
    public static final class NodeDiagnosticsSnapshot {
        public final List<NodeDiagnosticCheck> checks;
        public final List<String> cleanup_candidate_networks;
        public final Long completed_at_unix_ms;
        public final String diagnostic_id;
        public final NodeLifecycleSnapshot lifecycle;
        public final List<NodeDiagnosticRecoveryAction> recovery_actions;
        public final NodeRuntimeResidueSnapshot residue;
        public final List<String> retained_owner_networks;
        public final String sanitized_report;
        public final Long started_at_unix_ms;
        public final Boolean timed_out;

        public NodeDiagnosticsSnapshot(List<NodeDiagnosticCheck> checks, List<String> cleanup_candidate_networks, Long completed_at_unix_ms, String diagnostic_id, NodeLifecycleSnapshot lifecycle, List<NodeDiagnosticRecoveryAction> recovery_actions, NodeRuntimeResidueSnapshot residue, List<String> retained_owner_networks, String sanitized_report, Long started_at_unix_ms, Boolean timed_out) {
            this.checks = checks;
            this.cleanup_candidate_networks = cleanup_candidate_networks;
            this.completed_at_unix_ms = completed_at_unix_ms;
            this.diagnostic_id = diagnostic_id;
            this.lifecycle = lifecycle;
            this.recovery_actions = recovery_actions;
            this.residue = residue;
            this.retained_owner_networks = retained_owner_networks;
            this.sanitized_report = sanitized_report;
            this.started_at_unix_ms = started_at_unix_ms;
            this.timed_out = timed_out;
        }
    }

    /** DTO for NodeDiscoveryIndexError. */
    public static final class NodeDiscoveryIndexError {
        public final NodeDiscoveryIndexErrorCategory category;
        public final Boolean retryable;

        public NodeDiscoveryIndexError(NodeDiscoveryIndexErrorCategory category, Boolean retryable) {
            this.category = category;
            this.retryable = retryable;
        }
    }

    /** DTO for NodeDiscoveryIndexMetrics. */
    public static final class NodeDiscoveryIndexMetrics {
        public final Long cancellations;
        public final Long last_request_duration_ms;
        public final Long records_accepted;
        public final Long records_rejected;
        public final Long request_attempts;
        public final Long requests_failed;
        public final Long requests_succeeded;
        public final Long retries_scheduled;

        public NodeDiscoveryIndexMetrics(Long cancellations, Long last_request_duration_ms, Long records_accepted, Long records_rejected, Long request_attempts, Long requests_failed, Long requests_succeeded, Long retries_scheduled) {
            this.cancellations = cancellations;
            this.last_request_duration_ms = last_request_duration_ms;
            this.records_accepted = records_accepted;
            this.records_rejected = records_rejected;
            this.request_attempts = request_attempts;
            this.requests_failed = requests_failed;
            this.requests_succeeded = requests_succeeded;
            this.retries_scheduled = retries_scheduled;
        }
    }

    /** DTO for NodeDiscoveryIndexRecordSnapshot. */
    public static final class NodeDiscoveryIndexRecordSnapshot {
        public final Long accepted_at_unix_ms;
        public final String digest;
        public final Boolean discoverable;
        public final String invalid_reason;
        public final NetworkDiscoveryRecord record;
        public final NodeDiscoveryIndexSource source;

        public NodeDiscoveryIndexRecordSnapshot(Long accepted_at_unix_ms, String digest, Boolean discoverable, String invalid_reason, NetworkDiscoveryRecord record, NodeDiscoveryIndexSource source) {
            this.accepted_at_unix_ms = accepted_at_unix_ms;
            this.digest = digest;
            this.discoverable = discoverable;
            this.invalid_reason = invalid_reason;
            this.record = record;
            this.source = source;
        }
    }

    /** DTO for NodeDiscoveryIndexSnapshot. */
    public static final class NodeDiscoveryIndexSnapshot {
        public final Boolean data_plane_affected;
        public final String index_url;
        public final Long last_attempt_unix_ms;
        public final NodeDiscoveryIndexError last_error;
        public final Long last_success_unix_ms;
        public final NodeDiscoveryIndexMetrics metrics;
        public final List<NodeDiscoveryIndexRecordSnapshot> records;
        public final Long rejected_record_count;
        public final Boolean search_history_stored;
        public final NodeDiscoveryIndexSource source;
        public final NodeDiscoveryIndexState state;

        public NodeDiscoveryIndexSnapshot(Boolean data_plane_affected, String index_url, Long last_attempt_unix_ms, NodeDiscoveryIndexError last_error, Long last_success_unix_ms, NodeDiscoveryIndexMetrics metrics, List<NodeDiscoveryIndexRecordSnapshot> records, Long rejected_record_count, Boolean search_history_stored, NodeDiscoveryIndexSource source, NodeDiscoveryIndexState state) {
            this.data_plane_affected = data_plane_affected;
            this.index_url = index_url;
            this.last_attempt_unix_ms = last_attempt_unix_ms;
            this.last_error = last_error;
            this.last_success_unix_ms = last_success_unix_ms;
            this.metrics = metrics;
            this.records = records;
            this.rejected_record_count = rejected_record_count;
            this.search_history_stored = search_history_stored;
            this.source = source;
            this.state = state;
        }
    }

    /** DTO for NodeEmbeddedServiceSnapshot. */
    public static final class NodeEmbeddedServiceSnapshot {
        public final String network_name;
        public final String relay_addr;
        public final String signaling_addr;
        public final NodeEmbeddedServiceStatus status;

        public NodeEmbeddedServiceSnapshot(String network_name, String relay_addr, String signaling_addr, NodeEmbeddedServiceStatus status) {
            this.network_name = network_name;
            this.relay_addr = relay_addr;
            this.signaling_addr = signaling_addr;
            this.status = status;
        }
    }

    /** DTO for NodeEvent. */
    public static final class NodeEvent {
        public final NodeEventKind kind;
        public final Long sequence;
        public final Long snapshot_revision;

        public NodeEvent(NodeEventKind kind, Long sequence, Long snapshot_revision) {
            this.kind = kind;
            this.sequence = sequence;
            this.snapshot_revision = snapshot_revision;
        }
    }

    /** DTO for NodeEventCursor. */
    public static final class NodeEventCursor {
        public final Long sequence;
        public final Long snapshot_revision;

        public NodeEventCursor(Long sequence, Long snapshot_revision) {
            this.sequence = sequence;
            this.snapshot_revision = snapshot_revision;
        }
    }

    /** DTO for NodeGetInvitationStatusCommand. */
    public static final class NodeGetInvitationStatusCommand {
        public final String invitation_id;
        public final String network_id;

        public NodeGetInvitationStatusCommand(String invitation_id, String network_id) {
            this.invitation_id = invitation_id;
            this.network_id = network_id;
        }
    }

    /** DTO for NodeHttpContract. */
    public static final class NodeHttpContract {
        public final NodeApiCommandResponse api_command_response;
        public final NodeApiErrorResponse api_error_response;
        public final NodeApiEvent api_event;
        public final NodeApiManifestResponse api_manifest_response;
        public final NodeApiObjectResponse api_object_response;
        public final NodeApiOperationResponse api_operation_response;
        public final NodeApiSnapshotResponse api_snapshot_response;
        public final NodeCommandHttpRequest command_request;
        public final NodeCommandHttpResponse command_response;
        public final NodeHttpErrorResponse error_response;
        public final NodeManifestResponse manifest_response;
        public final NodeSnapshotResponse snapshot_response;
        public final NodeWsMessage websocket_message;

        public NodeHttpContract(NodeApiCommandResponse api_command_response, NodeApiErrorResponse api_error_response, NodeApiEvent api_event, NodeApiManifestResponse api_manifest_response, NodeApiObjectResponse api_object_response, NodeApiOperationResponse api_operation_response, NodeApiSnapshotResponse api_snapshot_response, NodeCommandHttpRequest command_request, NodeCommandHttpResponse command_response, NodeHttpErrorResponse error_response, NodeManifestResponse manifest_response, NodeSnapshotResponse snapshot_response, NodeWsMessage websocket_message) {
            this.api_command_response = api_command_response;
            this.api_error_response = api_error_response;
            this.api_event = api_event;
            this.api_manifest_response = api_manifest_response;
            this.api_object_response = api_object_response;
            this.api_operation_response = api_operation_response;
            this.api_snapshot_response = api_snapshot_response;
            this.command_request = command_request;
            this.command_response = command_response;
            this.error_response = error_response;
            this.manifest_response = manifest_response;
            this.snapshot_response = snapshot_response;
            this.websocket_message = websocket_message;
        }
    }

    /** DTO for NodeHttpErrorResponse. */
    public static final class NodeHttpErrorResponse {
        public final NodeErrorCategory category;
        public final NodeHttpErrorCode code;
        public final String message;
        public final String message_key;
        public final DomainObjectRef object;
        public final Map<String, Object> params;
        public final NodeProblem problem;
        public final OperationRecovery recovery;
        public final Integer requested_schema_version;
        public final Boolean retryable;
        public final Integer schema_version;
        public final Integer supported_schema_version;

        public NodeHttpErrorResponse(NodeErrorCategory category, NodeHttpErrorCode code, String message, String message_key, DomainObjectRef object, Map<String, Object> params, NodeProblem problem, OperationRecovery recovery, Integer requested_schema_version, Boolean retryable, Integer schema_version, Integer supported_schema_version) {
            this.category = category;
            this.code = code;
            this.message = message;
            this.message_key = message_key;
            this.object = object;
            this.params = params;
            this.problem = problem;
            this.recovery = recovery;
            this.requested_schema_version = requested_schema_version;
            this.retryable = retryable;
            this.schema_version = schema_version;
            this.supported_schema_version = supported_schema_version;
        }
    }

    /** DTO for NodeIdentityRotationSnapshot. */
    public static final class NodeIdentityRotationSnapshot {
        public final Long affected_configured_network_count;
        public final Long created_at_unix_ms;
        public final String new_fingerprint;
        public final String new_node_id;
        public final String old_fingerprint;
        public final String old_node_id;
        public final Boolean owner_initialized;
        public final Boolean owner_sessions_invalidated;

        public NodeIdentityRotationSnapshot(Long affected_configured_network_count, Long created_at_unix_ms, String new_fingerprint, String new_node_id, String old_fingerprint, String old_node_id, Boolean owner_initialized, Boolean owner_sessions_invalidated) {
            this.affected_configured_network_count = affected_configured_network_count;
            this.created_at_unix_ms = created_at_unix_ms;
            this.new_fingerprint = new_fingerprint;
            this.new_node_id = new_node_id;
            this.old_fingerprint = old_fingerprint;
            this.old_node_id = old_node_id;
            this.owner_initialized = owner_initialized;
            this.owner_sessions_invalidated = owner_sessions_invalidated;
        }
    }

    /** DTO for NodeIdentitySnapshot. */
    public static final class NodeIdentitySnapshot {
        public final Long created_at_unix_ms;
        public final String display_name;
        public final String node_id;
        public final String public_key_fingerprint;

        public NodeIdentitySnapshot(Long created_at_unix_ms, String display_name, String node_id, String public_key_fingerprint) {
            this.created_at_unix_ms = created_at_unix_ms;
            this.display_name = display_name;
            this.node_id = node_id;
            this.public_key_fingerprint = public_key_fingerprint;
        }
    }

    /** DTO for NodeInterfaceAddress. */
    public static final class NodeInterfaceAddress {
        public final String address;
        public final Integer prefix_len;

        public NodeInterfaceAddress(String address, Integer prefix_len) {
            this.address = address;
            this.prefix_len = prefix_len;
        }
    }

    /** DTO for NodeInterfaceSnapshot. */
    public static final class NodeInterfaceSnapshot {
        public final List<NodeInterfaceAddress> addresses;
        public final Boolean default_ipv4;
        public final Boolean default_ipv6;
        public final Long index;
        public final NodeInterfaceKind kind;
        public final Long mtu;
        public final String name;
        public final NodeInterfaceOwnership ownership;
        public final NodeInterfaceStatus status;

        public NodeInterfaceSnapshot(List<NodeInterfaceAddress> addresses, Boolean default_ipv4, Boolean default_ipv6, Long index, NodeInterfaceKind kind, Long mtu, String name, NodeInterfaceOwnership ownership, NodeInterfaceStatus status) {
            this.addresses = addresses;
            this.default_ipv4 = default_ipv4;
            this.default_ipv6 = default_ipv6;
            this.index = index;
            this.kind = kind;
            this.mtu = mtu;
            this.name = name;
            this.ownership = ownership;
            this.status = status;
        }
    }

    /** DTO for NodeJoinNetworkCommand. */
    public static final class NodeJoinNetworkCommand {
        public final Long acknowledged_at_unix_ms;
        public final NetworkDiscoveryRecord discovery_record;
        public final Boolean exit_contribution_acknowledged;
        public final String network_id;
        public final String network_name;
        public final Long network_policy_revision;
        public final String preview_digest;
        public final Long privacy_policy_revision;
        public final Boolean start;
        public final String target;
        public final String terms_hash;

        public NodeJoinNetworkCommand(Long acknowledged_at_unix_ms, NetworkDiscoveryRecord discovery_record, Boolean exit_contribution_acknowledged, String network_id, String network_name, Long network_policy_revision, String preview_digest, Long privacy_policy_revision, Boolean start, String target, String terms_hash) {
            this.acknowledged_at_unix_ms = acknowledged_at_unix_ms;
            this.discovery_record = discovery_record;
            this.exit_contribution_acknowledged = exit_contribution_acknowledged;
            this.network_id = network_id;
            this.network_name = network_name;
            this.network_policy_revision = network_policy_revision;
            this.preview_digest = preview_digest;
            this.privacy_policy_revision = privacy_policy_revision;
            this.start = start;
            this.target = target;
            this.terms_hash = terms_hash;
        }
    }

    /** DTO for NodeJoinRequestListResult. */
    public static final class NodeJoinRequestListResult {
        public final String network_id;
        public final List<JoinRequest> requests;

        public NodeJoinRequestListResult(String network_id, List<JoinRequest> requests) {
            this.network_id = network_id;
            this.requests = requests;
        }
    }

    /** DTO for NodeJoinRequestReviewAck. */
    public static final class NodeJoinRequestReviewAck {
        public final String network_id;
        public final List<NodeJoinRequestReviewOutcome> reviews;

        public NodeJoinRequestReviewAck(String network_id, List<NodeJoinRequestReviewOutcome> reviews) {
            this.network_id = network_id;
            this.reviews = reviews;
        }
    }

    /** DTO for NodeJoinRequestReviewItem. */
    public static final class NodeJoinRequestReviewItem {
        public final JoinRequestReviewAction action;
        public final Long expected_revision;
        public final String note;
        public final String request_id;

        public NodeJoinRequestReviewItem(JoinRequestReviewAction action, Long expected_revision, String note, String request_id) {
            this.action = action;
            this.expected_revision = expected_revision;
            this.note = note;
            this.request_id = request_id;
        }
    }

    /** DTO for NodeJoinRequestReviewOutcome. */
    public static final class NodeJoinRequestReviewOutcome {
        public final String request_id;
        public final Long revision;
        public final JoinRequestState state;

        public NodeJoinRequestReviewOutcome(String request_id, Long revision, JoinRequestState state) {
            this.request_id = request_id;
            this.revision = revision;
            this.state = state;
        }
    }

    /** DTO for NodeLeaveNetworkCommand. */
    public static final class NodeLeaveNetworkCommand {
        public final String network_name;

        public NodeLeaveNetworkCommand(String network_name) {
            this.network_name = network_name;
        }
    }

    /** DTO for NodeLifecycleSnapshot. */
    public static final class NodeLifecycleSnapshot {
        public final Long active_networks;
        public final Long configured_networks;
        public final List<String> failed_networks;
        public final NodePhase phase;

        public NodeLifecycleSnapshot(Long active_networks, Long configured_networks, List<String> failed_networks, NodePhase phase) {
            this.active_networks = active_networks;
            this.configured_networks = configured_networks;
            this.failed_networks = failed_networks;
            this.phase = phase;
        }
    }

    /** DTO for NodeListInvitationsCommand. */
    public static final class NodeListInvitationsCommand {
        public final String network_id;

        public NodeListInvitationsCommand(String network_id) {
            this.network_id = network_id;
        }
    }

    /** DTO for NodeListJoinRequestsCommand. */
    public static final class NodeListJoinRequestsCommand {
        public final String network_id;

        public NodeListJoinRequestsCommand(String network_id) {
            this.network_id = network_id;
        }
    }

    /** DTO for NodeLocalAccessPolicy. */
    public static final class NodeLocalAccessPolicy {
        public final List<NodeContributionVeto> contribution_vetoes;
        public final Long expires_at_unix_ms;
        public final Long issued_at_unix_ms;
        public final NodeNetworkScope network_scope;
        public final String node_id;
        public final Long revision;
        public final List<PermissionRule> rules;

        public NodeLocalAccessPolicy(List<NodeContributionVeto> contribution_vetoes, Long expires_at_unix_ms, Long issued_at_unix_ms, NodeNetworkScope network_scope, String node_id, Long revision, List<PermissionRule> rules) {
            this.contribution_vetoes = contribution_vetoes;
            this.expires_at_unix_ms = expires_at_unix_ms;
            this.issued_at_unix_ms = issued_at_unix_ms;
            this.network_scope = network_scope;
            this.node_id = node_id;
            this.revision = revision;
            this.rules = rules;
        }
    }

    /** DTO for NodeLogEntry. */
    public static final class NodeLogEntry {
        public final NodeLogLevel level;
        public final String message;
        public final String target;
        public final Long unix_ms;

        public NodeLogEntry(NodeLogLevel level, String message, String target, Long unix_ms) {
            this.level = level;
            this.message = message;
            this.target = target;
            this.unix_ms = unix_ms;
        }
    }

    /** DTO for NodeManifestResponse. */
    public static final class NodeManifestResponse {
        public final NodeUiManifest manifest;
        public final Integer schema_version;

        public NodeManifestResponse(NodeUiManifest manifest, Integer schema_version) {
            this.manifest = manifest;
            this.schema_version = schema_version;
        }
    }

    /** DTO for NodeMemberRemovalPreviewCommand. */
    public static final class NodeMemberRemovalPreviewCommand {
        public final String network_id;
        public final String target_node_id;

        public NodeMemberRemovalPreviewCommand(String network_id, String target_node_id) {
            this.network_id = network_id;
            this.target_node_id = target_node_id;
        }
    }

    /** DTO for NodeNetworkAllocation. */
    public static final class NodeNetworkAllocation {
        public final String ipv4_pool;
        public final String ipv6_pool;
        public final String mode;

        public NodeNetworkAllocation(String ipv4_pool, String ipv6_pool, String mode) {
            this.ipv4_pool = ipv4_pool;
            this.ipv6_pool = ipv6_pool;
            this.mode = mode;
        }
    }

    /** DTO for NodeNetworkAllocationSnapshot. */
    public static final class NodeNetworkAllocationSnapshot {
        public final NodeNetworkAllocation allocation;
        public final Boolean config_saved;
        public final String network_name;

        public NodeNetworkAllocationSnapshot(NodeNetworkAllocation allocation, Boolean config_saved, String network_name) {
            this.allocation = allocation;
            this.config_saved = config_saved;
            this.network_name = network_name;
        }
    }

    /** DTO for NodeNetworkConfigSnapshot. */
    public static final class NodeNetworkConfigSnapshot {
        public final AclAction acl_default_action;
        public final Boolean acl_enabled;
        public final Long acl_rule_count;
        public final String coordinator_url;
        public final List<String> direct_cidrs;
        public final String exit_node;
        public final Boolean exit_node_allow_lan_access;
        public final List<String> exit_node_allowed_clients;
        public final Boolean exit_node_enabled;
        public final String exit_node_region;
        public final Boolean identity_configured;
        public final Boolean log_file_configured;
        public final String log_level;
        public final Integer mtu;
        public final String network_name;
        public final Long p2p_connect_timeout_secs;
        public final Long p2p_heartbeat_interval_secs;
        public final Integer p2p_listen_port;
        public final String preferred_ipv4;
        public final String preferred_ipv6;
        public final List<String> proxy_cidrs;
        public final NodeRoutingMode routing_mode;
        public final String tun_name;
        public final String virtual_mode;
        public final String virtual_network;

        public NodeNetworkConfigSnapshot(AclAction acl_default_action, Boolean acl_enabled, Long acl_rule_count, String coordinator_url, List<String> direct_cidrs, String exit_node, Boolean exit_node_allow_lan_access, List<String> exit_node_allowed_clients, Boolean exit_node_enabled, String exit_node_region, Boolean identity_configured, Boolean log_file_configured, String log_level, Integer mtu, String network_name, Long p2p_connect_timeout_secs, Long p2p_heartbeat_interval_secs, Integer p2p_listen_port, String preferred_ipv4, String preferred_ipv6, List<String> proxy_cidrs, NodeRoutingMode routing_mode, String tun_name, String virtual_mode, String virtual_network) {
            this.acl_default_action = acl_default_action;
            this.acl_enabled = acl_enabled;
            this.acl_rule_count = acl_rule_count;
            this.coordinator_url = coordinator_url;
            this.direct_cidrs = direct_cidrs;
            this.exit_node = exit_node;
            this.exit_node_allow_lan_access = exit_node_allow_lan_access;
            this.exit_node_allowed_clients = exit_node_allowed_clients;
            this.exit_node_enabled = exit_node_enabled;
            this.exit_node_region = exit_node_region;
            this.identity_configured = identity_configured;
            this.log_file_configured = log_file_configured;
            this.log_level = log_level;
            this.mtu = mtu;
            this.network_name = network_name;
            this.p2p_connect_timeout_secs = p2p_connect_timeout_secs;
            this.p2p_heartbeat_interval_secs = p2p_heartbeat_interval_secs;
            this.p2p_listen_port = p2p_listen_port;
            this.preferred_ipv4 = preferred_ipv4;
            this.preferred_ipv6 = preferred_ipv6;
            this.proxy_cidrs = proxy_cidrs;
            this.routing_mode = routing_mode;
            this.tun_name = tun_name;
            this.virtual_mode = virtual_mode;
            this.virtual_network = virtual_network;
        }
    }

    /** DTO for NodeNetworkDiscoveryCommand. */
    public static final class NodeNetworkDiscoveryCommand {
        public final String index_url;
        public final String network_name;

        public NodeNetworkDiscoveryCommand(String index_url, String network_name) {
            this.index_url = index_url;
            this.network_name = network_name;
        }
    }

    /** DTO for NodeNetworkJoinPreview. */
    public static final class NodeNetworkJoinPreview {
        public final List<String> administrators;
        public final Boolean already_member;
        public final String blocked_reason_key;
        public final Boolean can_join;
        public final Boolean can_join_without_local_exit;
        public final List<CapabilityKind> capability_summary;
        public final String contribution_policy_key;
        public final Long discovery_revision;
        public final List<NetworkEndpoint> endpoints;
        public final ExitContributionObligation exit_contribution;
        public final String exit_contribution_bandwidth_impact_key;
        public final String exit_contribution_disable_key;
        public final String exit_contribution_logging_boundary_key;
        public final String exit_contribution_purpose_key;
        public final String exit_contribution_scope;
        public final Long expires_at_unix_ms;
        public final Long issued_at_unix_ms;
        public final String join_mode;
        public final Boolean join_requires_exit_consent;
        public final Boolean local_exit_available;
        public final Boolean local_exit_enabled;
        public final List<CapabilityKind> locally_contributed_capabilities;
        public final Long member_count;
        public final Long member_limit;
        public final String network_id;
        public final String network_name;
        public final Long network_policy_revision;
        public final String owner;
        public final String preview_digest;
        public final List<String> privacy_changes;
        public final Long privacy_policy_revision;
        public final String public_key;
        public final String terms_hash;
        public final NetworkVisibility visibility;

        public NodeNetworkJoinPreview(List<String> administrators, Boolean already_member, String blocked_reason_key, Boolean can_join, Boolean can_join_without_local_exit, List<CapabilityKind> capability_summary, String contribution_policy_key, Long discovery_revision, List<NetworkEndpoint> endpoints, ExitContributionObligation exit_contribution, String exit_contribution_bandwidth_impact_key, String exit_contribution_disable_key, String exit_contribution_logging_boundary_key, String exit_contribution_purpose_key, String exit_contribution_scope, Long expires_at_unix_ms, Long issued_at_unix_ms, String join_mode, Boolean join_requires_exit_consent, Boolean local_exit_available, Boolean local_exit_enabled, List<CapabilityKind> locally_contributed_capabilities, Long member_count, Long member_limit, String network_id, String network_name, Long network_policy_revision, String owner, String preview_digest, List<String> privacy_changes, Long privacy_policy_revision, String public_key, String terms_hash, NetworkVisibility visibility) {
            this.administrators = administrators;
            this.already_member = already_member;
            this.blocked_reason_key = blocked_reason_key;
            this.can_join = can_join;
            this.can_join_without_local_exit = can_join_without_local_exit;
            this.capability_summary = capability_summary;
            this.contribution_policy_key = contribution_policy_key;
            this.discovery_revision = discovery_revision;
            this.endpoints = endpoints;
            this.exit_contribution = exit_contribution;
            this.exit_contribution_bandwidth_impact_key = exit_contribution_bandwidth_impact_key;
            this.exit_contribution_disable_key = exit_contribution_disable_key;
            this.exit_contribution_logging_boundary_key = exit_contribution_logging_boundary_key;
            this.exit_contribution_purpose_key = exit_contribution_purpose_key;
            this.exit_contribution_scope = exit_contribution_scope;
            this.expires_at_unix_ms = expires_at_unix_ms;
            this.issued_at_unix_ms = issued_at_unix_ms;
            this.join_mode = join_mode;
            this.join_requires_exit_consent = join_requires_exit_consent;
            this.local_exit_available = local_exit_available;
            this.local_exit_enabled = local_exit_enabled;
            this.locally_contributed_capabilities = locally_contributed_capabilities;
            this.member_count = member_count;
            this.member_limit = member_limit;
            this.network_id = network_id;
            this.network_name = network_name;
            this.network_policy_revision = network_policy_revision;
            this.owner = owner;
            this.preview_digest = preview_digest;
            this.privacy_changes = privacy_changes;
            this.privacy_policy_revision = privacy_policy_revision;
            this.public_key = public_key;
            this.terms_hash = terms_hash;
            this.visibility = visibility;
        }
    }

    /** DTO for NodeNetworkJoinResult. */
    public static final class NodeNetworkJoinResult {
        public final Boolean config_saved;
        public final Boolean idempotent;
        public final NodeNetworkMembershipSnapshot membership;
        public final NodeSnapshot snapshot;
        public final Boolean started;

        public NodeNetworkJoinResult(Boolean config_saved, Boolean idempotent, NodeNetworkMembershipSnapshot membership, NodeSnapshot snapshot, Boolean started) {
            this.config_saved = config_saved;
            this.idempotent = idempotent;
            this.membership = membership;
            this.snapshot = snapshot;
            this.started = started;
        }
    }

    /** DTO for NodeNetworkLeaveResult. */
    public static final class NodeNetworkLeaveResult {
        public final Boolean left;
        public final String network_name;
        public final NodeSnapshot snapshot;

        public NodeNetworkLeaveResult(Boolean left, String network_name, NodeSnapshot snapshot) {
            this.left = left;
            this.network_name = network_name;
            this.snapshot = snapshot;
        }
    }

    /** DTO for NodeNetworkMembersCommand. */
    public static final class NodeNetworkMembersCommand {
        public final String network_id;

        public NodeNetworkMembersCommand(String network_id) {
            this.network_id = network_id;
        }
    }

    /** DTO for NodeNetworkMembershipSnapshot. */
    public static final class NodeNetworkMembershipSnapshot {
        public final Long acknowledged_at_unix_ms;
        public final Long created_at_unix_ms;
        public final MembershipTerms current_terms;
        public final String current_terms_hash;
        public final ExitContributionObligation exit_contribution;
        public final String exit_contribution_scope;
        public final Long local_policy_revision;
        public final Long member_limit;
        public final String network_id;
        public final String network_name;
        public final Long network_policy_revision;
        public final String node_id;
        public final Boolean policy_confirmation_required;
        public final Long privacy_policy_revision;
        public final MembershipRole role;
        public final MembershipState state;
        public final String suspension_reason;
        public final String terms_hash;
        public final Long updated_at_unix_ms;

        public NodeNetworkMembershipSnapshot(Long acknowledged_at_unix_ms, Long created_at_unix_ms, MembershipTerms current_terms, String current_terms_hash, ExitContributionObligation exit_contribution, String exit_contribution_scope, Long local_policy_revision, Long member_limit, String network_id, String network_name, Long network_policy_revision, String node_id, Boolean policy_confirmation_required, Long privacy_policy_revision, MembershipRole role, MembershipState state, String suspension_reason, String terms_hash, Long updated_at_unix_ms) {
            this.acknowledged_at_unix_ms = acknowledged_at_unix_ms;
            this.created_at_unix_ms = created_at_unix_ms;
            this.current_terms = current_terms;
            this.current_terms_hash = current_terms_hash;
            this.exit_contribution = exit_contribution;
            this.exit_contribution_scope = exit_contribution_scope;
            this.local_policy_revision = local_policy_revision;
            this.member_limit = member_limit;
            this.network_id = network_id;
            this.network_name = network_name;
            this.network_policy_revision = network_policy_revision;
            this.node_id = node_id;
            this.policy_confirmation_required = policy_confirmation_required;
            this.privacy_policy_revision = privacy_policy_revision;
            this.role = role;
            this.state = state;
            this.suspension_reason = suspension_reason;
            this.terms_hash = terms_hash;
            this.updated_at_unix_ms = updated_at_unix_ms;
        }
    }

    /** DTO for NodeNetworkProfileCommand. */
    public static final class NodeNetworkProfileCommand {
        public final Long expected_revision;
        public final String network_name;
        public final NetworkProfileUpdate update;

        public NodeNetworkProfileCommand(Long expected_revision, String network_name, NetworkProfileUpdate update) {
            this.expected_revision = expected_revision;
            this.network_name = network_name;
            this.update = update;
        }
    }

    /** DTO for NodeNetworkProfileSnapshot. */
    public static final class NodeNetworkProfileSnapshot {
        public final List<CapabilityKind> capability_declarations;
        public final ExitContributionObligation exit_contribution;
        public final String exit_contribution_scope;
        public final String icon;
        public final String join_mode;
        public final String language;
        public final String name;
        public final String network_id;
        public final Long policy_revision;
        public final String purpose;
        public final String region;
        public final Long revision;
        public final NodeSignedNetworkProfileSnapshot signed_profile;
        public final List<String> tags;
        public final NetworkVisibility visibility;

        public NodeNetworkProfileSnapshot(List<CapabilityKind> capability_declarations, ExitContributionObligation exit_contribution, String exit_contribution_scope, String icon, String join_mode, String language, String name, String network_id, Long policy_revision, String purpose, String region, Long revision, NodeSignedNetworkProfileSnapshot signed_profile, List<String> tags, NetworkVisibility visibility) {
            this.capability_declarations = capability_declarations;
            this.exit_contribution = exit_contribution;
            this.exit_contribution_scope = exit_contribution_scope;
            this.icon = icon;
            this.join_mode = join_mode;
            this.language = language;
            this.name = name;
            this.network_id = network_id;
            this.policy_revision = policy_revision;
            this.purpose = purpose;
            this.region = region;
            this.revision = revision;
            this.signed_profile = signed_profile;
            this.tags = tags;
            this.visibility = visibility;
        }
    }

    /** DTO for NodeNetworkSnapshot. */
    public static final class NodeNetworkSnapshot {
        public final String assigned_ipv4;
        public final String assigned_ipv6;
        public final String name;
        public final String p2p_listen_addr;
        public final Long peer_count;
        public final List<NodeNetworkRole> roles;
        public final NodeNetworkStatus status;
        public final NodeTrafficSnapshot traffic;
        public final String tun_name;
        public final Long uptime_secs;

        public NodeNetworkSnapshot(String assigned_ipv4, String assigned_ipv6, String name, String p2p_listen_addr, Long peer_count, List<NodeNetworkRole> roles, NodeNetworkStatus status, NodeTrafficSnapshot traffic, String tun_name, Long uptime_secs) {
            this.assigned_ipv4 = assigned_ipv4;
            this.assigned_ipv6 = assigned_ipv6;
            this.name = name;
            this.p2p_listen_addr = p2p_listen_addr;
            this.peer_count = peer_count;
            this.roles = roles;
            this.status = status;
            this.traffic = traffic;
            this.tun_name = tun_name;
            this.uptime_secs = uptime_secs;
        }
    }

    /** DTO for NodeOperationSnapshot. */
    public static final class NodeOperationSnapshot {
        public final DomainActorRef actor;
        public final Long attempt;
        public final String cancel_blocked_reason;
        public final Boolean cancellable;
        public final Long created_at_unix_ms;
        public final OperationError error;
        public final String idempotency_key;
        public final OperationKind kind;
        public final Long max_attempts;
        public final String operation_id;
        public final ProjectedNodeCommandResult result;
        public final String stage;
        public final OperationState state;
        public final DomainObjectRef target;
        public final Long updated_at_unix_ms;

        public NodeOperationSnapshot(DomainActorRef actor, Long attempt, String cancel_blocked_reason, Boolean cancellable, Long created_at_unix_ms, OperationError error, String idempotency_key, OperationKind kind, Long max_attempts, String operation_id, ProjectedNodeCommandResult result, String stage, OperationState state, DomainObjectRef target, Long updated_at_unix_ms) {
            this.actor = actor;
            this.attempt = attempt;
            this.cancel_blocked_reason = cancel_blocked_reason;
            this.cancellable = cancellable;
            this.created_at_unix_ms = created_at_unix_ms;
            this.error = error;
            this.idempotency_key = idempotency_key;
            this.kind = kind;
            this.max_attempts = max_attempts;
            this.operation_id = operation_id;
            this.result = result;
            this.stage = stage;
            this.state = state;
            this.target = target;
            this.updated_at_unix_ms = updated_at_unix_ms;
        }
    }

    /** DTO for NodePeerProbeSnapshot. */
    public static final class NodePeerProbeSnapshot {
        public final String endpoint;
        public final Boolean p2p_active;
        public final String peer_id;
        public final Long rtt_us;
        public final Boolean timed_out;
        public final String virtual_ipv6;

        public NodePeerProbeSnapshot(String endpoint, Boolean p2p_active, String peer_id, Long rtt_us, Boolean timed_out, String virtual_ipv6) {
            this.endpoint = endpoint;
            this.p2p_active = p2p_active;
            this.peer_id = peer_id;
            this.rtt_us = rtt_us;
            this.timed_out = timed_out;
            this.virtual_ipv6 = virtual_ipv6;
        }
    }

    /** DTO for NodePeerSnapshot. */
    public static final class NodePeerSnapshot {
        public final List<String> endpoints;
        public final Double loss_rate;
        public final String network_name;
        public final Boolean online;
        public final Boolean p2p_active;
        public final Boolean p2p_healthy;
        public final String peer_id;
        public final Long rtt_us;
        public final NodeTrafficSnapshot traffic;
        public final String virtual_ipv4;
        public final String virtual_ipv6;

        public NodePeerSnapshot(List<String> endpoints, Double loss_rate, String network_name, Boolean online, Boolean p2p_active, Boolean p2p_healthy, String peer_id, Long rtt_us, NodeTrafficSnapshot traffic, String virtual_ipv4, String virtual_ipv6) {
            this.endpoints = endpoints;
            this.loss_rate = loss_rate;
            this.network_name = network_name;
            this.online = online;
            this.p2p_active = p2p_active;
            this.p2p_healthy = p2p_healthy;
            this.peer_id = peer_id;
            this.rtt_us = rtt_us;
            this.traffic = traffic;
            this.virtual_ipv4 = virtual_ipv4;
            this.virtual_ipv6 = virtual_ipv6;
        }
    }

    /** DTO for NodePermissionSnapshot. */
    public static final class NodePermissionSnapshot {
        public final NodeAccessModel access_model;
        public final List<NodePermission> permissions;

        public NodePermissionSnapshot(NodeAccessModel access_model, List<NodePermission> permissions) {
            this.access_model = access_model;
            this.permissions = permissions;
        }
    }

    /** DTO for NodePreviewConfigurationCommand. */
    public static final class NodePreviewConfigurationCommand {
        public final Long expected_revision;
        public final Map<String, Object> networks;

        public NodePreviewConfigurationCommand(Long expected_revision, Map<String, Object> networks) {
            this.expected_revision = expected_revision;
            this.networks = networks;
        }
    }

    /** DTO for NodePreviewInvitationCommand. */
    public static final class NodePreviewInvitationCommand {
        public final String code;
        public final String coordinator_url;
        public final String network_id;

        public NodePreviewInvitationCommand(String code, String coordinator_url, String network_id) {
            this.code = code;
            this.coordinator_url = coordinator_url;
            this.network_id = network_id;
        }
    }

    /** DTO for NodePreviewJoinNetworkCommand. */
    public static final class NodePreviewJoinNetworkCommand {
        public final String network_id;

        public NodePreviewJoinNetworkCommand(String network_id) {
            this.network_id = network_id;
        }
    }

    /** DTO for NodePreviewPolicyCommand. */
    public static final class NodePreviewPolicyCommand {
        public final PolicySnapshot candidate;
        public final Long expected_revision;

        public NodePreviewPolicyCommand(PolicySnapshot candidate, Long expected_revision) {
            this.candidate = candidate;
            this.expected_revision = expected_revision;
        }
    }

    /** DTO for NodePreviewPolicyRollbackCommand. */
    public static final class NodePreviewPolicyRollbackCommand {
        public final Long expected_revision;
        public final Long target_revision;

        public NodePreviewPolicyRollbackCommand(Long expected_revision, Long target_revision) {
            this.expected_revision = expected_revision;
            this.target_revision = target_revision;
        }
    }

    /** DTO for NodePreviewTrafficPolicyCommand. */
    public static final class NodePreviewTrafficPolicyCommand {
        public final TrafficPolicyDraft draft;
        public final Long expected_revision;

        public NodePreviewTrafficPolicyCommand(TrafficPolicyDraft draft, Long expected_revision) {
            this.draft = draft;
            this.expected_revision = expected_revision;
        }
    }

    /** DTO for NodePreviewTrafficPolicyRollbackCommand. */
    public static final class NodePreviewTrafficPolicyRollbackCommand {
        public final Long expected_revision;
        public final Long target_revision;

        public NodePreviewTrafficPolicyRollbackCommand(Long expected_revision, Long target_revision) {
            this.expected_revision = expected_revision;
            this.target_revision = target_revision;
        }
    }

    /** DTO for NodeProbePeerCommand. */
    public static final class NodeProbePeerCommand {
        public final String network_name;
        public final String target;
        public final Long timeout_ms;

        public NodeProbePeerCommand(String network_name, String target, Long timeout_ms) {
            this.network_name = network_name;
            this.target = target;
            this.timeout_ms = timeout_ms;
        }
    }

    /** DTO for NodeProblem. */
    public static final class NodeProblem {
        public final NodeErrorCategory category;
        public final String code;
        public final NodeProblemDetails details;
        public final String message;
        public final String message_key;
        public final String network_name;
        public final Map<String, Object> params;
        public final OperationRecovery recovery;
        public final Boolean retryable;

        public NodeProblem(NodeErrorCategory category, String code, NodeProblemDetails details, String message, String message_key, String network_name, Map<String, Object> params, OperationRecovery recovery, Boolean retryable) {
            this.category = category;
            this.code = code;
            this.details = details;
            this.message = message;
            this.message_key = message_key;
            this.network_name = network_name;
            this.params = params;
            this.recovery = recovery;
            this.retryable = retryable;
        }
    }

    /** DTO for NodeProblemDetails. */
    public static final class NodeProblemDetails {
        public final Long current_revision;
        public final Long expected_revision;
        public final List<NodeValidationIssue> validation_issues;

        public NodeProblemDetails(Long current_revision, Long expected_revision, List<NodeValidationIssue> validation_issues) {
            this.current_revision = current_revision;
            this.expected_revision = expected_revision;
            this.validation_issues = validation_issues;
        }
    }

    /** DTO for NodeRedeemInvitationCommand. */
    public static final class NodeRedeemInvitationCommand {
        public final String coordinator_url;
        public final InvitationRedeemRequest request;

        public NodeRedeemInvitationCommand(String coordinator_url, InvitationRedeemRequest request) {
            this.coordinator_url = coordinator_url;
            this.request = request;
        }
    }

    /** DTO for NodeResidualCleanupResult. */
    public static final class NodeResidualCleanupResult {
        public final List<String> cleaned_networks;
        public final List<NodeCleanupFailureSnapshot> failed_networks;
        public final NodeRuntimeResidueSnapshot residue;

        public NodeResidualCleanupResult(List<String> cleaned_networks, List<NodeCleanupFailureSnapshot> failed_networks, NodeRuntimeResidueSnapshot residue) {
            this.cleaned_networks = cleaned_networks;
            this.failed_networks = failed_networks;
            this.residue = residue;
        }
    }

    /** DTO for NodeResubmitJoinRequestCommand. */
    public static final class NodeResubmitJoinRequestCommand {
        public final Long expected_revision;
        public final String network_id;
        public final String reason;
        public final String request_id;

        public NodeResubmitJoinRequestCommand(Long expected_revision, String network_id, String reason, String request_id) {
            this.expected_revision = expected_revision;
            this.network_id = network_id;
            this.reason = reason;
            this.request_id = request_id;
        }
    }

    /** DTO for NodeReviewJoinRequestCommand. */
    public static final class NodeReviewJoinRequestCommand {
        public final String network_id;
        public final NodeJoinRequestReviewItem review;

        public NodeReviewJoinRequestCommand(String network_id, NodeJoinRequestReviewItem review) {
            this.network_id = network_id;
            this.review = review;
        }
    }

    /** DTO for NodeRevokeInvitationCommand. */
    public static final class NodeRevokeInvitationCommand {
        public final Long expected_revision;
        public final String invitation_id;
        public final String network_id;

        public NodeRevokeInvitationCommand(Long expected_revision, String invitation_id, String network_id) {
            this.expected_revision = expected_revision;
            this.invitation_id = invitation_id;
            this.network_id = network_id;
        }
    }

    /** DTO for NodeRollbackConfigurationCommand. */
    public static final class NodeRollbackConfigurationCommand {
        public final Long expected_revision;
        public final Long target_revision;

        public NodeRollbackConfigurationCommand(Long expected_revision, Long target_revision) {
            this.expected_revision = expected_revision;
            this.target_revision = target_revision;
        }
    }

    /** DTO for NodeRollbackPolicyCommand. */
    public static final class NodeRollbackPolicyCommand {
        public final Boolean confirm_privilege_expansion;
        public final Long expected_revision;
        public final Boolean includes_local_policy;
        public final List<String> network_ids;
        public final Long target_revision;

        public NodeRollbackPolicyCommand(Boolean confirm_privilege_expansion, Long expected_revision, Boolean includes_local_policy, List<String> network_ids, Long target_revision) {
            this.confirm_privilege_expansion = confirm_privilege_expansion;
            this.expected_revision = expected_revision;
            this.includes_local_policy = includes_local_policy;
            this.network_ids = network_ids;
            this.target_revision = target_revision;
        }
    }

    /** DTO for NodeRollbackTrafficPolicyCommand. */
    public static final class NodeRollbackTrafficPolicyCommand {
        public final Boolean confirm_risk;
        public final Long expected_revision;
        public final Long target_revision;

        public NodeRollbackTrafficPolicyCommand(Boolean confirm_risk, Long expected_revision, Long target_revision) {
            this.confirm_risk = confirm_risk;
            this.expected_revision = expected_revision;
            this.target_revision = target_revision;
        }
    }

    /** DTO for NodeRotateNodeIdentityCommand. */
    public static final class NodeRotateNodeIdentityCommand {
        public final String expected_fingerprint;

        public NodeRotateNodeIdentityCommand(String expected_fingerprint) {
            this.expected_fingerprint = expected_fingerprint;
        }
    }

    /** DTO for NodeRoutingSummarySnapshot. */
    public static final class NodeRoutingSummarySnapshot {
        public final NodeRoutingDefaultAction default_action;
        public final NodeRoutingTargetSnapshot effective_target;
        public final Long enabled_rule_count;
        public final Long last_applied_at_unix_ms;
        public final Long last_applied_revision;
        public final String network_name;
        public final NodeProblem problem;
        public final String status;

        public NodeRoutingSummarySnapshot(NodeRoutingDefaultAction default_action, NodeRoutingTargetSnapshot effective_target, Long enabled_rule_count, Long last_applied_at_unix_ms, Long last_applied_revision, String network_name, NodeProblem problem, String status) {
            this.default_action = default_action;
            this.effective_target = effective_target;
            this.enabled_rule_count = enabled_rule_count;
            this.last_applied_at_unix_ms = last_applied_at_unix_ms;
            this.last_applied_revision = last_applied_revision;
            this.network_name = network_name;
            this.problem = problem;
            this.status = status;
        }
    }

    /** DTO for NodeRuntimeResidueSnapshot. */
    public static final class NodeRuntimeResidueSnapshot {
        public final List<NodeCleanupStepSnapshot> cleanup_steps;
        public final Boolean configuration_retained;
        public final Long embedded_services_remaining;
        public final List<String> issues;
        public final Long listeners_remaining;
        public final Long peers_remaining;
        public final List<String> residual_networks;
        public final Long sessions_remaining;

        public NodeRuntimeResidueSnapshot(List<NodeCleanupStepSnapshot> cleanup_steps, Boolean configuration_retained, Long embedded_services_remaining, List<String> issues, Long listeners_remaining, Long peers_remaining, List<String> residual_networks, Long sessions_remaining) {
            this.cleanup_steps = cleanup_steps;
            this.configuration_retained = configuration_retained;
            this.embedded_services_remaining = embedded_services_remaining;
            this.issues = issues;
            this.listeners_remaining = listeners_remaining;
            this.peers_remaining = peers_remaining;
            this.residual_networks = residual_networks;
            this.sessions_remaining = sessions_remaining;
        }
    }

    /** DTO for NodeSetNetworkRoleCommand. */
    public static final class NodeSetNetworkRoleCommand {
        public final Boolean enabled;
        public final String network_name;
        public final NodeNetworkRole role;

        public NodeSetNetworkRoleCommand(Boolean enabled, String network_name, NodeNetworkRole role) {
            this.enabled = enabled;
            this.network_name = network_name;
            this.role = role;
        }
    }

    /** DTO for NodeSetNodeDisplayNameCommand. */
    public static final class NodeSetNodeDisplayNameCommand {
        public final String display_name;

        public NodeSetNodeDisplayNameCommand(String display_name) {
            this.display_name = display_name;
        }
    }

    /** DTO for NodeSignedNetworkProfileSnapshot. */
    public static final class NodeSignedNetworkProfileSnapshot {
        public final Long expires_at_unix_ms;
        public final Long issued_at_unix_ms;
        public final Integer schema_version;

        public NodeSignedNetworkProfileSnapshot(Long expires_at_unix_ms, Long issued_at_unix_ms, Integer schema_version) {
            this.expires_at_unix_ms = expires_at_unix_ms;
            this.issued_at_unix_ms = issued_at_unix_ms;
            this.schema_version = schema_version;
        }
    }

    /** DTO for NodeSnapshot. */
    public static final class NodeSnapshot {
        public final NodeAccessControlSnapshot access_control;
        public final NodeCapabilityRegistrySnapshot capabilities;
        public final NodeConfigurationSnapshot configuration;
        public final NodeControlHealth control_health;
        public final NodeDiscoveryIndexSnapshot discovery_index;
        public final List<NodeEmbeddedServiceSnapshot> embedded_services;
        public final ExitNodeCapabilities exit_node_capabilities;
        public final NodeIdentitySnapshot identity;
        public final List<NodeProblem> interface_problems;
        public final List<NodeInterfaceSnapshot> interfaces;
        public final NodeProblem last_error;
        public final NodeLifecycleSnapshot lifecycle;
        public final List<NodeLogEntry> logs;
        public final List<NodeNetworkMembershipSnapshot> memberships;
        public final List<NodeNetworkProfileSnapshot> network_profiles;
        public final List<NodeNetworkSnapshot> networks;
        public final List<NodePeerSnapshot> peers;
        public final NodePermissionSnapshot permissions;
        public final List<ActivityEntry> recent_activity;
        public final Long revision;
        public final List<NodeRoutingSummarySnapshot> routing_summaries;
        public final TopologySnapshot topology;
        public final TunCapabilities tun_capabilities;
        public final UiModeResolution ui_mode;

        public NodeSnapshot(NodeAccessControlSnapshot access_control, NodeCapabilityRegistrySnapshot capabilities, NodeConfigurationSnapshot configuration, NodeControlHealth control_health, NodeDiscoveryIndexSnapshot discovery_index, List<NodeEmbeddedServiceSnapshot> embedded_services, ExitNodeCapabilities exit_node_capabilities, NodeIdentitySnapshot identity, List<NodeProblem> interface_problems, List<NodeInterfaceSnapshot> interfaces, NodeProblem last_error, NodeLifecycleSnapshot lifecycle, List<NodeLogEntry> logs, List<NodeNetworkMembershipSnapshot> memberships, List<NodeNetworkProfileSnapshot> network_profiles, List<NodeNetworkSnapshot> networks, List<NodePeerSnapshot> peers, NodePermissionSnapshot permissions, List<ActivityEntry> recent_activity, Long revision, List<NodeRoutingSummarySnapshot> routing_summaries, TopologySnapshot topology, TunCapabilities tun_capabilities, UiModeResolution ui_mode) {
            this.access_control = access_control;
            this.capabilities = capabilities;
            this.configuration = configuration;
            this.control_health = control_health;
            this.discovery_index = discovery_index;
            this.embedded_services = embedded_services;
            this.exit_node_capabilities = exit_node_capabilities;
            this.identity = identity;
            this.interface_problems = interface_problems;
            this.interfaces = interfaces;
            this.last_error = last_error;
            this.lifecycle = lifecycle;
            this.logs = logs;
            this.memberships = memberships;
            this.network_profiles = network_profiles;
            this.networks = networks;
            this.peers = peers;
            this.permissions = permissions;
            this.recent_activity = recent_activity;
            this.revision = revision;
            this.routing_summaries = routing_summaries;
            this.topology = topology;
            this.tun_capabilities = tun_capabilities;
            this.ui_mode = ui_mode;
        }
    }

    /** DTO for NodeSnapshotResponse. */
    public static final class NodeSnapshotResponse {
        public final Integer schema_version;
        public final ProjectedNodeSnapshot snapshot;

        public NodeSnapshotResponse(Integer schema_version, ProjectedNodeSnapshot snapshot) {
            this.schema_version = schema_version;
            this.snapshot = snapshot;
        }
    }

    /** DTO for NodeSubmitJoinRequestCommand. */
    public static final class NodeSubmitJoinRequestCommand {
        public final String network_id;
        public final String reason;

        public NodeSubmitJoinRequestCommand(String network_id, String reason) {
            this.network_id = network_id;
            this.reason = reason;
        }
    }

    /** DTO for NodeTestTrafficPolicyCommand. */
    public static final class NodeTestTrafficPolicyCommand {
        public final TrafficPolicy candidate;
        public final TrafficPolicy current;
        public final TrafficPolicyTestInput input;

        public NodeTestTrafficPolicyCommand(TrafficPolicy candidate, TrafficPolicy current, TrafficPolicyTestInput input) {
            this.candidate = candidate;
            this.current = current;
            this.input = input;
        }
    }

    /** DTO for NodeTrafficSnapshot. */
    public static final class NodeTrafficSnapshot {
        public final Long error_count;
        public final Long p2p_rx_bytes;
        public final Long p2p_rx_packets;
        public final Long p2p_tx_bytes;
        public final Long p2p_tx_packets;
        public final Long tun_rx_bytes;
        public final Long tun_rx_packets;
        public final Long tun_tx_bytes;
        public final Long tun_tx_packets;

        public NodeTrafficSnapshot(Long error_count, Long p2p_rx_bytes, Long p2p_rx_packets, Long p2p_tx_bytes, Long p2p_tx_packets, Long tun_rx_bytes, Long tun_rx_packets, Long tun_tx_bytes, Long tun_tx_packets) {
            this.error_count = error_count;
            this.p2p_rx_bytes = p2p_rx_bytes;
            this.p2p_rx_packets = p2p_rx_packets;
            this.p2p_tx_bytes = p2p_tx_bytes;
            this.p2p_tx_packets = p2p_tx_packets;
            this.tun_rx_bytes = tun_rx_bytes;
            this.tun_rx_packets = tun_rx_packets;
            this.tun_tx_bytes = tun_tx_bytes;
            this.tun_tx_packets = tun_tx_packets;
        }
    }

    /** DTO for NodeUiAction. */
    public static final class NodeUiAction {
        public final NodeUiAudit audit;
        public final NodeUiAvailability availability;
        public final String backend_status;
        public final String command;
        public final NodeUiConfirmation confirmation;
        public final List<NodeUiField> fields;
        public final String id;
        public final String label_key;
        public final NodePermission permission;

        public NodeUiAction(NodeUiAudit audit, NodeUiAvailability availability, String backend_status, String command, NodeUiConfirmation confirmation, List<NodeUiField> fields, String id, String label_key, NodePermission permission) {
            this.audit = audit;
            this.availability = availability;
            this.backend_status = backend_status;
            this.command = command;
            this.confirmation = confirmation;
            this.fields = fields;
            this.id = id;
            this.label_key = label_key;
            this.permission = permission;
        }
    }

    /** DTO for NodeUiAudit. */
    public static final class NodeUiAudit {
        public final String action;
        public final NodeUiAuditPolicy policy;

        public NodeUiAudit(String action, NodeUiAuditPolicy policy) {
            this.action = action;
            this.policy = policy;
        }
    }

    /** DTO for NodeUiConfirmation. */
    public static final class NodeUiConfirmation {
        public final String consequence_key;
        public final Boolean destructive;
        public final Boolean recent_authentication_required;
        public final String title_key;

        public NodeUiConfirmation(String consequence_key, Boolean destructive, Boolean recent_authentication_required, String title_key) {
            this.consequence_key = consequence_key;
            this.destructive = destructive;
            this.recent_authentication_required = recent_authentication_required;
            this.title_key = title_key;
        }
    }

    /** DTO for NodeUiField. */
    public static final class NodeUiField {
        public final String id;
        public final NodeUiInputKind input;
        public final String label_key;
        public final Boolean required;
        public final NodeUiValidation validation;

        public NodeUiField(String id, NodeUiInputKind input, String label_key, Boolean required, NodeUiValidation validation) {
            this.id = id;
            this.input = input;
            this.label_key = label_key;
            this.required = required;
            this.validation = validation;
        }
    }

    /** DTO for NodeUiManifest. */
    public static final class NodeUiManifest {
        public final List<NodeUiActionBlockReason> action_block_reasons;
        public final List<NodeUiPageState> page_states;
        public final List<NodeUiPage> pages;
        public final Integer schema_version;

        public NodeUiManifest(List<NodeUiActionBlockReason> action_block_reasons, List<NodeUiPageState> page_states, List<NodeUiPage> pages, Integer schema_version) {
            this.action_block_reasons = action_block_reasons;
            this.page_states = page_states;
            this.pages = pages;
            this.schema_version = schema_version;
        }
    }

    /** DTO for NodeUiPage. */
    public static final class NodeUiPage {
        public final List<NodeUiAction> actions;
        public final String id;
        public final String nav_label_key;
        public final String title_key;

        public NodeUiPage(List<NodeUiAction> actions, String id, String nav_label_key, String title_key) {
            this.actions = actions;
            this.id = id;
            this.nav_label_key = nav_label_key;
            this.title_key = title_key;
        }
    }

    /** DTO for NodeUpdateNetworkAllocationCommand. */
    public static final class NodeUpdateNetworkAllocationCommand {
        public final NodeNetworkAllocation allocation;
        public final String network_name;

        public NodeUpdateNetworkAllocationCommand(NodeNetworkAllocation allocation, String network_name) {
            this.allocation = allocation;
            this.network_name = network_name;
        }
    }

    /** DTO for NodeUpdateRoutingCommand. */
    public static final class NodeUpdateRoutingCommand {
        public final String exit_node;
        public final Boolean is_exit_node;
        public final NodeRoutingMode mode;
        public final String network_name;

        public NodeUpdateRoutingCommand(String exit_node, Boolean is_exit_node, NodeRoutingMode mode, String network_name) {
            this.exit_node = exit_node;
            this.is_exit_node = is_exit_node;
            this.mode = mode;
            this.network_name = network_name;
        }
    }

    /** DTO for NodeValidationIssue. */
    public static final class NodeValidationIssue {
        public final String code;
        public final String message;
        public final String path;

        public NodeValidationIssue(String code, String message, String path) {
            this.code = code;
            this.message = message;
            this.path = path;
        }
    }

    /** DTO for NodeWithdrawJoinRequestCommand. */
    public static final class NodeWithdrawJoinRequestCommand {
        public final String network_id;
        public final String request_id;

        public NodeWithdrawJoinRequestCommand(String network_id, String request_id) {
            this.network_id = network_id;
            this.request_id = request_id;
        }
    }

    /** DTO for OperationError. */
    public static final class OperationError {
        public final String code;
        public final String message_key;
        public final DomainObjectRef object;
        public final Map<String, Object> params;
        public final List<DomainObjectRef> partial_changes;
        public final OperationRecovery recovery;
        public final Boolean retryable;

        public OperationError(String code, String message_key, DomainObjectRef object, Map<String, Object> params, List<DomainObjectRef> partial_changes, OperationRecovery recovery, Boolean retryable) {
            this.code = code;
            this.message_key = message_key;
            this.object = object;
            this.params = params;
            this.partial_changes = partial_changes;
            this.recovery = recovery;
            this.retryable = retryable;
        }
    }

    /** DTO for OperationRecovery. */
    public static final class OperationRecovery {
        public final String hint_key;
        public final String kind;

        public OperationRecovery(String hint_key, String kind) {
            this.hint_key = hint_key;
            this.kind = kind;
        }
    }

    /** DTO for PermissionRule. */
    public static final class PermissionRule {
        public final List<PolicyAction> actions;
        public final List<PermissionCondition> conditions;
        public final PermissionEffect effect;
        public final List<ResourceKind> resources;
        public final String rule_id;
        public final List<SubjectSelector> subjects;

        public PermissionRule(List<PolicyAction> actions, List<PermissionCondition> conditions, PermissionEffect effect, List<ResourceKind> resources, String rule_id, List<SubjectSelector> subjects) {
            this.actions = actions;
            this.conditions = conditions;
            this.effect = effect;
            this.resources = resources;
            this.rule_id = rule_id;
            this.subjects = subjects;
        }
    }

    /** DTO for PolicyAffectedObject. */
    public static final class PolicyAffectedObject {
        public final PolicyObjectKind kind;
        public final String network_id;
        public final String object_id;

        public PolicyAffectedObject(PolicyObjectKind kind, String network_id, String object_id) {
            this.kind = kind;
            this.network_id = network_id;
            this.object_id = object_id;
        }
    }

    /** DTO for PolicyApplyResult. */
    public static final class PolicyApplyResult {
        public final List<PolicyAffectedObject> affected_objects;
        public final PolicyHistoryEntry audit;
        public final List<PolicyDiffEntry> diffs;
        public final Boolean no_op;
        public final Long previous_revision;
        public final Long revision;
        public final PolicyRiskSummary risk;
        public final PolicySnapshot snapshot;

        public PolicyApplyResult(List<PolicyAffectedObject> affected_objects, PolicyHistoryEntry audit, List<PolicyDiffEntry> diffs, Boolean no_op, Long previous_revision, Long revision, PolicyRiskSummary risk, PolicySnapshot snapshot) {
            this.affected_objects = affected_objects;
            this.audit = audit;
            this.diffs = diffs;
            this.no_op = no_op;
            this.previous_revision = previous_revision;
            this.revision = revision;
            this.risk = risk;
            this.snapshot = snapshot;
        }
    }

    /** DTO for PolicyCurrent. */
    public static final class PolicyCurrent {
        public final Long revision;
        public final PolicySnapshot snapshot;

        public PolicyCurrent(Long revision, PolicySnapshot snapshot) {
            this.revision = revision;
            this.snapshot = snapshot;
        }
    }

    /** DTO for PolicyDiffEntry. */
    public static final class PolicyDiffEntry {
        public final Object after;
        public final Object before;
        public final PolicyChangeKind change;
        public final String path;

        public PolicyDiffEntry(Object after, Object before, PolicyChangeKind change, String path) {
            this.after = after;
            this.before = before;
            this.change = change;
            this.path = path;
        }
    }

    /** DTO for PolicyHistoryEntry. */
    public static final class PolicyHistoryEntry {
        public final PolicyChangeAction action;
        public final String actor;
        public final List<PolicyAffectedObject> affected_objects;
        public final List<PolicyDiffEntry> diffs;
        public final Long local_policy_revision;
        public final Map<String, Object> network_acl_revisions;
        public final String occurred_at;
        public final Long previous_revision;
        public final Long revision;
        public final PolicyRiskSummary risk;
        public final String source;
        public final Long target_revision;
        public final String transaction_id;

        public PolicyHistoryEntry(PolicyChangeAction action, String actor, List<PolicyAffectedObject> affected_objects, List<PolicyDiffEntry> diffs, Long local_policy_revision, Map<String, Object> network_acl_revisions, String occurred_at, Long previous_revision, Long revision, PolicyRiskSummary risk, String source, Long target_revision, String transaction_id) {
            this.action = action;
            this.actor = actor;
            this.affected_objects = affected_objects;
            this.diffs = diffs;
            this.local_policy_revision = local_policy_revision;
            this.network_acl_revisions = network_acl_revisions;
            this.occurred_at = occurred_at;
            this.previous_revision = previous_revision;
            this.revision = revision;
            this.risk = risk;
            this.source = source;
            this.target_revision = target_revision;
            this.transaction_id = transaction_id;
        }
    }

    /** DTO for PolicyHistorySnapshot. */
    public static final class PolicyHistorySnapshot {
        public final Long current_revision;
        public final List<PolicyHistoryEntry> entries;

        public PolicyHistorySnapshot(Long current_revision, List<PolicyHistoryEntry> entries) {
            this.current_revision = current_revision;
            this.entries = entries;
        }
    }

    /** DTO for PolicyPrecedenceExplanation. */
    public static final class PolicyPrecedenceExplanation {
        public final PolicyPrecedence precedence;
        public final String rule_id;
        public final PolicyRuleSource source;
        public final Boolean triggered;

        public PolicyPrecedenceExplanation(PolicyPrecedence precedence, String rule_id, PolicyRuleSource source, Boolean triggered) {
            this.precedence = precedence;
            this.rule_id = rule_id;
            this.source = source;
            this.triggered = triggered;
        }
    }

    /** DTO for PolicyPreview. */
    public static final class PolicyPreview {
        public final List<PolicyAffectedObject> affected_objects;
        public final Boolean can_apply;
        public final PolicySnapshot candidate;
        public final PolicySnapshot current;
        public final Long current_revision;
        public final List<PolicyDiffEntry> diffs;
        public final Long expected_revision;
        public final Boolean has_changes;
        public final Boolean includes_local_policy;
        public final List<String> network_ids;
        public final Boolean revision_conflict;
        public final PolicyRiskSummary risk;
        public final Long rollback_target_revision;

        public PolicyPreview(List<PolicyAffectedObject> affected_objects, Boolean can_apply, PolicySnapshot candidate, PolicySnapshot current, Long current_revision, List<PolicyDiffEntry> diffs, Long expected_revision, Boolean has_changes, Boolean includes_local_policy, List<String> network_ids, Boolean revision_conflict, PolicyRiskSummary risk, Long rollback_target_revision) {
            this.affected_objects = affected_objects;
            this.can_apply = can_apply;
            this.candidate = candidate;
            this.current = current;
            this.current_revision = current_revision;
            this.diffs = diffs;
            this.expected_revision = expected_revision;
            this.has_changes = has_changes;
            this.includes_local_policy = includes_local_policy;
            this.network_ids = network_ids;
            this.revision_conflict = revision_conflict;
            this.risk = risk;
            this.rollback_target_revision = rollback_target_revision;
        }
    }

    /** DTO for PolicyRiskSummary. */
    public static final class PolicyRiskSummary {
        public final PolicyRiskLevel level;
        public final Boolean privilege_expansion;
        public final List<PolicyRiskReason> reasons;
        public final Boolean requires_explicit_confirmation;
        public final Boolean requires_recent_reauthentication;

        public PolicyRiskSummary(PolicyRiskLevel level, Boolean privilege_expansion, List<PolicyRiskReason> reasons, Boolean requires_explicit_confirmation, Boolean requires_recent_reauthentication) {
            this.level = level;
            this.privilege_expansion = privilege_expansion;
            this.reasons = reasons;
            this.requires_explicit_confirmation = requires_explicit_confirmation;
            this.requires_recent_reauthentication = requires_recent_reauthentication;
        }
    }

    /** DTO for PolicyRollbackResult. */
    public static final class PolicyRollbackResult {
        public final List<PolicyAffectedObject> affected_objects;
        public final PolicyHistoryEntry audit;
        public final List<PolicyDiffEntry> diffs;
        public final Long previous_revision;
        public final Long revision;
        public final PolicyRiskSummary risk;
        public final PolicySnapshot snapshot;
        public final Long target_revision;

        public PolicyRollbackResult(List<PolicyAffectedObject> affected_objects, PolicyHistoryEntry audit, List<PolicyDiffEntry> diffs, Long previous_revision, Long revision, PolicyRiskSummary risk, PolicySnapshot snapshot, Long target_revision) {
            this.affected_objects = affected_objects;
            this.audit = audit;
            this.diffs = diffs;
            this.previous_revision = previous_revision;
            this.revision = revision;
            this.risk = risk;
            this.snapshot = snapshot;
            this.target_revision = target_revision;
        }
    }

    /** DTO for PolicyRuleExplanation. */
    public static final class PolicyRuleExplanation {
        public final List<PolicyAction> actions;
        public final Boolean applied;
        public final List<PermissionCondition> conditions;
        public final PermissionEffect effect;
        public final Boolean matched;
        public final PolicyPrecedence precedence;
        public final List<ResourceKind> resources;
        public final String rule_id;
        public final PolicyRuleSource source;
        public final String source_path;
        public final List<SubjectSelector> subjects;

        public PolicyRuleExplanation(List<PolicyAction> actions, Boolean applied, List<PermissionCondition> conditions, PermissionEffect effect, Boolean matched, PolicyPrecedence precedence, List<ResourceKind> resources, String rule_id, PolicyRuleSource source, String source_path, List<SubjectSelector> subjects) {
            this.actions = actions;
            this.applied = applied;
            this.conditions = conditions;
            this.effect = effect;
            this.matched = matched;
            this.precedence = precedence;
            this.resources = resources;
            this.rule_id = rule_id;
            this.source = source;
            this.source_path = source_path;
            this.subjects = subjects;
        }
    }

    /** DTO for PolicySnapshot. */
    public static final class PolicySnapshot {
        public final NodeLocalAccessPolicy local_policy;
        public final List<NetworkAcl> network_acls;

        public PolicySnapshot(NodeLocalAccessPolicy local_policy, List<NetworkAcl> network_acls) {
            this.local_policy = local_policy;
            this.network_acls = network_acls;
        }
    }

    /** DTO for PolicyTestInput. */
    public static final class PolicyTestInput {
        public final String actor;
        public final Boolean emergency_coordinator;
        public final List<String> groups;
        public final NodeLocalAccessPolicy local_policy;
        public final Boolean membership_active;
        public final NetworkAcl network_acl;
        public final String network_id;
        public final Long now_unix_ms;
        public final Boolean recent_reauthentication;
        public final PolicyTestRequest request;
        public final NetworkRole role;

        public PolicyTestInput(String actor, Boolean emergency_coordinator, List<String> groups, NodeLocalAccessPolicy local_policy, Boolean membership_active, NetworkAcl network_acl, String network_id, Long now_unix_ms, Boolean recent_reauthentication, PolicyTestRequest request, NetworkRole role) {
            this.actor = actor;
            this.emergency_coordinator = emergency_coordinator;
            this.groups = groups;
            this.local_policy = local_policy;
            this.membership_active = membership_active;
            this.network_acl = network_acl;
            this.network_id = network_id;
            this.now_unix_ms = now_unix_ms;
            this.recent_reauthentication = recent_reauthentication;
            this.request = request;
            this.role = role;
        }
    }

    /** DTO for PolicyTestRequest. */
    public static final class PolicyTestRequest {
        public final PolicyAction action;
        public final PermissionIntent intent;
        public final String object_id;
        public final ResourceKind resource;
        public final String resource_network_id;

        public PolicyTestRequest(PolicyAction action, PermissionIntent intent, String object_id, ResourceKind resource, String resource_network_id) {
            this.action = action;
            this.intent = intent;
            this.object_id = object_id;
            this.resource = resource;
            this.resource_network_id = resource_network_id;
        }
    }

    /** DTO for PolicyTestResult. */
    public static final class PolicyTestResult {
        public final EffectiveAccess decision;
        public final List<PolicyRuleExplanation> explanations;
        public final List<PolicyPrecedenceExplanation> precedence;
        public final List<PolicyValidationIssue> validation_issues;

        public PolicyTestResult(EffectiveAccess decision, List<PolicyRuleExplanation> explanations, List<PolicyPrecedenceExplanation> precedence, List<PolicyValidationIssue> validation_issues) {
            this.decision = decision;
            this.explanations = explanations;
            this.precedence = precedence;
            this.validation_issues = validation_issues;
        }
    }

    /** DTO for PolicyValidationIssue. */
    public static final class PolicyValidationIssue {
        public final String code;
        public final String message;
        public final String path;
        public final String related_rule_id;
        public final PolicyValidationSource source;

        public PolicyValidationIssue(String code, String message, String path, String related_rule_id, PolicyValidationSource source) {
            this.code = code;
            this.message = message;
            this.path = path;
            this.related_rule_id = related_rule_id;
            this.source = source;
        }
    }

    /** DTO for PortRange. */
    public static final class PortRange {
        public final Integer end;
        public final Integer start;

        public PortRange(Integer end, Integer start) {
            this.end = end;
            this.start = start;
        }
    }

    /** DTO for ProjectedActivityEntry. */
    public static final class ProjectedActivityEntry {
        public final String action;
        public final String actor;
        public final String actor_role;
        public final String audit_id;
        public final ActivityCategory category;
        public final Map<String, Object> details;
        public final String network_id;
        public final String object_id;
        public final String object_kind;
        public final Long occurred_at_unix_ms;
        public final String operation_id;
        public final AuditOutcome outcome;
        public final String reason;
        public final String resource;
        public final ActivitySeverity severity;
        public final String summary;

        public ProjectedActivityEntry(String action, String actor, String actor_role, String audit_id, ActivityCategory category, Map<String, Object> details, String network_id, String object_id, String object_kind, Long occurred_at_unix_ms, String operation_id, AuditOutcome outcome, String reason, String resource, ActivitySeverity severity, String summary) {
            this.action = action;
            this.actor = actor;
            this.actor_role = actor_role;
            this.audit_id = audit_id;
            this.category = category;
            this.details = details;
            this.network_id = network_id;
            this.object_id = object_id;
            this.object_kind = object_kind;
            this.occurred_at_unix_ms = occurred_at_unix_ms;
            this.operation_id = operation_id;
            this.outcome = outcome;
            this.reason = reason;
            this.resource = resource;
            this.severity = severity;
            this.summary = summary;
        }
    }

    /** DTO for ProjectedNodeCommandResult. */
    public static final class ProjectedNodeCommandResult {
        public final ProjectedNodeCommandOutcome outcome;
        public final Long request_id;

        public ProjectedNodeCommandResult(ProjectedNodeCommandOutcome outcome, Long request_id) {
            this.outcome = outcome;
            this.request_id = request_id;
        }
    }

    /** DTO for ProjectedNodeEmbeddedServiceSnapshot. */
    public static final class ProjectedNodeEmbeddedServiceSnapshot {
        public final String network_name;
        public final String relay_addr;
        public final String signaling_addr;
        public final NodeEmbeddedServiceStatus status;

        public ProjectedNodeEmbeddedServiceSnapshot(String network_name, String relay_addr, String signaling_addr, NodeEmbeddedServiceStatus status) {
            this.network_name = network_name;
            this.relay_addr = relay_addr;
            this.signaling_addr = signaling_addr;
            this.status = status;
        }
    }

    /** DTO for ProjectedNodeEvent. */
    public static final class ProjectedNodeEvent {
        public final ProjectedNodeEventKind kind;
        public final Long sequence;
        public final Long snapshot_revision;

        public ProjectedNodeEvent(ProjectedNodeEventKind kind, Long sequence, Long snapshot_revision) {
            this.kind = kind;
            this.sequence = sequence;
            this.snapshot_revision = snapshot_revision;
        }
    }

    /** DTO for ProjectedNodeIdentitySnapshot. */
    public static final class ProjectedNodeIdentitySnapshot {
        public final Long created_at_unix_ms;
        public final String display_name;
        public final String node_id;
        public final String public_key_fingerprint;

        public ProjectedNodeIdentitySnapshot(Long created_at_unix_ms, String display_name, String node_id, String public_key_fingerprint) {
            this.created_at_unix_ms = created_at_unix_ms;
            this.display_name = display_name;
            this.node_id = node_id;
            this.public_key_fingerprint = public_key_fingerprint;
        }
    }

    /** DTO for ProjectedNodeInterfaceSnapshot. */
    public static final class ProjectedNodeInterfaceSnapshot {
        public final List<NodeInterfaceAddress> addresses;
        public final Boolean default_ipv4;
        public final Boolean default_ipv6;
        public final Long index;
        public final NodeInterfaceKind kind;
        public final Long mtu;
        public final String name;
        public final NodeInterfaceOwnership ownership;
        public final NodeInterfaceStatus status;

        public ProjectedNodeInterfaceSnapshot(List<NodeInterfaceAddress> addresses, Boolean default_ipv4, Boolean default_ipv6, Long index, NodeInterfaceKind kind, Long mtu, String name, NodeInterfaceOwnership ownership, NodeInterfaceStatus status) {
            this.addresses = addresses;
            this.default_ipv4 = default_ipv4;
            this.default_ipv6 = default_ipv6;
            this.index = index;
            this.kind = kind;
            this.mtu = mtu;
            this.name = name;
            this.ownership = ownership;
            this.status = status;
        }
    }

    /** DTO for ProjectedNodeNetworkJoinResult. */
    public static final class ProjectedNodeNetworkJoinResult {
        public final Boolean config_saved;
        public final Boolean idempotent;
        public final NodeNetworkMembershipSnapshot membership;
        public final ProjectedNodeSnapshot snapshot;
        public final Boolean started;

        public ProjectedNodeNetworkJoinResult(Boolean config_saved, Boolean idempotent, NodeNetworkMembershipSnapshot membership, ProjectedNodeSnapshot snapshot, Boolean started) {
            this.config_saved = config_saved;
            this.idempotent = idempotent;
            this.membership = membership;
            this.snapshot = snapshot;
            this.started = started;
        }
    }

    /** DTO for ProjectedNodeNetworkLeaveResult. */
    public static final class ProjectedNodeNetworkLeaveResult {
        public final Boolean left;
        public final String network_name;
        public final ProjectedNodeSnapshot snapshot;

        public ProjectedNodeNetworkLeaveResult(Boolean left, String network_name, ProjectedNodeSnapshot snapshot) {
            this.left = left;
            this.network_name = network_name;
            this.snapshot = snapshot;
        }
    }

    /** DTO for ProjectedNodeNetworkSnapshot. */
    public static final class ProjectedNodeNetworkSnapshot {
        public final String assigned_ipv4;
        public final String assigned_ipv6;
        public final String name;
        public final String p2p_listen_addr;
        public final Long peer_count;
        public final List<NodeNetworkRole> roles;
        public final NodeNetworkStatus status;
        public final NodeTrafficSnapshot traffic;
        public final String tun_name;
        public final Long uptime_secs;

        public ProjectedNodeNetworkSnapshot(String assigned_ipv4, String assigned_ipv6, String name, String p2p_listen_addr, Long peer_count, List<NodeNetworkRole> roles, NodeNetworkStatus status, NodeTrafficSnapshot traffic, String tun_name, Long uptime_secs) {
            this.assigned_ipv4 = assigned_ipv4;
            this.assigned_ipv6 = assigned_ipv6;
            this.name = name;
            this.p2p_listen_addr = p2p_listen_addr;
            this.peer_count = peer_count;
            this.roles = roles;
            this.status = status;
            this.traffic = traffic;
            this.tun_name = tun_name;
            this.uptime_secs = uptime_secs;
        }
    }

    /** DTO for ProjectedNodePeerSnapshot. */
    public static final class ProjectedNodePeerSnapshot {
        public final List<String> endpoints;
        public final Double loss_rate;
        public final String network_name;
        public final Boolean online;
        public final Boolean p2p_active;
        public final Boolean p2p_healthy;
        public final String peer_id;
        public final Long rtt_us;
        public final NodeTrafficSnapshot traffic;
        public final String virtual_ipv4;
        public final String virtual_ipv6;

        public ProjectedNodePeerSnapshot(List<String> endpoints, Double loss_rate, String network_name, Boolean online, Boolean p2p_active, Boolean p2p_healthy, String peer_id, Long rtt_us, NodeTrafficSnapshot traffic, String virtual_ipv4, String virtual_ipv6) {
            this.endpoints = endpoints;
            this.loss_rate = loss_rate;
            this.network_name = network_name;
            this.online = online;
            this.p2p_active = p2p_active;
            this.p2p_healthy = p2p_healthy;
            this.peer_id = peer_id;
            this.rtt_us = rtt_us;
            this.traffic = traffic;
            this.virtual_ipv4 = virtual_ipv4;
            this.virtual_ipv6 = virtual_ipv6;
        }
    }

    /** DTO for ProjectedNodeSnapshot. */
    public static final class ProjectedNodeSnapshot {
        public final NodeAccessControlSnapshot access_control;
        public final NodeCapabilityRegistrySnapshot capabilities;
        public final NodeConfigurationSnapshot configuration;
        public final NodeControlHealth control_health;
        public final NodeDiscoveryIndexSnapshot discovery_index;
        public final List<ProjectedNodeEmbeddedServiceSnapshot> embedded_services;
        public final ExitNodeCapabilities exit_node_capabilities;
        public final ProjectedNodeIdentitySnapshot identity;
        public final List<NodeProblem> interface_problems;
        public final List<ProjectedNodeInterfaceSnapshot> interfaces;
        public final NodeProblem last_error;
        public final NodeLifecycleSnapshot lifecycle;
        public final List<NodeLogEntry> logs;
        public final List<NodeNetworkMembershipSnapshot> memberships;
        public final List<NodeNetworkProfileSnapshot> network_profiles;
        public final List<ProjectedNodeNetworkSnapshot> networks;
        public final List<ProjectedNodePeerSnapshot> peers;
        public final NodePermissionSnapshot permissions;
        public final List<ProjectedActivityEntry> recent_activity;
        public final Long revision;
        public final List<NodeRoutingSummarySnapshot> routing_summaries;
        public final TopologySnapshot topology;
        public final TunCapabilities tun_capabilities;
        public final UiModeResolution ui_mode;

        public ProjectedNodeSnapshot(NodeAccessControlSnapshot access_control, NodeCapabilityRegistrySnapshot capabilities, NodeConfigurationSnapshot configuration, NodeControlHealth control_health, NodeDiscoveryIndexSnapshot discovery_index, List<ProjectedNodeEmbeddedServiceSnapshot> embedded_services, ExitNodeCapabilities exit_node_capabilities, ProjectedNodeIdentitySnapshot identity, List<NodeProblem> interface_problems, List<ProjectedNodeInterfaceSnapshot> interfaces, NodeProblem last_error, NodeLifecycleSnapshot lifecycle, List<NodeLogEntry> logs, List<NodeNetworkMembershipSnapshot> memberships, List<NodeNetworkProfileSnapshot> network_profiles, List<ProjectedNodeNetworkSnapshot> networks, List<ProjectedNodePeerSnapshot> peers, NodePermissionSnapshot permissions, List<ProjectedActivityEntry> recent_activity, Long revision, List<NodeRoutingSummarySnapshot> routing_summaries, TopologySnapshot topology, TunCapabilities tun_capabilities, UiModeResolution ui_mode) {
            this.access_control = access_control;
            this.capabilities = capabilities;
            this.configuration = configuration;
            this.control_health = control_health;
            this.discovery_index = discovery_index;
            this.embedded_services = embedded_services;
            this.exit_node_capabilities = exit_node_capabilities;
            this.identity = identity;
            this.interface_problems = interface_problems;
            this.interfaces = interfaces;
            this.last_error = last_error;
            this.lifecycle = lifecycle;
            this.logs = logs;
            this.memberships = memberships;
            this.network_profiles = network_profiles;
            this.networks = networks;
            this.peers = peers;
            this.permissions = permissions;
            this.recent_activity = recent_activity;
            this.revision = revision;
            this.routing_summaries = routing_summaries;
            this.topology = topology;
            this.tun_capabilities = tun_capabilities;
            this.ui_mode = ui_mode;
        }
    }

    /** DTO for TopologyBoundary. */
    public static final class TopologyBoundary {
        public final List<CapabilityKind> capabilities;
        public final Long member_count;
        public final String name;
        public final String network_id;
        public final MembershipRole viewer_role;
        public final Long visible_member_count;

        public TopologyBoundary(List<CapabilityKind> capabilities, Long member_count, String name, String network_id, MembershipRole viewer_role, Long visible_member_count) {
            this.capabilities = capabilities;
            this.member_count = member_count;
            this.name = name;
            this.network_id = network_id;
            this.viewer_role = viewer_role;
            this.visible_member_count = visible_member_count;
        }
    }

    /** DTO for TopologyCluster. */
    public static final class TopologyCluster {
        public final Boolean expandable;
        public final Long hidden_member_count;
        public final String network_id;
        public final List<String> sample;

        public TopologyCluster(Boolean expandable, Long hidden_member_count, String network_id, List<String> sample) {
            this.expandable = expandable;
            this.hidden_member_count = hidden_member_count;
            this.network_id = network_id;
            this.sample = sample;
        }
    }

    /** DTO for TopologyConnection. */
    public static final class TopologyConnection {
        public final String connection_id;
        public final String from;
        public final List<String> networks;
        public final Long observed_at_unix_ms;
        public final TopologyConnectionState state;
        public final String to;

        public TopologyConnection(String connection_id, String from, List<String> networks, Long observed_at_unix_ms, TopologyConnectionState state, String to) {
            this.connection_id = connection_id;
            this.from = from;
            this.networks = networks;
            this.observed_at_unix_ms = observed_at_unix_ms;
            this.state = state;
            this.to = to;
        }
    }

    /** DTO for TopologyNode. */
    public static final class TopologyNode {
        public final List<CapabilityKind> capabilities;
        public final String display_name;
        public final TopologyNodeHealth health;
        public final Boolean is_self;
        public final List<TopologyNodeNetwork> networks;
        public final String node_id;

        public TopologyNode(List<CapabilityKind> capabilities, String display_name, TopologyNodeHealth health, Boolean is_self, List<TopologyNodeNetwork> networks, String node_id) {
            this.capabilities = capabilities;
            this.display_name = display_name;
            this.health = health;
            this.is_self = is_self;
            this.networks = networks;
            this.node_id = node_id;
        }
    }

    /** DTO for TopologyNodeNetwork. */
    public static final class TopologyNodeNetwork {
        public final List<CapabilityKind> capabilities;
        public final TopologyNodeHealth health;
        public final String network_id;
        public final NetworkMemberPresence presence;
        public final MembershipRole role;
        public final NetworkMemberStatus status;

        public TopologyNodeNetwork(List<CapabilityKind> capabilities, TopologyNodeHealth health, String network_id, NetworkMemberPresence presence, MembershipRole role, NetworkMemberStatus status) {
            this.capabilities = capabilities;
            this.health = health;
            this.network_id = network_id;
            this.presence = presence;
            this.role = role;
            this.status = status;
        }
    }

    /** DTO for TopologySnapshot. */
    public static final class TopologySnapshot {
        public final List<TopologyBoundary> boundaries;
        public final List<TopologyCluster> clusters;
        public final List<TopologyConnection> connections;
        public final Long generated_at_unix_ms;
        public final List<TopologyNode> nodes;
        public final Long revision;
        public final Boolean truncated;
        public final String viewer;

        public TopologySnapshot(List<TopologyBoundary> boundaries, List<TopologyCluster> clusters, List<TopologyConnection> connections, Long generated_at_unix_ms, List<TopologyNode> nodes, Long revision, Boolean truncated, String viewer) {
            this.boundaries = boundaries;
            this.clusters = clusters;
            this.connections = connections;
            this.generated_at_unix_ms = generated_at_unix_ms;
            this.nodes = nodes;
            this.revision = revision;
            this.truncated = truncated;
            this.viewer = viewer;
        }
    }

    /** DTO for TrafficDefaultPolicy. */
    public static final class TrafficDefaultPolicy {
        public final TrafficAction action;
        public final Long confirmed_at_unix_ms;
        public final TrafficDefaultConfirmedBy confirmed_by;

        public TrafficDefaultPolicy(TrafficAction action, Long confirmed_at_unix_ms, TrafficDefaultConfirmedBy confirmed_by) {
            this.action = action;
            this.confirmed_at_unix_ms = confirmed_at_unix_ms;
            this.confirmed_by = confirmed_by;
        }
    }

    /** DTO for TrafficFailureDecision. */
    public static final class TrafficFailureDecision {
        public final String consequence_key;
        public final ConfirmedTrafficDirectFallback direct_fallback;
        public final String failure;
        public final TrafficFailureOutcome outcome;
        public final TrafficFailureDecisionReason reason;
        public final TrafficFailureRecoveryAction recovery_action;

        public TrafficFailureDecision(String consequence_key, ConfirmedTrafficDirectFallback direct_fallback, String failure, TrafficFailureOutcome outcome, TrafficFailureDecisionReason reason, TrafficFailureRecoveryAction recovery_action) {
            this.consequence_key = consequence_key;
            this.direct_fallback = direct_fallback;
            this.failure = failure;
            this.outcome = outcome;
            this.reason = reason;
            this.recovery_action = recovery_action;
        }
    }

    /** DTO for TrafficFailurePolicy. */
    public static final class TrafficFailurePolicy {
        public final ConfirmedTrafficDirectFallback direct_fallback;

        public TrafficFailurePolicy(ConfirmedTrafficDirectFallback direct_fallback) {
            this.direct_fallback = direct_fallback;
        }
    }

    /** DTO for TrafficFlow. */
    public static final class TrafficFlow {
        public final ApplicationIdentity application;
        public final String destination_ip;
        public final Integer destination_port;
        public final String domain;
        public final TrafficProtocol protocol;

        public TrafficFlow(ApplicationIdentity application, String destination_ip, Integer destination_port, String domain, TrafficProtocol protocol) {
            this.application = application;
            this.destination_ip = destination_ip;
            this.destination_port = destination_port;
            this.domain = domain;
            this.protocol = protocol;
        }
    }

    /** DTO for TrafficMatch. */
    public static final class TrafficMatch {
        public final List<ApplicationIdentity> applications;
        public final List<String> destination_cidrs;
        public final List<PortRange> destination_ports;
        public final List<String> domain_suffixes;
        public final List<String> exact_domains;
        public final List<TrafficProtocol> protocols;

        public TrafficMatch(List<ApplicationIdentity> applications, List<String> destination_cidrs, List<PortRange> destination_ports, List<String> domain_suffixes, List<String> exact_domains, List<TrafficProtocol> protocols) {
            this.applications = applications;
            this.destination_cidrs = destination_cidrs;
            this.destination_ports = destination_ports;
            this.domain_suffixes = domain_suffixes;
            this.exact_domains = exact_domains;
            this.protocols = protocols;
        }
    }

    /** DTO for TrafficPolicy. */
    public static final class TrafficPolicy {
        public final TrafficDefaultPolicy default_policy;
        public final TrafficFailurePolicy failure_policy;
        public final String node_id;
        public final Long revision;
        public final List<TrafficRule> rules;

        public TrafficPolicy(TrafficDefaultPolicy default_policy, TrafficFailurePolicy failure_policy, String node_id, Long revision, List<TrafficRule> rules) {
            this.default_policy = default_policy;
            this.failure_policy = failure_policy;
            this.node_id = node_id;
            this.revision = revision;
            this.rules = rules;
        }
    }

    /** DTO for TrafficPolicyAffectedObject. */
    public static final class TrafficPolicyAffectedObject {
        public final TrafficPolicyObjectKind kind;
        public final String object_id;

        public TrafficPolicyAffectedObject(TrafficPolicyObjectKind kind, String object_id) {
            this.kind = kind;
            this.object_id = object_id;
        }
    }

    /** DTO for TrafficPolicyApplyResult. */
    public static final class TrafficPolicyApplyResult {
        public final List<TrafficPolicyAffectedObject> affected_objects;
        public final String content_hash;
        public final List<TrafficPolicyDiffEntry> diffs;
        public final TrafficPolicyHistoryEntry history;
        public final Boolean no_op;
        public final TrafficPolicy policy;
        public final Long previous_revision;
        public final Long revision;

        public TrafficPolicyApplyResult(List<TrafficPolicyAffectedObject> affected_objects, String content_hash, List<TrafficPolicyDiffEntry> diffs, TrafficPolicyHistoryEntry history, Boolean no_op, TrafficPolicy policy, Long previous_revision, Long revision) {
            this.affected_objects = affected_objects;
            this.content_hash = content_hash;
            this.diffs = diffs;
            this.history = history;
            this.no_op = no_op;
            this.policy = policy;
            this.previous_revision = previous_revision;
            this.revision = revision;
        }
    }

    /** DTO for TrafficPolicyCurrent. */
    public static final class TrafficPolicyCurrent {
        public final String content_hash;
        public final TrafficPolicy policy;
        public final Long revision;

        public TrafficPolicyCurrent(String content_hash, TrafficPolicy policy, Long revision) {
            this.content_hash = content_hash;
            this.policy = policy;
            this.revision = revision;
        }
    }

    /** DTO for TrafficPolicyDiffEntry. */
    public static final class TrafficPolicyDiffEntry {
        public final Object after;
        public final Object before;
        public final TrafficPolicyChangeKind change;
        public final String path;

        public TrafficPolicyDiffEntry(Object after, Object before, TrafficPolicyChangeKind change, String path) {
            this.after = after;
            this.before = before;
            this.change = change;
            this.path = path;
        }
    }

    /** DTO for TrafficPolicyDraft. */
    public static final class TrafficPolicyDraft {
        public final TrafficDefaultPolicy default_policy;
        public final TrafficFailurePolicy failure_policy;
        public final String node_id;
        public final List<TrafficRule> rules;

        public TrafficPolicyDraft(TrafficDefaultPolicy default_policy, TrafficFailurePolicy failure_policy, String node_id, List<TrafficRule> rules) {
            this.default_policy = default_policy;
            this.failure_policy = failure_policy;
            this.node_id = node_id;
            this.rules = rules;
        }
    }

    /** DTO for TrafficPolicyHistoryEntry. */
    public static final class TrafficPolicyHistoryEntry {
        public final TrafficPolicyChangeAction action;
        public final String actor;
        public final List<TrafficPolicyAffectedObject> affected_objects;
        public final Long applied_revision;
        public final Long candidate_revision;
        public final String content_hash;
        public final List<TrafficPolicyDiffEntry> diffs;
        public final String occurred_at;
        public final TrafficPolicyHistoryOutcome outcome;
        public final Long previous_revision;
        public final String reason;
        public final String reason_code;
        public final String source;
        public final Long target_revision;
        public final String transaction_id;

        public TrafficPolicyHistoryEntry(TrafficPolicyChangeAction action, String actor, List<TrafficPolicyAffectedObject> affected_objects, Long applied_revision, Long candidate_revision, String content_hash, List<TrafficPolicyDiffEntry> diffs, String occurred_at, TrafficPolicyHistoryOutcome outcome, Long previous_revision, String reason, String reason_code, String source, Long target_revision, String transaction_id) {
            this.action = action;
            this.actor = actor;
            this.affected_objects = affected_objects;
            this.applied_revision = applied_revision;
            this.candidate_revision = candidate_revision;
            this.content_hash = content_hash;
            this.diffs = diffs;
            this.occurred_at = occurred_at;
            this.outcome = outcome;
            this.previous_revision = previous_revision;
            this.reason = reason;
            this.reason_code = reason_code;
            this.source = source;
            this.target_revision = target_revision;
            this.transaction_id = transaction_id;
        }
    }

    /** DTO for TrafficPolicyHistorySnapshot. */
    public static final class TrafficPolicyHistorySnapshot {
        public final Long current_revision;
        public final List<TrafficPolicyHistoryEntry> entries;

        public TrafficPolicyHistorySnapshot(Long current_revision, List<TrafficPolicyHistoryEntry> entries) {
            this.current_revision = current_revision;
            this.entries = entries;
        }
    }

    /** DTO for TrafficPolicyPreview. */
    public static final class TrafficPolicyPreview {
        public final List<TrafficPolicyAffectedObject> affected_objects;
        public final Boolean can_apply;
        public final TrafficPolicy candidate;
        public final Long candidate_revision;
        public final TrafficPolicy current;
        public final Long current_revision;
        public final List<TrafficPolicyDiffEntry> diffs;
        public final Long expected_revision;
        public final Boolean has_changes;
        public final Boolean revision_conflict;
        public final List<TrafficRuleRisk> risks;
        public final Long rollback_target_revision;

        public TrafficPolicyPreview(List<TrafficPolicyAffectedObject> affected_objects, Boolean can_apply, TrafficPolicy candidate, Long candidate_revision, TrafficPolicy current, Long current_revision, List<TrafficPolicyDiffEntry> diffs, Long expected_revision, Boolean has_changes, Boolean revision_conflict, List<TrafficRuleRisk> risks, Long rollback_target_revision) {
            this.affected_objects = affected_objects;
            this.can_apply = can_apply;
            this.candidate = candidate;
            this.candidate_revision = candidate_revision;
            this.current = current;
            this.current_revision = current_revision;
            this.diffs = diffs;
            this.expected_revision = expected_revision;
            this.has_changes = has_changes;
            this.revision_conflict = revision_conflict;
            this.risks = risks;
            this.rollback_target_revision = rollback_target_revision;
        }
    }

    /** DTO for TrafficPolicyRollbackResult. */
    public static final class TrafficPolicyRollbackResult {
        public final List<TrafficPolicyAffectedObject> affected_objects;
        public final String content_hash;
        public final List<TrafficPolicyDiffEntry> diffs;
        public final TrafficPolicyHistoryEntry history;
        public final TrafficPolicy policy;
        public final Long previous_revision;
        public final Long revision;
        public final Long target_revision;

        public TrafficPolicyRollbackResult(List<TrafficPolicyAffectedObject> affected_objects, String content_hash, List<TrafficPolicyDiffEntry> diffs, TrafficPolicyHistoryEntry history, TrafficPolicy policy, Long previous_revision, Long revision, Long target_revision) {
            this.affected_objects = affected_objects;
            this.content_hash = content_hash;
            this.diffs = diffs;
            this.history = history;
            this.policy = policy;
            this.previous_revision = previous_revision;
            this.revision = revision;
            this.target_revision = target_revision;
        }
    }

    /** DTO for TrafficPolicyTestComparison. */
    public static final class TrafficPolicyTestComparison {
        public final TrafficPolicyTestDecision after;
        public final TrafficPolicyTestDecision before;
        public final TrafficPolicyTestDelta delta;
        public final String network_id;
        public final TrafficPolicyTestContextSource source;

        public TrafficPolicyTestComparison(TrafficPolicyTestDecision after, TrafficPolicyTestDecision before, TrafficPolicyTestDelta delta, String network_id, TrafficPolicyTestContextSource source) {
            this.after = after;
            this.before = before;
            this.delta = delta;
            this.network_id = network_id;
            this.source = source;
        }
    }

    /** DTO for TrafficPolicyTestDelta. */
    public static final class TrafficPolicyTestDelta {
        public final Boolean changed;
        public final Boolean failure_outcomes_changed;
        public final Boolean matched_rule_changed;
        public final Boolean route_changed;

        public TrafficPolicyTestDelta(Boolean changed, Boolean failure_outcomes_changed, Boolean matched_rule_changed, Boolean route_changed) {
            this.changed = changed;
            this.failure_outcomes_changed = failure_outcomes_changed;
            this.matched_rule_changed = matched_rule_changed;
            this.route_changed = route_changed;
        }
    }

    /** DTO for TrafficPolicyTestInput. */
    public static final class TrafficPolicyTestInput {
        public final String fixed_exit;
        public final String fixed_network;
        public final TrafficFlow flow;
        public final String network_id;
        public final TrafficPolicyTestContextSource source;

        public TrafficPolicyTestInput(String fixed_exit, String fixed_network, TrafficFlow flow, String network_id, TrafficPolicyTestContextSource source) {
            this.fixed_exit = fixed_exit;
            this.fixed_network = fixed_network;
            this.flow = flow;
            this.network_id = network_id;
            this.source = source;
        }
    }

    /** DTO for TrafficRule. */
    public static final class TrafficRule {
        public final TrafficAction action;
        public final String description;
        public final Boolean enabled;
        public final TrafficMatch matcher;
        public final Long priority;
        public final String rule_id;

        public TrafficRule(TrafficAction action, String description, Boolean enabled, TrafficMatch matcher, Long priority, String rule_id) {
            this.action = action;
            this.description = description;
            this.enabled = enabled;
            this.matcher = matcher;
            this.priority = priority;
            this.rule_id = rule_id;
        }
    }

    /** DTO for TrafficRuleRisk. */
    public static final class TrafficRuleRisk {
        public final Boolean enabled;
        public final Long priority;
        public final List<TrafficMatchRisk> risks;
        public final String rule_id;

        public TrafficRuleRisk(Boolean enabled, Long priority, List<TrafficMatchRisk> risks, String rule_id) {
            this.enabled = enabled;
            this.priority = priority;
            this.risks = risks;
            this.rule_id = rule_id;
        }
    }

    /** DTO for TunCapabilities. */
    public static final class TunCapabilities {
        public final Boolean can_rebuild;
        public final TunPlatform platform;
        public final List<TunCapabilityReason> reasons;
        public final Integer schema_version;
        public final Boolean supported;

        public TunCapabilities(Boolean can_rebuild, TunPlatform platform, List<TunCapabilityReason> reasons, Integer schema_version, Boolean supported) {
            this.can_rebuild = can_rebuild;
            this.platform = platform;
            this.reasons = reasons;
            this.schema_version = schema_version;
            this.supported = supported;
        }
    }

    /** DTO for UiModeResolution. */
    public static final class UiModeResolution {
        public final DisplayEnvironment display;
        public final UiMode effective;
        public final Boolean gpui_available;
        public final UiModeRequest requested;

        public UiModeResolution(DisplayEnvironment display, UiMode effective, Boolean gpui_available, UiModeRequest requested) {
            this.display = display;
            this.effective = effective;
            this.gpui_available = gpui_available;
            this.requested = requested;
        }
    }

    /** Tagged union DTO for ApplicationIdentity. */
    public abstract static class ApplicationIdentity {
        public final String kind;

        private ApplicationIdentity(String kind) {
            this.kind = kind;
        }

        public static final class ExecutablePath extends ApplicationIdentity {
            public final String value;

            public ExecutablePath(String value) {
                super("executable_path");
                this.value = value;
            }
        }

        public static final class PackageId extends ApplicationIdentity {
            public final String value;

            public PackageId(String value) {
                super("package_id");
                this.value = value;
            }
        }

        public static final class Unknown extends ApplicationIdentity {
            public Unknown() {
                super("unknown");
            }
        }
    }

    /** Tagged union DTO for NodeApiEvent. */
    public abstract static class NodeApiEvent {
        public final String type;

        private NodeApiEvent(String type) {
            this.type = type;
        }

        public static final class Snapshot extends NodeApiEvent {
            public final NodeEventCursor cursor;
            public final Long request_id;
            public final Integer schema_version;
            public final String server_time;
            public final ProjectedNodeSnapshot snapshot;

            public Snapshot(NodeEventCursor cursor, Long request_id, Integer schema_version, String server_time, ProjectedNodeSnapshot snapshot) {
                super("snapshot");
                this.cursor = cursor;
                this.request_id = request_id;
                this.schema_version = schema_version;
                this.server_time = server_time;
                this.snapshot = snapshot;
            }
        }

        public static final class Event extends NodeApiEvent {
            public final ProjectedNodeEvent event;
            public final Long request_id;
            public final Integer schema_version;
            public final String server_time;

            public Event(ProjectedNodeEvent event, Long request_id, Integer schema_version, String server_time) {
                super("event");
                this.event = event;
                this.request_id = request_id;
                this.schema_version = schema_version;
                this.server_time = server_time;
            }
        }

        public static final class ResyncRequired extends NodeApiEvent {
            public final NodeEventCursor latest_cursor;
            public final NodeEventReplayReason reason;
            public final Long request_id;
            public final NodeEventCursor requested_cursor;
            public final Integer schema_version;
            public final String server_time;

            public ResyncRequired(NodeEventCursor latest_cursor, NodeEventReplayReason reason, Long request_id, NodeEventCursor requested_cursor, Integer schema_version, String server_time) {
                super("resync_required");
                this.latest_cursor = latest_cursor;
                this.reason = reason;
                this.request_id = request_id;
                this.requested_cursor = requested_cursor;
                this.schema_version = schema_version;
                this.server_time = server_time;
            }
        }
    }

    /** Tagged union DTO for NodeApiObject. */
    public abstract static class NodeApiObject {
        public final String kind;

        private NodeApiObject(String kind) {
            this.kind = kind;
        }

        public static final class Node extends NodeApiObject {
            public final String id;
            public final ProjectedNodeIdentitySnapshot identity;

            public Node(String id, ProjectedNodeIdentitySnapshot identity) {
                super("node");
                this.id = id;
                this.identity = identity;
            }
        }

        public static final class Network extends NodeApiObject {
            public final String id;
            public final ProjectedNodeNetworkSnapshot network;

            public Network(String id, ProjectedNodeNetworkSnapshot network) {
                super("network");
                this.id = id;
                this.network = network;
            }
        }

        public static final class Peer extends NodeApiObject {
            public final String id;
            public final ProjectedNodePeerSnapshot peer;

            public Peer(String id, ProjectedNodePeerSnapshot peer) {
                super("peer");
                this.id = id;
                this.peer = peer;
            }
        }

        public static final class EmbeddedService extends NodeApiObject {
            public final String id;
            public final ProjectedNodeEmbeddedServiceSnapshot service;

            public EmbeddedService(String id, ProjectedNodeEmbeddedServiceSnapshot service) {
                super("embedded_service");
                this.id = id;
                this.service = service;
            }
        }

        public static final class NetworkConfiguration extends NodeApiObject {
            public final NodeNetworkConfigSnapshot configuration;
            public final String id;

            public NetworkConfiguration(NodeNetworkConfigSnapshot configuration, String id) {
                super("network_configuration");
                this.configuration = configuration;
                this.id = id;
            }
        }
    }

    /** Tagged union DTO for NodeCommand. */
    public abstract static class NodeCommand {
        public final NodeCommandKind command;

        private NodeCommand(NodeCommandKind command) {
            this.command = command;
        }

        public static final class GetSnapshot extends NodeCommand {
            public GetSnapshot() {
                super(NodeCommandKind.GET_SNAPSHOT);
            }
        }

        public static final class GetUiManifest extends NodeCommand {
            public GetUiManifest() {
                super(NodeCommandKind.GET_UI_MANIFEST);
            }
        }

        public static final class GetNodeDiagnostics extends NodeCommand {
            public GetNodeDiagnostics() {
                super(NodeCommandKind.GET_NODE_DIAGNOSTICS);
            }
        }

        public static final class StartNetworks extends NodeCommand {
            public final StartNetworksPayload payload;

            public StartNetworks(StartNetworksPayload payload) {
                super(NodeCommandKind.START_NETWORKS);
                this.payload = payload;
            }
            /** DTO for StartNetworksPayload. */
            public static final class StartNetworksPayload {
                public final NodeNetworkSelection selection;

                public StartNetworksPayload(NodeNetworkSelection selection) {
                    this.selection = selection;
                }
            }
        }

        public static final class StopNetworks extends NodeCommand {
            public final StopNetworksPayload payload;

            public StopNetworks(StopNetworksPayload payload) {
                super(NodeCommandKind.STOP_NETWORKS);
                this.payload = payload;
            }
            /** DTO for StopNetworksPayload. */
            public static final class StopNetworksPayload {
                public final Boolean all;
                public final List<String> networks;

                public StopNetworksPayload(Boolean all, List<String> networks) {
                    this.all = all;
                    this.networks = networks;
                }
            }
        }

        public static final class RestartNetworks extends NodeCommand {
            public final RestartNetworksPayload payload;

            public RestartNetworks(RestartNetworksPayload payload) {
                super(NodeCommandKind.RESTART_NETWORKS);
                this.payload = payload;
            }
            /** DTO for RestartNetworksPayload. */
            public static final class RestartNetworksPayload {
                public final Boolean all;
                public final List<String> networks;

                public RestartNetworksPayload(Boolean all, List<String> networks) {
                    this.all = all;
                    this.networks = networks;
                }
            }
        }

        public static final class RebuildTun extends NodeCommand {
            public final RebuildTunPayload payload;

            public RebuildTun(RebuildTunPayload payload) {
                super(NodeCommandKind.REBUILD_TUN);
                this.payload = payload;
            }
            /** DTO for RebuildTunPayload. */
            public static final class RebuildTunPayload {
                public final String network_name;

                public RebuildTunPayload(String network_name) {
                    this.network_name = network_name;
                }
            }
        }

        public static final class CreateNetwork extends NodeCommand {
            public final NodeCreateNetworkCommand payload;

            public CreateNetwork(NodeCreateNetworkCommand payload) {
                super(NodeCommandKind.CREATE_NETWORK);
                this.payload = payload;
            }
        }

        public static final class PreviewNetworkProfile extends NodeCommand {
            public final NodeNetworkProfileCommand payload;

            public PreviewNetworkProfile(NodeNetworkProfileCommand payload) {
                super(NodeCommandKind.PREVIEW_NETWORK_PROFILE);
                this.payload = payload;
            }
        }

        public static final class UpdateNetworkProfile extends NodeCommand {
            public final NodeNetworkProfileCommand payload;

            public UpdateNetworkProfile(NodeNetworkProfileCommand payload) {
                super(NodeCommandKind.UPDATE_NETWORK_PROFILE);
                this.payload = payload;
            }
        }

        public static final class PublishNetworkDiscovery extends NodeCommand {
            public final NodeNetworkDiscoveryCommand payload;

            public PublishNetworkDiscovery(NodeNetworkDiscoveryCommand payload) {
                super(NodeCommandKind.PUBLISH_NETWORK_DISCOVERY);
                this.payload = payload;
            }
        }

        public static final class UpdateNetworkDiscovery extends NodeCommand {
            public final NodeNetworkDiscoveryCommand payload;

            public UpdateNetworkDiscovery(NodeNetworkDiscoveryCommand payload) {
                super(NodeCommandKind.UPDATE_NETWORK_DISCOVERY);
                this.payload = payload;
            }
        }

        public static final class UnpublishNetworkDiscovery extends NodeCommand {
            public final NodeNetworkDiscoveryCommand payload;

            public UnpublishNetworkDiscovery(NodeNetworkDiscoveryCommand payload) {
                super(NodeCommandKind.UNPUBLISH_NETWORK_DISCOVERY);
                this.payload = payload;
            }
        }

        public static final class RefreshDiscoveryIndex extends NodeCommand {
            public final RefreshDiscoveryIndexPayload payload;

            public RefreshDiscoveryIndex(RefreshDiscoveryIndexPayload payload) {
                super(NodeCommandKind.REFRESH_DISCOVERY_INDEX);
                this.payload = payload;
            }
            /** DTO for RefreshDiscoveryIndexPayload. */
            public static final class RefreshDiscoveryIndexPayload {
                public final String index_url;

                public RefreshDiscoveryIndexPayload(String index_url) {
                    this.index_url = index_url;
                }
            }
        }

        public static final class RetryDiscoveryIndex extends NodeCommand {
            public RetryDiscoveryIndex() {
                super(NodeCommandKind.RETRY_DISCOVERY_INDEX);
            }
        }

        public static final class PreviewJoinNetwork extends NodeCommand {
            public final NodePreviewJoinNetworkCommand payload;

            public PreviewJoinNetwork(NodePreviewJoinNetworkCommand payload) {
                super(NodeCommandKind.PREVIEW_JOIN_NETWORK);
                this.payload = payload;
            }
        }

        public static final class JoinNetwork extends NodeCommand {
            public final NodeJoinNetworkCommand payload;

            public JoinNetwork(NodeJoinNetworkCommand payload) {
                super(NodeCommandKind.JOIN_NETWORK);
                this.payload = payload;
            }
        }

        public static final class SubmitJoinRequest extends NodeCommand {
            public final NodeSubmitJoinRequestCommand payload;

            public SubmitJoinRequest(NodeSubmitJoinRequestCommand payload) {
                super(NodeCommandKind.SUBMIT_JOIN_REQUEST);
                this.payload = payload;
            }
        }

        public static final class ListJoinRequests extends NodeCommand {
            public final NodeListJoinRequestsCommand payload;

            public ListJoinRequests(NodeListJoinRequestsCommand payload) {
                super(NodeCommandKind.LIST_JOIN_REQUESTS);
                this.payload = payload;
            }
        }

        public static final class ListNetworkMembers extends NodeCommand {
            public final NodeNetworkMembersCommand payload;

            public ListNetworkMembers(NodeNetworkMembersCommand payload) {
                super(NodeCommandKind.LIST_NETWORK_MEMBERS);
                this.payload = payload;
            }
        }

        public static final class PreviewMemberRemoval extends NodeCommand {
            public final NodeMemberRemovalPreviewCommand payload;

            public PreviewMemberRemoval(NodeMemberRemovalPreviewCommand payload) {
                super(NodeCommandKind.PREVIEW_MEMBER_REMOVAL);
                this.payload = payload;
            }
        }

        public static final class ChangeMemberLifecycle extends NodeCommand {
            public final NodeChangeMemberLifecycleCommand payload;

            public ChangeMemberLifecycle(NodeChangeMemberLifecycleCommand payload) {
                super(NodeCommandKind.CHANGE_MEMBER_LIFECYCLE);
                this.payload = payload;
            }
        }

        public static final class ReviewJoinRequest extends NodeCommand {
            public final NodeReviewJoinRequestCommand payload;

            public ReviewJoinRequest(NodeReviewJoinRequestCommand payload) {
                super(NodeCommandKind.REVIEW_JOIN_REQUEST);
                this.payload = payload;
            }
        }

        public static final class BatchReviewJoinRequests extends NodeCommand {
            public final NodeBatchReviewJoinRequestsCommand payload;

            public BatchReviewJoinRequests(NodeBatchReviewJoinRequestsCommand payload) {
                super(NodeCommandKind.BATCH_REVIEW_JOIN_REQUESTS);
                this.payload = payload;
            }
        }

        public static final class WithdrawJoinRequest extends NodeCommand {
            public final NodeWithdrawJoinRequestCommand payload;

            public WithdrawJoinRequest(NodeWithdrawJoinRequestCommand payload) {
                super(NodeCommandKind.WITHDRAW_JOIN_REQUEST);
                this.payload = payload;
            }
        }

        public static final class ResubmitJoinRequest extends NodeCommand {
            public final NodeResubmitJoinRequestCommand payload;

            public ResubmitJoinRequest(NodeResubmitJoinRequestCommand payload) {
                super(NodeCommandKind.RESUBMIT_JOIN_REQUEST);
                this.payload = payload;
            }
        }

        public static final class CreateInvitation extends NodeCommand {
            public final NodeCreateInvitationCommand payload;

            public CreateInvitation(NodeCreateInvitationCommand payload) {
                super(NodeCommandKind.CREATE_INVITATION);
                this.payload = payload;
            }
        }

        public static final class ListInvitations extends NodeCommand {
            public final NodeListInvitationsCommand payload;

            public ListInvitations(NodeListInvitationsCommand payload) {
                super(NodeCommandKind.LIST_INVITATIONS);
                this.payload = payload;
            }
        }

        public static final class GetInvitationStatus extends NodeCommand {
            public final NodeGetInvitationStatusCommand payload;

            public GetInvitationStatus(NodeGetInvitationStatusCommand payload) {
                super(NodeCommandKind.GET_INVITATION_STATUS);
                this.payload = payload;
            }
        }

        public static final class RevokeInvitation extends NodeCommand {
            public final NodeRevokeInvitationCommand payload;

            public RevokeInvitation(NodeRevokeInvitationCommand payload) {
                super(NodeCommandKind.REVOKE_INVITATION);
                this.payload = payload;
            }
        }

        public static final class PreviewInvitation extends NodeCommand {
            public final NodePreviewInvitationCommand payload;

            public PreviewInvitation(NodePreviewInvitationCommand payload) {
                super(NodeCommandKind.PREVIEW_INVITATION);
                this.payload = payload;
            }
        }

        public static final class RedeemInvitation extends NodeCommand {
            public final NodeRedeemInvitationCommand payload;

            public RedeemInvitation(NodeRedeemInvitationCommand payload) {
                super(NodeCommandKind.REDEEM_INVITATION);
                this.payload = payload;
            }
        }

        public static final class LeaveNetwork extends NodeCommand {
            public final NodeLeaveNetworkCommand payload;

            public LeaveNetwork(NodeLeaveNetworkCommand payload) {
                super(NodeCommandKind.LEAVE_NETWORK);
                this.payload = payload;
            }
        }

        public static final class AcknowledgeMembershipPolicy extends NodeCommand {
            public final NodeAcknowledgeMembershipPolicyCommand payload;

            public AcknowledgeMembershipPolicy(NodeAcknowledgeMembershipPolicyCommand payload) {
                super(NodeCommandKind.ACKNOWLEDGE_MEMBERSHIP_POLICY);
                this.payload = payload;
            }
        }

        public static final class UpdateRouting extends NodeCommand {
            public final NodeUpdateRoutingCommand payload;

            public UpdateRouting(NodeUpdateRoutingCommand payload) {
                super(NodeCommandKind.UPDATE_ROUTING);
                this.payload = payload;
            }
        }

        public static final class SetNetworkRole extends NodeCommand {
            public final NodeSetNetworkRoleCommand payload;

            public SetNetworkRole(NodeSetNetworkRoleCommand payload) {
                super(NodeCommandKind.SET_NETWORK_ROLE);
                this.payload = payload;
            }
        }

        public static final class SetNodeDisplayName extends NodeCommand {
            public final NodeSetNodeDisplayNameCommand payload;

            public SetNodeDisplayName(NodeSetNodeDisplayNameCommand payload) {
                super(NodeCommandKind.SET_NODE_DISPLAY_NAME);
                this.payload = payload;
            }
        }

        public static final class RotateNodeIdentity extends NodeCommand {
            public final NodeRotateNodeIdentityCommand payload;

            public RotateNodeIdentity(NodeRotateNodeIdentityCommand payload) {
                super(NodeCommandKind.ROTATE_NODE_IDENTITY);
                this.payload = payload;
            }
        }

        public static final class GetBlacklist extends NodeCommand {
            public final GetBlacklistPayload payload;

            public GetBlacklist(GetBlacklistPayload payload) {
                super(NodeCommandKind.GET_BLACKLIST);
                this.payload = payload;
            }
            /** DTO for GetBlacklistPayload. */
            public static final class GetBlacklistPayload {
                public final String network_name;

                public GetBlacklistPayload(String network_name) {
                    this.network_name = network_name;
                }
            }
        }

        public static final class BanClient extends NodeCommand {
            public final NodeClientBlacklistCommand payload;

            public BanClient(NodeClientBlacklistCommand payload) {
                super(NodeCommandKind.BAN_CLIENT);
                this.payload = payload;
            }
        }

        public static final class UnbanClient extends NodeCommand {
            public final NodeClientBlacklistCommand payload;

            public UnbanClient(NodeClientBlacklistCommand payload) {
                super(NodeCommandKind.UNBAN_CLIENT);
                this.payload = payload;
            }
        }

        public static final class GetNetworkAllocation extends NodeCommand {
            public final GetNetworkAllocationPayload payload;

            public GetNetworkAllocation(GetNetworkAllocationPayload payload) {
                super(NodeCommandKind.GET_NETWORK_ALLOCATION);
                this.payload = payload;
            }
            /** DTO for GetNetworkAllocationPayload. */
            public static final class GetNetworkAllocationPayload {
                public final String network_name;

                public GetNetworkAllocationPayload(String network_name) {
                    this.network_name = network_name;
                }
            }
        }

        public static final class UpdateNetworkAllocation extends NodeCommand {
            public final NodeUpdateNetworkAllocationCommand payload;

            public UpdateNetworkAllocation(NodeUpdateNetworkAllocationCommand payload) {
                super(NodeCommandKind.UPDATE_NETWORK_ALLOCATION);
                this.payload = payload;
            }
        }

        public static final class ProbePeer extends NodeCommand {
            public final NodeProbePeerCommand payload;

            public ProbePeer(NodeProbePeerCommand payload) {
                super(NodeCommandKind.PROBE_PEER);
                this.payload = payload;
            }
        }

        public static final class StartNode extends NodeCommand {
            public StartNode() {
                super(NodeCommandKind.START_NODE);
            }
        }

        public static final class StopNode extends NodeCommand {
            public StopNode() {
                super(NodeCommandKind.STOP_NODE);
            }
        }

        public static final class RestartNode extends NodeCommand {
            public RestartNode() {
                super(NodeCommandKind.RESTART_NODE);
            }
        }

        public static final class CleanupNodeResiduals extends NodeCommand {
            public final CleanupNodeResidualsPayload payload;

            public CleanupNodeResiduals(CleanupNodeResidualsPayload payload) {
                super(NodeCommandKind.CLEANUP_NODE_RESIDUALS);
                this.payload = payload;
            }
            /** DTO for CleanupNodeResidualsPayload. */
            public static final class CleanupNodeResidualsPayload {
                public final Boolean all;
                public final List<String> networks;

                public CleanupNodeResidualsPayload(Boolean all, List<String> networks) {
                    this.all = all;
                    this.networks = networks;
                }
            }
        }

        public static final class Shutdown extends NodeCommand {
            public Shutdown() {
                super(NodeCommandKind.SHUTDOWN);
            }
        }

        public static final class GetConfiguration extends NodeCommand {
            public GetConfiguration() {
                super(NodeCommandKind.GET_CONFIGURATION);
            }
        }

        public static final class PreviewConfiguration extends NodeCommand {
            public final NodePreviewConfigurationCommand payload;

            public PreviewConfiguration(NodePreviewConfigurationCommand payload) {
                super(NodeCommandKind.PREVIEW_CONFIGURATION);
                this.payload = payload;
            }
        }

        public static final class ApplyConfiguration extends NodeCommand {
            public final NodeApplyConfigurationCommand payload;

            public ApplyConfiguration(NodeApplyConfigurationCommand payload) {
                super(NodeCommandKind.APPLY_CONFIGURATION);
                this.payload = payload;
            }
        }

        public static final class RollbackConfiguration extends NodeCommand {
            public final NodeRollbackConfigurationCommand payload;

            public RollbackConfiguration(NodeRollbackConfigurationCommand payload) {
                super(NodeCommandKind.ROLLBACK_CONFIGURATION);
                this.payload = payload;
            }
        }

        public static final class GetConfigurationHistory extends NodeCommand {
            public GetConfigurationHistory() {
                super(NodeCommandKind.GET_CONFIGURATION_HISTORY);
            }
        }

        public static final class GetPolicy extends NodeCommand {
            public GetPolicy() {
                super(NodeCommandKind.GET_POLICY);
            }
        }

        public static final class PreviewPolicy extends NodeCommand {
            public final NodePreviewPolicyCommand payload;

            public PreviewPolicy(NodePreviewPolicyCommand payload) {
                super(NodeCommandKind.PREVIEW_POLICY);
                this.payload = payload;
            }
        }

        public static final class PreviewPolicyRollback extends NodeCommand {
            public final NodePreviewPolicyRollbackCommand payload;

            public PreviewPolicyRollback(NodePreviewPolicyRollbackCommand payload) {
                super(NodeCommandKind.PREVIEW_POLICY_ROLLBACK);
                this.payload = payload;
            }
        }

        public static final class ApplyPolicy extends NodeCommand {
            public final NodeApplyPolicyCommand payload;

            public ApplyPolicy(NodeApplyPolicyCommand payload) {
                super(NodeCommandKind.APPLY_POLICY);
                this.payload = payload;
            }
        }

        public static final class RollbackPolicy extends NodeCommand {
            public final NodeRollbackPolicyCommand payload;

            public RollbackPolicy(NodeRollbackPolicyCommand payload) {
                super(NodeCommandKind.ROLLBACK_POLICY);
                this.payload = payload;
            }
        }

        public static final class GetPolicyHistory extends NodeCommand {
            public GetPolicyHistory() {
                super(NodeCommandKind.GET_POLICY_HISTORY);
            }
        }

        public static final class TestPolicy extends NodeCommand {
            public final PolicyTestInput payload;

            public TestPolicy(PolicyTestInput payload) {
                super(NodeCommandKind.TEST_POLICY);
                this.payload = payload;
            }
        }

        public static final class GetTrafficPolicy extends NodeCommand {
            public GetTrafficPolicy() {
                super(NodeCommandKind.GET_TRAFFIC_POLICY);
            }
        }

        public static final class PreviewTrafficPolicy extends NodeCommand {
            public final NodePreviewTrafficPolicyCommand payload;

            public PreviewTrafficPolicy(NodePreviewTrafficPolicyCommand payload) {
                super(NodeCommandKind.PREVIEW_TRAFFIC_POLICY);
                this.payload = payload;
            }
        }

        public static final class PreviewTrafficPolicyRollback extends NodeCommand {
            public final NodePreviewTrafficPolicyRollbackCommand payload;

            public PreviewTrafficPolicyRollback(NodePreviewTrafficPolicyRollbackCommand payload) {
                super(NodeCommandKind.PREVIEW_TRAFFIC_POLICY_ROLLBACK);
                this.payload = payload;
            }
        }

        public static final class ApplyTrafficPolicy extends NodeCommand {
            public final NodeApplyTrafficPolicyCommand payload;

            public ApplyTrafficPolicy(NodeApplyTrafficPolicyCommand payload) {
                super(NodeCommandKind.APPLY_TRAFFIC_POLICY);
                this.payload = payload;
            }
        }

        public static final class RollbackTrafficPolicy extends NodeCommand {
            public final NodeRollbackTrafficPolicyCommand payload;

            public RollbackTrafficPolicy(NodeRollbackTrafficPolicyCommand payload) {
                super(NodeCommandKind.ROLLBACK_TRAFFIC_POLICY);
                this.payload = payload;
            }
        }

        public static final class GetTrafficPolicyHistory extends NodeCommand {
            public GetTrafficPolicyHistory() {
                super(NodeCommandKind.GET_TRAFFIC_POLICY_HISTORY);
            }
        }

        public static final class TestTrafficPolicy extends NodeCommand {
            public final NodeTestTrafficPolicyCommand payload;

            public TestTrafficPolicy(NodeTestTrafficPolicyCommand payload) {
                super(NodeCommandKind.TEST_TRAFFIC_POLICY);
                this.payload = payload;
            }
        }
    }

    /** Tagged union DTO for NodeCommandOutcome. */
    public abstract static class NodeCommandOutcome {
        public final String status;

        private NodeCommandOutcome(String status) {
            this.status = status;
        }

        public static final class Snapshot extends NodeCommandOutcome {
            public final NodeSnapshot data;

            public Snapshot(NodeSnapshot data) {
                super("snapshot");
                this.data = data;
            }
        }

        public static final class UiManifest extends NodeCommandOutcome {
            public final NodeUiManifest data;

            public UiManifest(NodeUiManifest data) {
                super("ui_manifest");
                this.data = data;
            }
        }

        public static final class Diagnostics extends NodeCommandOutcome {
            public final NodeDiagnosticsSnapshot data;

            public Diagnostics(NodeDiagnosticsSnapshot data) {
                super("diagnostics");
                this.data = data;
            }
        }

        public static final class Cleanup extends NodeCommandOutcome {
            public final NodeResidualCleanupResult data;

            public Cleanup(NodeResidualCleanupResult data) {
                super("cleanup");
                this.data = data;
            }
        }

        public static final class Blacklist extends NodeCommandOutcome {
            public final NodeBlacklistSnapshot data;

            public Blacklist(NodeBlacklistSnapshot data) {
                super("blacklist");
                this.data = data;
            }
        }

        public static final class NetworkAllocation extends NodeCommandOutcome {
            public final NodeNetworkAllocationSnapshot data;

            public NetworkAllocation(NodeNetworkAllocationSnapshot data) {
                super("network_allocation");
                this.data = data;
            }
        }

        public static final class Probe extends NodeCommandOutcome {
            public final NodePeerProbeSnapshot data;

            public Probe(NodePeerProbeSnapshot data) {
                super("probe");
                this.data = data;
            }
        }

        public static final class NetworkCreated extends NodeCommandOutcome {
            public final NodeCreatedNetwork data;

            public NetworkCreated(NodeCreatedNetwork data) {
                super("network_created");
                this.data = data;
            }
        }

        public static final class NetworkProfilePreview extends NodeCommandOutcome {
            public final NetworkProfilePreview data;

            public NetworkProfilePreview(NetworkProfilePreview data) {
                super("network_profile_preview");
                this.data = data;
            }
        }

        public static final class NetworkProfileUpdated extends NodeCommandOutcome {
            public final NetworkProfileApplyResult data;

            public NetworkProfileUpdated(NetworkProfileApplyResult data) {
                super("network_profile_updated");
                this.data = data;
            }
        }

        public static final class NetworkJoinPreview extends NodeCommandOutcome {
            public final NodeNetworkJoinPreview data;

            public NetworkJoinPreview(NodeNetworkJoinPreview data) {
                super("network_join_preview");
                this.data = data;
            }
        }

        public static final class NetworkJoined extends NodeCommandOutcome {
            public final NodeNetworkJoinResult data;

            public NetworkJoined(NodeNetworkJoinResult data) {
                super("network_joined");
                this.data = data;
            }
        }

        public static final class JoinRequestSubmitted extends NodeCommandOutcome {
            public final JoinRequest data;

            public JoinRequestSubmitted(JoinRequest data) {
                super("join_request_submitted");
                this.data = data;
            }
        }

        public static final class JoinRequests extends NodeCommandOutcome {
            public final NodeJoinRequestListResult data;

            public JoinRequests(NodeJoinRequestListResult data) {
                super("join_requests");
                this.data = data;
            }
        }

        public static final class NetworkMembers extends NodeCommandOutcome {
            public final NetworkMemberDirectory data;

            public NetworkMembers(NetworkMemberDirectory data) {
                super("network_members");
                this.data = data;
            }
        }

        public static final class MemberRemovalPreview extends NodeCommandOutcome {
            public final MemberRemovalPreview data;

            public MemberRemovalPreview(MemberRemovalPreview data) {
                super("member_removal_preview");
                this.data = data;
            }
        }

        public static final class MemberLifecycleChanged extends NodeCommandOutcome {
            public final MemberLifecycleChange data;

            public MemberLifecycleChanged(MemberLifecycleChange data) {
                super("member_lifecycle_changed");
                this.data = data;
            }
        }

        public static final class JoinRequestsReviewed extends NodeCommandOutcome {
            public final NodeJoinRequestReviewAck data;

            public JoinRequestsReviewed(NodeJoinRequestReviewAck data) {
                super("join_requests_reviewed");
                this.data = data;
            }
        }

        public static final class JoinRequestUpdated extends NodeCommandOutcome {
            public final JoinRequest data;

            public JoinRequestUpdated(JoinRequest data) {
                super("join_request_updated");
                this.data = data;
            }
        }

        public static final class InvitationCreated extends NodeCommandOutcome {
            public final IssuedInvitation data;

            public InvitationCreated(IssuedInvitation data) {
                super("invitation_created");
                this.data = data;
            }
        }

        public static final class Invitations extends NodeCommandOutcome {
            public final List<InvitationStatusView> data;

            public Invitations(List<InvitationStatusView> data) {
                super("invitations");
                this.data = data;
            }
        }

        public static final class InvitationStatus extends NodeCommandOutcome {
            public final InvitationStatusView data;

            public InvitationStatus(InvitationStatusView data) {
                super("invitation_status");
                this.data = data;
            }
        }

        public static final class InvitationRevoked extends NodeCommandOutcome {
            public final InvitationStatusView data;

            public InvitationRevoked(InvitationStatusView data) {
                super("invitation_revoked");
                this.data = data;
            }
        }

        public static final class InvitationPreview extends NodeCommandOutcome {
            public final InvitationPreview data;

            public InvitationPreview(InvitationPreview data) {
                super("invitation_preview");
                this.data = data;
            }
        }

        public static final class InvitationRedeemed extends NodeCommandOutcome {
            public final InvitationRedemption data;

            public InvitationRedeemed(InvitationRedemption data) {
                super("invitation_redeemed");
                this.data = data;
            }
        }

        public static final class NetworkLeft extends NodeCommandOutcome {
            public final NodeNetworkLeaveResult data;

            public NetworkLeft(NodeNetworkLeaveResult data) {
                super("network_left");
                this.data = data;
            }
        }

        public static final class MembershipPolicyAcknowledged extends NodeCommandOutcome {
            public final NodeNetworkMembershipSnapshot data;

            public MembershipPolicyAcknowledged(NodeNetworkMembershipSnapshot data) {
                super("membership_policy_acknowledged");
                this.data = data;
            }
        }

        public static final class IdentityRotated extends NodeCommandOutcome {
            public final NodeIdentityRotationSnapshot data;

            public IdentityRotated(NodeIdentityRotationSnapshot data) {
                super("identity_rotated");
                this.data = data;
            }
        }

        public static final class Configuration extends NodeCommandOutcome {
            public final ConfigurationDocument data;

            public Configuration(ConfigurationDocument data) {
                super("configuration");
                this.data = data;
            }
        }

        public static final class ConfigurationPreview extends NodeCommandOutcome {
            public final ConfigurationPreview data;

            public ConfigurationPreview(ConfigurationPreview data) {
                super("configuration_preview");
                this.data = data;
            }
        }

        public static final class ConfigurationApplied extends NodeCommandOutcome {
            public final ConfigurationApplyResult data;

            public ConfigurationApplied(ConfigurationApplyResult data) {
                super("configuration_applied");
                this.data = data;
            }
        }

        public static final class ConfigurationHistory extends NodeCommandOutcome {
            public final List<ConfigurationHistoryEntry> data;

            public ConfigurationHistory(List<ConfigurationHistoryEntry> data) {
                super("configuration_history");
                this.data = data;
            }
        }

        public static final class Policy extends NodeCommandOutcome {
            public final PolicyCurrent data;

            public Policy(PolicyCurrent data) {
                super("policy");
                this.data = data;
            }
        }

        public static final class PolicyPreview extends NodeCommandOutcome {
            public final PolicyPreview data;

            public PolicyPreview(PolicyPreview data) {
                super("policy_preview");
                this.data = data;
            }
        }

        public static final class PolicyApplied extends NodeCommandOutcome {
            public final PolicyApplyResult data;

            public PolicyApplied(PolicyApplyResult data) {
                super("policy_applied");
                this.data = data;
            }
        }

        public static final class PolicyHistory extends NodeCommandOutcome {
            public final PolicyHistorySnapshot data;

            public PolicyHistory(PolicyHistorySnapshot data) {
                super("policy_history");
                this.data = data;
            }
        }

        public static final class PolicyRolledBack extends NodeCommandOutcome {
            public final PolicyRollbackResult data;

            public PolicyRolledBack(PolicyRollbackResult data) {
                super("policy_rolled_back");
                this.data = data;
            }
        }

        public static final class PolicyTest extends NodeCommandOutcome {
            public final PolicyTestResult data;

            public PolicyTest(PolicyTestResult data) {
                super("policy_test");
                this.data = data;
            }
        }

        public static final class TrafficPolicy extends NodeCommandOutcome {
            public final TrafficPolicyCurrent data;

            public TrafficPolicy(TrafficPolicyCurrent data) {
                super("traffic_policy");
                this.data = data;
            }
        }

        public static final class TrafficPolicyPreview extends NodeCommandOutcome {
            public final TrafficPolicyPreview data;

            public TrafficPolicyPreview(TrafficPolicyPreview data) {
                super("traffic_policy_preview");
                this.data = data;
            }
        }

        public static final class TrafficPolicyApplied extends NodeCommandOutcome {
            public final TrafficPolicyApplyResult data;

            public TrafficPolicyApplied(TrafficPolicyApplyResult data) {
                super("traffic_policy_applied");
                this.data = data;
            }
        }

        public static final class TrafficPolicyHistory extends NodeCommandOutcome {
            public final TrafficPolicyHistorySnapshot data;

            public TrafficPolicyHistory(TrafficPolicyHistorySnapshot data) {
                super("traffic_policy_history");
                this.data = data;
            }
        }

        public static final class TrafficPolicyRolledBack extends NodeCommandOutcome {
            public final TrafficPolicyRollbackResult data;

            public TrafficPolicyRolledBack(TrafficPolicyRollbackResult data) {
                super("traffic_policy_rolled_back");
                this.data = data;
            }
        }

        public static final class TrafficPolicyTest extends NodeCommandOutcome {
            public final TrafficPolicyTestComparison data;

            public TrafficPolicyTest(TrafficPolicyTestComparison data) {
                super("traffic_policy_test");
                this.data = data;
            }
        }

        public static final class Accepted extends NodeCommandOutcome {
            public final AcceptedData data;

            public Accepted(AcceptedData data) {
                super("accepted");
                this.data = data;
            }
            /** DTO for AcceptedData. */
            public static final class AcceptedData {
                public final String operation_id;

                public AcceptedData(String operation_id) {
                    this.operation_id = operation_id;
                }
            }
        }

        public static final class Completed extends NodeCommandOutcome {
            public final CompletedData data;

            public Completed(CompletedData data) {
                super("completed");
                this.data = data;
            }
            /** DTO for CompletedData. */
            public static final class CompletedData {
                public final String message;

                public CompletedData(String message) {
                    this.message = message;
                }
            }
        }

        public static final class Rejected extends NodeCommandOutcome {
            public final NodeProblem data;

            public Rejected(NodeProblem data) {
                super("rejected");
                this.data = data;
            }
        }
    }

    /** Tagged union DTO for NodeEventKind. */
    public abstract static class NodeEventKind {
        public final String type;

        private NodeEventKind(String type) {
            this.type = type;
        }

        public static final class StateChanged extends NodeEventKind {
            public final StateChangedData data;

            public StateChanged(StateChangedData data) {
                super("state_changed");
                this.data = data;
            }
            /** DTO for StateChangedData. */
            public static final class StateChangedData {
                public final Long revision;

                public StateChangedData(Long revision) {
                    this.revision = revision;
                }
            }
        }

        public static final class NetworkChanged extends NodeEventKind {
            public final NetworkChangedData data;

            public NetworkChanged(NetworkChangedData data) {
                super("network_changed");
                this.data = data;
            }
            /** DTO for NetworkChangedData. */
            public static final class NetworkChangedData {
                public final String network_name;
                public final NodeNetworkStatus status;

                public NetworkChangedData(String network_name, NodeNetworkStatus status) {
                    this.network_name = network_name;
                    this.status = status;
                }
            }
        }

        public static final class EmbeddedServiceChanged extends NodeEventKind {
            public final EmbeddedServiceChangedData data;

            public EmbeddedServiceChanged(EmbeddedServiceChangedData data) {
                super("embedded_service_changed");
                this.data = data;
            }
            /** DTO for EmbeddedServiceChangedData. */
            public static final class EmbeddedServiceChangedData {
                public final String network_name;
                public final NodeEmbeddedServiceStatus status;

                public EmbeddedServiceChangedData(String network_name, NodeEmbeddedServiceStatus status) {
                    this.network_name = network_name;
                    this.status = status;
                }
            }
        }

        public static final class LogAppended extends NodeEventKind {
            public final LogAppendedData data;

            public LogAppended(LogAppendedData data) {
                super("log_appended");
                this.data = data;
            }
            /** DTO for LogAppendedData. */
            public static final class LogAppendedData {
                public final NodeLogLevel level;
                public final String message;

                public LogAppendedData(NodeLogLevel level, String message) {
                    this.level = level;
                    this.message = message;
                }
            }
        }

        public static final class RoutingChanged extends NodeEventKind {
            public final RoutingChangedData data;

            public RoutingChanged(RoutingChangedData data) {
                super("routing_changed");
                this.data = data;
            }
            /** DTO for RoutingChangedData. */
            public static final class RoutingChangedData {
                public final String exit_node;
                public final Boolean is_exit_node;
                public final NodeRoutingMode mode;
                public final String network_name;

                public RoutingChangedData(String exit_node, Boolean is_exit_node, NodeRoutingMode mode, String network_name) {
                    this.exit_node = exit_node;
                    this.is_exit_node = is_exit_node;
                    this.mode = mode;
                    this.network_name = network_name;
                }
            }
        }

        public static final class RoleChanged extends NodeEventKind {
            public final RoleChangedData data;

            public RoleChanged(RoleChangedData data) {
                super("role_changed");
                this.data = data;
            }
            /** DTO for RoleChangedData. */
            public static final class RoleChangedData {
                public final Boolean enabled;
                public final String network_name;
                public final NodeNetworkRole role;
                public final List<NodeNetworkRole> roles;

                public RoleChangedData(Boolean enabled, String network_name, NodeNetworkRole role, List<NodeNetworkRole> roles) {
                    this.enabled = enabled;
                    this.network_name = network_name;
                    this.role = role;
                    this.roles = roles;
                }
            }
        }

        public static final class CommandCompleted extends NodeEventKind {
            public final CommandCompletedData data;

            public CommandCompleted(CommandCompletedData data) {
                super("command_completed");
                this.data = data;
            }
            /** DTO for CommandCompletedData. */
            public static final class CommandCompletedData {
                public final Long request_id;
                public final Boolean success;

                public CommandCompletedData(Long request_id, Boolean success) {
                    this.request_id = request_id;
                    this.success = success;
                }
            }
        }

        public static final class OperationChanged extends NodeEventKind {
            public final OperationChangedData data;

            public OperationChanged(OperationChangedData data) {
                super("operation_changed");
                this.data = data;
            }
            /** DTO for OperationChangedData. */
            public static final class OperationChangedData {
                public final String operation_id;
                public final String stage;
                public final OperationState state;

                public OperationChangedData(String operation_id, String stage, OperationState state) {
                    this.operation_id = operation_id;
                    this.stage = stage;
                    this.state = state;
                }
            }
        }
    }

    /** Tagged union DTO for NodeNetworkScope. */
    public abstract static class NodeNetworkScope {
        public final String kind;

        private NodeNetworkScope(String kind) {
            this.kind = kind;
        }

        public static final class All extends NodeNetworkScope {
            public All() {
                super("all");
            }
        }

        public static final class Only extends NodeNetworkScope {
            public final List<String> network_ids;

            public Only(List<String> network_ids) {
                super("only");
                this.network_ids = network_ids;
            }
        }
    }

    /** Tagged union DTO for NodeNetworkSelection. */
    public abstract static class NodeNetworkSelection {
        public final String kind;

        private NodeNetworkSelection(String kind) {
            this.kind = kind;
        }

        public static final class Default extends NodeNetworkSelection {
            public Default() {
                super("default");
            }
        }

        public static final class All extends NodeNetworkSelection {
            public All() {
                super("all");
            }
        }

        public static final class None extends NodeNetworkSelection {
            public None() {
                super("none");
            }
        }

        public static final class Named extends NodeNetworkSelection {
            public final List<String> named;

            public Named(List<String> named) {
                super("named");
                this.named = named;
            }
        }
    }

    /** Tagged union DTO for NodeRoutingDefaultAction. */
    public abstract static class NodeRoutingDefaultAction {
        public final String kind;

        private NodeRoutingDefaultAction(String kind) {
            this.kind = kind;
        }

        public static final class Direct extends NodeRoutingDefaultAction {
            public Direct() {
                super("direct");
            }
        }

        public static final class Network extends NodeRoutingDefaultAction {
            public Network() {
                super("network");
            }
        }

        public static final class Exit extends NodeRoutingDefaultAction {
            public final String value;

            public Exit(String value) {
                super("exit");
                this.value = value;
            }
        }

        public static final class Reject extends NodeRoutingDefaultAction {
            public Reject() {
                super("reject");
            }
        }
    }

    /** Tagged union DTO for NodeRoutingTargetSnapshot. */
    public abstract static class NodeRoutingTargetSnapshot {
        public final String kind;

        private NodeRoutingTargetSnapshot(String kind) {
            this.kind = kind;
        }

        public static final class Network extends NodeRoutingTargetSnapshot {
            public final String network_name;

            public Network(String network_name) {
                super("network");
                this.network_name = network_name;
            }
        }

        public static final class Exit extends NodeRoutingTargetSnapshot {
            public final String node_id;

            public Exit(String node_id) {
                super("exit");
                this.node_id = node_id;
            }
        }
    }

    /** Tagged union DTO for NodeWsMessage. */
    public abstract static class NodeWsMessage {
        public final String type;

        private NodeWsMessage(String type) {
            this.type = type;
        }

        public static final class Snapshot extends NodeWsMessage {
            public final Integer schema_version;
            public final ProjectedNodeSnapshot snapshot;

            public Snapshot(Integer schema_version, ProjectedNodeSnapshot snapshot) {
                super("snapshot");
                this.schema_version = schema_version;
                this.snapshot = snapshot;
            }
        }

        public static final class Event extends NodeWsMessage {
            public final ProjectedNodeEvent event;
            public final Integer schema_version;

            public Event(ProjectedNodeEvent event, Integer schema_version) {
                super("event");
                this.event = event;
                this.schema_version = schema_version;
            }
        }

        public static final class Resync extends NodeWsMessage {
            public final String reason;
            public final Integer schema_version;
            public final ProjectedNodeSnapshot snapshot;

            public Resync(String reason, Integer schema_version, ProjectedNodeSnapshot snapshot) {
                super("resync");
                this.reason = reason;
                this.schema_version = schema_version;
                this.snapshot = snapshot;
            }
        }

        public static final class Error extends NodeWsMessage {
            public final NodeErrorCategory category;
            public final NodeHttpErrorCode code;
            public final String message;
            public final String message_key;
            public final DomainObjectRef object;
            public final Map<String, Object> params;
            public final OperationRecovery recovery;
            public final Boolean retryable;
            public final Integer schema_version;

            public Error(NodeErrorCategory category, NodeHttpErrorCode code, String message, String message_key, DomainObjectRef object, Map<String, Object> params, OperationRecovery recovery, Boolean retryable, Integer schema_version) {
                super("error");
                this.category = category;
                this.code = code;
                this.message = message;
                this.message_key = message_key;
                this.object = object;
                this.params = params;
                this.recovery = recovery;
                this.retryable = retryable;
                this.schema_version = schema_version;
            }
        }
    }

    /** Tagged union DTO for PermissionCondition. */
    public abstract static class PermissionCondition {
        public final String kind;

        private PermissionCondition(String kind) {
            this.kind = kind;
        }

        public static final class MembershipActive extends PermissionCondition {
            public MembershipActive() {
                super("membership_active");
            }
        }

        public static final class RecentReauthentication extends PermissionCondition {
            public RecentReauthentication() {
                super("recent_reauthentication");
            }
        }

        public static final class ResourceInNetwork extends PermissionCondition {
            public final String value;

            public ResourceInNetwork(String value) {
                super("resource_in_network");
                this.value = value;
            }
        }

        public static final class NonEmergencyCoordinator extends PermissionCondition {
            public NonEmergencyCoordinator() {
                super("non_emergency_coordinator");
            }
        }
    }

    /** Tagged union DTO for ProjectedNodeCommandOutcome. */
    public abstract static class ProjectedNodeCommandOutcome {
        public final String status;

        private ProjectedNodeCommandOutcome(String status) {
            this.status = status;
        }

        public static final class Snapshot extends ProjectedNodeCommandOutcome {
            public final ProjectedNodeSnapshot data;

            public Snapshot(ProjectedNodeSnapshot data) {
                super("snapshot");
                this.data = data;
            }
        }

        public static final class UiManifest extends ProjectedNodeCommandOutcome {
            public final NodeUiManifest data;

            public UiManifest(NodeUiManifest data) {
                super("ui_manifest");
                this.data = data;
            }
        }

        public static final class Diagnostics extends ProjectedNodeCommandOutcome {
            public final NodeDiagnosticsSnapshot data;

            public Diagnostics(NodeDiagnosticsSnapshot data) {
                super("diagnostics");
                this.data = data;
            }
        }

        public static final class Cleanup extends ProjectedNodeCommandOutcome {
            public final NodeResidualCleanupResult data;

            public Cleanup(NodeResidualCleanupResult data) {
                super("cleanup");
                this.data = data;
            }
        }

        public static final class Blacklist extends ProjectedNodeCommandOutcome {
            public final NodeBlacklistSnapshot data;

            public Blacklist(NodeBlacklistSnapshot data) {
                super("blacklist");
                this.data = data;
            }
        }

        public static final class NetworkAllocation extends ProjectedNodeCommandOutcome {
            public final NodeNetworkAllocationSnapshot data;

            public NetworkAllocation(NodeNetworkAllocationSnapshot data) {
                super("network_allocation");
                this.data = data;
            }
        }

        public static final class Probe extends ProjectedNodeCommandOutcome {
            public final NodePeerProbeSnapshot data;

            public Probe(NodePeerProbeSnapshot data) {
                super("probe");
                this.data = data;
            }
        }

        public static final class NetworkCreated extends ProjectedNodeCommandOutcome {
            public final NodeCreatedNetwork data;

            public NetworkCreated(NodeCreatedNetwork data) {
                super("network_created");
                this.data = data;
            }
        }

        public static final class NetworkProfilePreview extends ProjectedNodeCommandOutcome {
            public final NetworkProfilePreview data;

            public NetworkProfilePreview(NetworkProfilePreview data) {
                super("network_profile_preview");
                this.data = data;
            }
        }

        public static final class NetworkProfileUpdated extends ProjectedNodeCommandOutcome {
            public final NetworkProfileApplyResult data;

            public NetworkProfileUpdated(NetworkProfileApplyResult data) {
                super("network_profile_updated");
                this.data = data;
            }
        }

        public static final class NetworkJoinPreview extends ProjectedNodeCommandOutcome {
            public final NodeNetworkJoinPreview data;

            public NetworkJoinPreview(NodeNetworkJoinPreview data) {
                super("network_join_preview");
                this.data = data;
            }
        }

        public static final class NetworkJoined extends ProjectedNodeCommandOutcome {
            public final ProjectedNodeNetworkJoinResult data;

            public NetworkJoined(ProjectedNodeNetworkJoinResult data) {
                super("network_joined");
                this.data = data;
            }
        }

        public static final class JoinRequestSubmitted extends ProjectedNodeCommandOutcome {
            public final JoinRequest data;

            public JoinRequestSubmitted(JoinRequest data) {
                super("join_request_submitted");
                this.data = data;
            }
        }

        public static final class JoinRequests extends ProjectedNodeCommandOutcome {
            public final NodeJoinRequestListResult data;

            public JoinRequests(NodeJoinRequestListResult data) {
                super("join_requests");
                this.data = data;
            }
        }

        public static final class NetworkMembers extends ProjectedNodeCommandOutcome {
            public final NetworkMemberDirectory data;

            public NetworkMembers(NetworkMemberDirectory data) {
                super("network_members");
                this.data = data;
            }
        }

        public static final class MemberRemovalPreview extends ProjectedNodeCommandOutcome {
            public final MemberRemovalPreview data;

            public MemberRemovalPreview(MemberRemovalPreview data) {
                super("member_removal_preview");
                this.data = data;
            }
        }

        public static final class MemberLifecycleChanged extends ProjectedNodeCommandOutcome {
            public final MemberLifecycleChange data;

            public MemberLifecycleChanged(MemberLifecycleChange data) {
                super("member_lifecycle_changed");
                this.data = data;
            }
        }

        public static final class JoinRequestsReviewed extends ProjectedNodeCommandOutcome {
            public final NodeJoinRequestReviewAck data;

            public JoinRequestsReviewed(NodeJoinRequestReviewAck data) {
                super("join_requests_reviewed");
                this.data = data;
            }
        }

        public static final class JoinRequestUpdated extends ProjectedNodeCommandOutcome {
            public final JoinRequest data;

            public JoinRequestUpdated(JoinRequest data) {
                super("join_request_updated");
                this.data = data;
            }
        }

        public static final class InvitationCreated extends ProjectedNodeCommandOutcome {
            public final IssuedInvitation data;

            public InvitationCreated(IssuedInvitation data) {
                super("invitation_created");
                this.data = data;
            }
        }

        public static final class Invitations extends ProjectedNodeCommandOutcome {
            public final List<InvitationStatusView> data;

            public Invitations(List<InvitationStatusView> data) {
                super("invitations");
                this.data = data;
            }
        }

        public static final class InvitationStatus extends ProjectedNodeCommandOutcome {
            public final InvitationStatusView data;

            public InvitationStatus(InvitationStatusView data) {
                super("invitation_status");
                this.data = data;
            }
        }

        public static final class InvitationRevoked extends ProjectedNodeCommandOutcome {
            public final InvitationStatusView data;

            public InvitationRevoked(InvitationStatusView data) {
                super("invitation_revoked");
                this.data = data;
            }
        }

        public static final class InvitationPreview extends ProjectedNodeCommandOutcome {
            public final InvitationPreview data;

            public InvitationPreview(InvitationPreview data) {
                super("invitation_preview");
                this.data = data;
            }
        }

        public static final class InvitationRedeemed extends ProjectedNodeCommandOutcome {
            public final InvitationRedemption data;

            public InvitationRedeemed(InvitationRedemption data) {
                super("invitation_redeemed");
                this.data = data;
            }
        }

        public static final class NetworkLeft extends ProjectedNodeCommandOutcome {
            public final ProjectedNodeNetworkLeaveResult data;

            public NetworkLeft(ProjectedNodeNetworkLeaveResult data) {
                super("network_left");
                this.data = data;
            }
        }

        public static final class MembershipPolicyAcknowledged extends ProjectedNodeCommandOutcome {
            public final NodeNetworkMembershipSnapshot data;

            public MembershipPolicyAcknowledged(NodeNetworkMembershipSnapshot data) {
                super("membership_policy_acknowledged");
                this.data = data;
            }
        }

        public static final class IdentityRotated extends ProjectedNodeCommandOutcome {
            public final NodeIdentityRotationSnapshot data;

            public IdentityRotated(NodeIdentityRotationSnapshot data) {
                super("identity_rotated");
                this.data = data;
            }
        }

        public static final class Configuration extends ProjectedNodeCommandOutcome {
            public final ConfigurationDocument data;

            public Configuration(ConfigurationDocument data) {
                super("configuration");
                this.data = data;
            }
        }

        public static final class ConfigurationPreview extends ProjectedNodeCommandOutcome {
            public final ConfigurationPreview data;

            public ConfigurationPreview(ConfigurationPreview data) {
                super("configuration_preview");
                this.data = data;
            }
        }

        public static final class ConfigurationApplied extends ProjectedNodeCommandOutcome {
            public final ConfigurationApplyResult data;

            public ConfigurationApplied(ConfigurationApplyResult data) {
                super("configuration_applied");
                this.data = data;
            }
        }

        public static final class ConfigurationHistory extends ProjectedNodeCommandOutcome {
            public final List<ConfigurationHistoryEntry> data;

            public ConfigurationHistory(List<ConfigurationHistoryEntry> data) {
                super("configuration_history");
                this.data = data;
            }
        }

        public static final class Policy extends ProjectedNodeCommandOutcome {
            public final PolicyCurrent data;

            public Policy(PolicyCurrent data) {
                super("policy");
                this.data = data;
            }
        }

        public static final class PolicyPreview extends ProjectedNodeCommandOutcome {
            public final PolicyPreview data;

            public PolicyPreview(PolicyPreview data) {
                super("policy_preview");
                this.data = data;
            }
        }

        public static final class PolicyApplied extends ProjectedNodeCommandOutcome {
            public final PolicyApplyResult data;

            public PolicyApplied(PolicyApplyResult data) {
                super("policy_applied");
                this.data = data;
            }
        }

        public static final class PolicyHistory extends ProjectedNodeCommandOutcome {
            public final PolicyHistorySnapshot data;

            public PolicyHistory(PolicyHistorySnapshot data) {
                super("policy_history");
                this.data = data;
            }
        }

        public static final class PolicyRolledBack extends ProjectedNodeCommandOutcome {
            public final PolicyRollbackResult data;

            public PolicyRolledBack(PolicyRollbackResult data) {
                super("policy_rolled_back");
                this.data = data;
            }
        }

        public static final class PolicyTest extends ProjectedNodeCommandOutcome {
            public final PolicyTestResult data;

            public PolicyTest(PolicyTestResult data) {
                super("policy_test");
                this.data = data;
            }
        }

        public static final class TrafficPolicy extends ProjectedNodeCommandOutcome {
            public final TrafficPolicyCurrent data;

            public TrafficPolicy(TrafficPolicyCurrent data) {
                super("traffic_policy");
                this.data = data;
            }
        }

        public static final class TrafficPolicyPreview extends ProjectedNodeCommandOutcome {
            public final TrafficPolicyPreview data;

            public TrafficPolicyPreview(TrafficPolicyPreview data) {
                super("traffic_policy_preview");
                this.data = data;
            }
        }

        public static final class TrafficPolicyApplied extends ProjectedNodeCommandOutcome {
            public final TrafficPolicyApplyResult data;

            public TrafficPolicyApplied(TrafficPolicyApplyResult data) {
                super("traffic_policy_applied");
                this.data = data;
            }
        }

        public static final class TrafficPolicyHistory extends ProjectedNodeCommandOutcome {
            public final TrafficPolicyHistorySnapshot data;

            public TrafficPolicyHistory(TrafficPolicyHistorySnapshot data) {
                super("traffic_policy_history");
                this.data = data;
            }
        }

        public static final class TrafficPolicyRolledBack extends ProjectedNodeCommandOutcome {
            public final TrafficPolicyRollbackResult data;

            public TrafficPolicyRolledBack(TrafficPolicyRollbackResult data) {
                super("traffic_policy_rolled_back");
                this.data = data;
            }
        }

        public static final class TrafficPolicyTest extends ProjectedNodeCommandOutcome {
            public final TrafficPolicyTestComparison data;

            public TrafficPolicyTest(TrafficPolicyTestComparison data) {
                super("traffic_policy_test");
                this.data = data;
            }
        }

        public static final class Accepted extends ProjectedNodeCommandOutcome {
            public final AcceptedData data;

            public Accepted(AcceptedData data) {
                super("accepted");
                this.data = data;
            }
            /** DTO for AcceptedData. */
            public static final class AcceptedData {
                public final String operation_id;

                public AcceptedData(String operation_id) {
                    this.operation_id = operation_id;
                }
            }
        }

        public static final class Completed extends ProjectedNodeCommandOutcome {
            public final CompletedData data;

            public Completed(CompletedData data) {
                super("completed");
                this.data = data;
            }
            /** DTO for CompletedData. */
            public static final class CompletedData {
                public final String message;

                public CompletedData(String message) {
                    this.message = message;
                }
            }
        }

        public static final class Rejected extends ProjectedNodeCommandOutcome {
            public final NodeProblem data;

            public Rejected(NodeProblem data) {
                super("rejected");
                this.data = data;
            }
        }
    }

    /** Tagged union DTO for ProjectedNodeEventKind. */
    public abstract static class ProjectedNodeEventKind {
        public final String type;

        private ProjectedNodeEventKind(String type) {
            this.type = type;
        }

        public static final class StateChanged extends ProjectedNodeEventKind {
            public final StateChangedData data;

            public StateChanged(StateChangedData data) {
                super("state_changed");
                this.data = data;
            }
            /** DTO for StateChangedData. */
            public static final class StateChangedData {
                public final Long revision;

                public StateChangedData(Long revision) {
                    this.revision = revision;
                }
            }
        }

        public static final class NetworkChanged extends ProjectedNodeEventKind {
            public final NetworkChangedData data;

            public NetworkChanged(NetworkChangedData data) {
                super("network_changed");
                this.data = data;
            }
            /** DTO for NetworkChangedData. */
            public static final class NetworkChangedData {
                public final String network_name;
                public final NodeNetworkStatus status;

                public NetworkChangedData(String network_name, NodeNetworkStatus status) {
                    this.network_name = network_name;
                    this.status = status;
                }
            }
        }

        public static final class EmbeddedServiceChanged extends ProjectedNodeEventKind {
            public final EmbeddedServiceChangedData data;

            public EmbeddedServiceChanged(EmbeddedServiceChangedData data) {
                super("embedded_service_changed");
                this.data = data;
            }
            /** DTO for EmbeddedServiceChangedData. */
            public static final class EmbeddedServiceChangedData {
                public final String network_name;
                public final NodeEmbeddedServiceStatus status;

                public EmbeddedServiceChangedData(String network_name, NodeEmbeddedServiceStatus status) {
                    this.network_name = network_name;
                    this.status = status;
                }
            }
        }

        public static final class LogAppended extends ProjectedNodeEventKind {
            public final LogAppendedData data;

            public LogAppended(LogAppendedData data) {
                super("log_appended");
                this.data = data;
            }
            /** DTO for LogAppendedData. */
            public static final class LogAppendedData {
                public final NodeLogLevel level;
                public final String message;

                public LogAppendedData(NodeLogLevel level, String message) {
                    this.level = level;
                    this.message = message;
                }
            }
        }

        public static final class RoutingChanged extends ProjectedNodeEventKind {
            public final RoutingChangedData data;

            public RoutingChanged(RoutingChangedData data) {
                super("routing_changed");
                this.data = data;
            }
            /** DTO for RoutingChangedData. */
            public static final class RoutingChangedData {
                public final String exit_node;
                public final Boolean is_exit_node;
                public final NodeRoutingMode mode;
                public final String network_name;

                public RoutingChangedData(String exit_node, Boolean is_exit_node, NodeRoutingMode mode, String network_name) {
                    this.exit_node = exit_node;
                    this.is_exit_node = is_exit_node;
                    this.mode = mode;
                    this.network_name = network_name;
                }
            }
        }

        public static final class RoleChanged extends ProjectedNodeEventKind {
            public final RoleChangedData data;

            public RoleChanged(RoleChangedData data) {
                super("role_changed");
                this.data = data;
            }
            /** DTO for RoleChangedData. */
            public static final class RoleChangedData {
                public final Boolean enabled;
                public final String network_name;
                public final NodeNetworkRole role;
                public final List<NodeNetworkRole> roles;

                public RoleChangedData(Boolean enabled, String network_name, NodeNetworkRole role, List<NodeNetworkRole> roles) {
                    this.enabled = enabled;
                    this.network_name = network_name;
                    this.role = role;
                    this.roles = roles;
                }
            }
        }

        public static final class CommandCompleted extends ProjectedNodeEventKind {
            public final CommandCompletedData data;

            public CommandCompleted(CommandCompletedData data) {
                super("command_completed");
                this.data = data;
            }
            /** DTO for CommandCompletedData. */
            public static final class CommandCompletedData {
                public final Long request_id;
                public final Boolean success;

                public CommandCompletedData(Long request_id, Boolean success) {
                    this.request_id = request_id;
                    this.success = success;
                }
            }
        }

        public static final class OperationChanged extends ProjectedNodeEventKind {
            public final OperationChangedData data;

            public OperationChanged(OperationChangedData data) {
                super("operation_changed");
                this.data = data;
            }
            /** DTO for OperationChangedData. */
            public static final class OperationChangedData {
                public final String operation_id;
                public final String stage;
                public final OperationState state;

                public OperationChangedData(String operation_id, String stage, OperationState state) {
                    this.operation_id = operation_id;
                    this.stage = stage;
                    this.state = state;
                }
            }
        }
    }

    /** Tagged union DTO for SubjectSelector. */
    public abstract static class SubjectSelector {
        public final String kind;

        private SubjectSelector(String kind) {
            this.kind = kind;
        }

        public static final class AnyMember extends SubjectSelector {
            public AnyMember() {
                super("any_member");
            }
        }

        public static final class Role extends SubjectSelector {
            public final NetworkRole value;

            public Role(NetworkRole value) {
                super("role");
                this.value = value;
            }
        }

        public static final class Node extends SubjectSelector {
            public final String value;

            public Node(String value) {
                super("node");
                this.value = value;
            }
        }

        public static final class Group extends SubjectSelector {
            public final String value;

            public Group(String value) {
                super("group");
                this.value = value;
            }
        }
    }

    /** Tagged union DTO for TrafficAction. */
    public abstract static class TrafficAction {
        public final String kind;

        private TrafficAction(String kind) {
            this.kind = kind;
        }

        public static final class Direct extends TrafficAction {
            public Direct() {
                super("direct");
            }
        }

        public static final class Network extends TrafficAction {
            public final String value;

            public Network(String value) {
                super("network");
                this.value = value;
            }
        }

        public static final class Exit extends TrafficAction {
            public final String value;

            public Exit(String value) {
                super("exit");
                this.value = value;
            }
        }

        public static final class Reject extends TrafficAction {
            public Reject() {
                super("reject");
            }
        }
    }

    /** Tagged union DTO for TrafficMatchRisk. */
    public abstract static class TrafficMatchRisk {
        public final String kind;

        private TrafficMatchRisk(String kind) {
            this.kind = kind;
        }

        public static final class AlwaysMatches extends TrafficMatchRisk {
            public AlwaysMatches() {
                super("always_matches");
            }
        }

        public static final class AnyDestination extends TrafficMatchRisk {
            public AnyDestination() {
                super("any_destination");
            }
        }

        public static final class AnyPort extends TrafficMatchRisk {
            public AnyPort() {
                super("any_port");
            }
        }

        public static final class AnyProtocol extends TrafficMatchRisk {
            public AnyProtocol() {
                super("any_protocol");
            }
        }

        public static final class AnyApplication extends TrafficMatchRisk {
            public AnyApplication() {
                super("any_application");
            }
        }

        public static final class BroadCidr extends TrafficMatchRisk {
            public final String cidr;
            public final Integer prefix_len;

            public BroadCidr(String cidr, Integer prefix_len) {
                super("broad_cidr");
                this.cidr = cidr;
                this.prefix_len = prefix_len;
            }
        }

        public static final class FullPortRange extends TrafficMatchRisk {
            public final Integer end;
            public final Integer start;

            public FullPortRange(Integer end, Integer start) {
                super("full_port_range");
                this.end = end;
                this.start = start;
            }
        }
    }

    /** Tagged union DTO for TrafficPolicyTestContextSource. */
    public abstract static class TrafficPolicyTestContextSource {
        public final String kind;

        private TrafficPolicyTestContextSource(String kind) {
            this.kind = kind;
        }

        public static final class Manual extends TrafficPolicyTestContextSource {
            public Manual() {
                super("manual");
            }
        }

        public static final class Activity extends TrafficPolicyTestContextSource {
            public final String activity_id;

            public Activity(String activity_id) {
                super("activity");
                this.activity_id = activity_id;
            }
        }

        public static final class Diagnostic extends TrafficPolicyTestContextSource {
            public final String diagnostic_id;

            public Diagnostic(String diagnostic_id) {
                super("diagnostic");
                this.diagnostic_id = diagnostic_id;
            }
        }
    }

    /** Tagged union DTO for TrafficPolicyTestDecision. */
    public abstract static class TrafficPolicyTestDecision {
        public final String visibility;

        private TrafficPolicyTestDecision(String visibility) {
            this.visibility = visibility;
        }

        public static final class Full extends TrafficPolicyTestDecision {
            public final TrafficAction action;
            public final List<TrafficFailureDecision> failure_decisions;
            public final String matched_rule;
            public final TrafficDecisionReason reason;

            public Full(TrafficAction action, List<TrafficFailureDecision> failure_decisions, String matched_rule, TrafficDecisionReason reason) {
                super("full");
                this.action = action;
                this.failure_decisions = failure_decisions;
                this.matched_rule = matched_rule;
                this.reason = reason;
            }
        }

        public static final class RouteSummary extends TrafficPolicyTestDecision {
            public final TrafficAction action;

            public RouteSummary(TrafficAction action) {
                super("route_summary");
                this.action = action;
            }
        }
    }

    /** Parsed command identity; payload JSON remains owned by the platform JSON parser. */
    public static final class CommandEnvelope {
        public final long requestId;
        public final NodeCommandKind command;

        private CommandEnvelope(long requestId, NodeCommandKind command) {
            this.requestId = requestId;
            this.command = command;
        }
    }

    private NodeContractV1() {}

    /** Validate the version, request id and canonical command name before dispatch. */
    public static CommandEnvelope commandEnvelope(
            long schemaVersion, long requestId, String commandName) {
        if (schemaVersion != SCHEMA_VERSION) {
            throw new IllegalArgumentException("unsupported node contract version: " + schemaVersion);
        }
        if (requestId < 0) {
            throw new IllegalArgumentException("request id must be non-negative");
        }
        NodeCommandKind command = NodeCommandKind.fromWireName(commandName);
        if (!isManifestAction(command)) {
            throw new IllegalArgumentException("command is not a user-visible manifest action: " + commandName);
        }
        return new CommandEnvelope(requestId, command);
    }

    public static boolean isManifestAction(NodeCommandKind command) {
        return             command == NodeCommandKind.ACKNOWLEDGE_MEMBERSHIP_POLICY ||
            command == NodeCommandKind.APPLY_CONFIGURATION ||
            command == NodeCommandKind.APPLY_POLICY ||
            command == NodeCommandKind.APPLY_TRAFFIC_POLICY ||
            command == NodeCommandKind.BAN_CLIENT ||
            command == NodeCommandKind.BATCH_REVIEW_JOIN_REQUESTS ||
            command == NodeCommandKind.CHANGE_MEMBER_LIFECYCLE ||
            command == NodeCommandKind.CLEANUP_NODE_RESIDUALS ||
            command == NodeCommandKind.CREATE_INVITATION ||
            command == NodeCommandKind.CREATE_NETWORK ||
            command == NodeCommandKind.GET_BLACKLIST ||
            command == NodeCommandKind.GET_CONFIGURATION ||
            command == NodeCommandKind.GET_CONFIGURATION_HISTORY ||
            command == NodeCommandKind.GET_INVITATION_STATUS ||
            command == NodeCommandKind.GET_NETWORK_ALLOCATION ||
            command == NodeCommandKind.GET_NODE_DIAGNOSTICS ||
            command == NodeCommandKind.GET_POLICY ||
            command == NodeCommandKind.GET_POLICY_HISTORY ||
            command == NodeCommandKind.GET_SNAPSHOT ||
            command == NodeCommandKind.GET_TRAFFIC_POLICY ||
            command == NodeCommandKind.GET_TRAFFIC_POLICY_HISTORY ||
            command == NodeCommandKind.JOIN_NETWORK ||
            command == NodeCommandKind.LEAVE_NETWORK ||
            command == NodeCommandKind.LIST_INVITATIONS ||
            command == NodeCommandKind.LIST_JOIN_REQUESTS ||
            command == NodeCommandKind.LIST_NETWORK_MEMBERS ||
            command == NodeCommandKind.PREVIEW_CONFIGURATION ||
            command == NodeCommandKind.PREVIEW_INVITATION ||
            command == NodeCommandKind.PREVIEW_JOIN_NETWORK ||
            command == NodeCommandKind.PREVIEW_MEMBER_REMOVAL ||
            command == NodeCommandKind.PREVIEW_NETWORK_PROFILE ||
            command == NodeCommandKind.PREVIEW_POLICY ||
            command == NodeCommandKind.PREVIEW_POLICY_ROLLBACK ||
            command == NodeCommandKind.PREVIEW_TRAFFIC_POLICY ||
            command == NodeCommandKind.PREVIEW_TRAFFIC_POLICY_ROLLBACK ||
            command == NodeCommandKind.PROBE_PEER ||
            command == NodeCommandKind.PUBLISH_NETWORK_DISCOVERY ||
            command == NodeCommandKind.REBUILD_TUN ||
            command == NodeCommandKind.REDEEM_INVITATION ||
            command == NodeCommandKind.REFRESH_DISCOVERY_INDEX ||
            command == NodeCommandKind.RESTART_NETWORKS ||
            command == NodeCommandKind.RESTART_NODE ||
            command == NodeCommandKind.RESUBMIT_JOIN_REQUEST ||
            command == NodeCommandKind.RETRY_DISCOVERY_INDEX ||
            command == NodeCommandKind.REVIEW_JOIN_REQUEST ||
            command == NodeCommandKind.REVOKE_INVITATION ||
            command == NodeCommandKind.ROLLBACK_CONFIGURATION ||
            command == NodeCommandKind.ROLLBACK_POLICY ||
            command == NodeCommandKind.ROLLBACK_TRAFFIC_POLICY ||
            command == NodeCommandKind.ROTATE_NODE_IDENTITY ||
            command == NodeCommandKind.SET_NETWORK_ROLE ||
            command == NodeCommandKind.SET_NODE_DISPLAY_NAME ||
            command == NodeCommandKind.SHUTDOWN ||
            command == NodeCommandKind.START_NETWORKS ||
            command == NodeCommandKind.START_NODE ||
            command == NodeCommandKind.STOP_NETWORKS ||
            command == NodeCommandKind.STOP_NODE ||
            command == NodeCommandKind.SUBMIT_JOIN_REQUEST ||
            command == NodeCommandKind.TEST_POLICY ||
            command == NodeCommandKind.TEST_TRAFFIC_POLICY ||
            command == NodeCommandKind.UNBAN_CLIENT ||
            command == NodeCommandKind.UNPUBLISH_NETWORK_DISCOVERY ||
            command == NodeCommandKind.UPDATE_NETWORK_ALLOCATION ||
            command == NodeCommandKind.UPDATE_NETWORK_DISCOVERY ||
            command == NodeCommandKind.UPDATE_NETWORK_PROFILE ||
            command == NodeCommandKind.UPDATE_ROUTING ||
            command == NodeCommandKind.WITHDRAW_JOIN_REQUEST;
    }
}
