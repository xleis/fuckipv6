// Generated from contracts/schema/*.schema.json. Do not edit by hand.
// Regenerate with scripts/generate-contracts.sh.
package cc.xlei.fuckipv6.contract;

import java.util.List;
import java.util.Map;

/** Versioned Android-safe DTO and enum bindings for the node wire contract. */
public final class NodeContractV1 {
    public static final int SCHEMA_VERSION = 1;
    public static final String[] DTO_CLASS_NAMES = {
        "ActivityEntry", "ConfigurationApplyResult", "ConfigurationDiffEntry", "ConfigurationDocument", "ConfigurationHistoryEntry", "ConfigurationPreview", "ConfigurationValidationIssue", "DomainActorRef", "DomainObjectRef", "ExitNodeCapabilities", "NodeApiCommandResponse", "NodeApiErrorResponse", "NodeApiManifestResponse", "NodeApiObjectResponse", "NodeApiOperationResponse", "NodeApiSnapshotResponse", "NodeApplyConfigurationCommand", "NodeBlacklistSnapshot", "NodeCapabilityRegistrySnapshot", "NodeCapabilitySnapshot", "NodeCleanupFailureSnapshot", "NodeCleanupStepSnapshot", "NodeClientBlacklistCommand", "NodeCommandHttpRequest", "NodeCommandHttpResponse", "NodeCommandRequest", "NodeCommandResult", "NodeConfigurationSnapshot", "NodeCreateNetworkCommand", "NodeCreatedNetwork", "NodeDiagnosticsSnapshot", "NodeEmbeddedServiceSnapshot", "NodeEvent", "NodeEventCursor", "NodeHttpContract", "NodeHttpErrorResponse", "NodeIdentitySnapshot", "NodeInterfaceAddress", "NodeInterfaceSnapshot", "NodeJoinNetworkCommand", "NodeLifecycleSnapshot", "NodeLogEntry", "NodeManifestResponse", "NodeNetworkAllocation", "NodeNetworkAllocationSnapshot", "NodeNetworkConfigSnapshot", "NodeNetworkSnapshot", "NodeOperationSnapshot", "NodePeerProbeSnapshot", "NodePeerSnapshot", "NodePermissionSnapshot", "NodePreviewConfigurationCommand", "NodeProbePeerCommand", "NodeProblem", "NodeProblemDetails", "NodeResidualCleanupResult", "NodeRollbackConfigurationCommand", "NodeRuntimeResidueSnapshot", "NodeSetNetworkRoleCommand", "NodeSnapshot", "NodeSnapshotResponse", "NodeTrafficSnapshot", "NodeUiAction", "NodeUiAudit", "NodeUiConfirmation", "NodeUiField", "NodeUiManifest", "NodeUiPage", "NodeUpdateNetworkAllocationCommand", "NodeUpdateRoutingCommand", "NodeValidationIssue", "OperationError", "OperationRecovery", "UiModeResolution", "NodeApiEvent", "NodeApiObject", "NodeCommand", "NodeCommandOutcome", "NodeEventKind", "NodeNetworkSelection", "NodeWsMessage"
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
        APPLY_CONFIGURATION("apply_configuration"),
        BAN_CLIENT("ban_client"),
        CLEANUP_NODE_RESIDUALS("cleanup_node_residuals"),
        CREATE_NETWORK("create_network"),
        GET_BLACKLIST("get_blacklist"),
        GET_CONFIGURATION("get_configuration"),
        GET_CONFIGURATION_HISTORY("get_configuration_history"),
        GET_NETWORK_ALLOCATION("get_network_allocation"),
        GET_NODE_DIAGNOSTICS("get_node_diagnostics"),
        GET_SNAPSHOT("get_snapshot"),
        GET_UI_MANIFEST("get_ui_manifest"),
        JOIN_NETWORK("join_network"),
        PREVIEW_CONFIGURATION("preview_configuration"),
        PROBE_PEER("probe_peer"),
        REBUILD_TUN("rebuild_tun"),
        RESTART_NETWORKS("restart_networks"),
        RESTART_NODE("restart_node"),
        ROLLBACK_CONFIGURATION("rollback_configuration"),
        SET_NETWORK_ROLE("set_network_role"),
        SHUTDOWN("shutdown"),
        START_NETWORKS("start_networks"),
        START_NODE("start_node"),
        STOP_NETWORKS("stop_networks"),
        STOP_NODE("stop_node"),
        UNBAN_CLIENT("unban_client"),
        UPDATE_NETWORK_ALLOCATION("update_network_allocation"),
        UPDATE_ROUTING("update_routing");

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
        EXTERNAL_HOST("external_host"),
        NETWORK_MODE("network_mode"),
        NETWORK_NAME("network_name"),
        NETWORK_SELECTION("network_selection"),
        PEER_TARGET("peer_target"),
        PORT("port"),
        REVISION("revision"),
        ROLE("role"),
        ROUTING_MODE("routing_mode"),
        URL_OR_TICKET("url_or_ticket"),
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
        CONFIGURATION_JSON("configuration_json");

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
        NETWORK_CREATE("network_create"),
        NETWORK_APPLY("network_apply"),
        TUN_REBUILD("tun_rebuild"),
        NETWORK_LEAVE("network_leave"),
        MEMBERSHIP_APPROVE("membership_approve"),
        MEMBERSHIP_REJECT("membership_reject"),
        MEMBERSHIP_REMOVE("membership_remove"),
        POLICY_TEST("policy_test"),
        POLICY_APPLY("policy_apply"),
        POLICY_ROLLBACK("policy_rollback"),
        TRAFFIC_RULES_APPLY("traffic_rules_apply"),
        NETWORK_ROLE_UPDATE("network_role_update"),
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
        public final String audit_id;
        public final Map<String, Object> details;
        public final String network_id;
        public final Long occurred_at_unix_ms;
        public final String operation_id;
        public final AuditOutcome outcome;
        public final String resource;
        public final String summary;

        public ActivityEntry(String action, String actor, String audit_id, Map<String, Object> details, String network_id, Long occurred_at_unix_ms, String operation_id, AuditOutcome outcome, String resource, String summary) {
            this.action = action;
            this.actor = actor;
            this.audit_id = audit_id;
            this.details = details;
            this.network_id = network_id;
            this.occurred_at_unix_ms = occurred_at_unix_ms;
            this.operation_id = operation_id;
            this.outcome = outcome;
            this.resource = resource;
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

    /** DTO for NodeApiCommandResponse. */
    public static final class NodeApiCommandResponse {
        public final NodeOperationSnapshot operation;
        public final Long request_id;
        public final NodeCommandResult result;
        public final Integer schema_version;
        public final String server_time;

        public NodeApiCommandResponse(NodeOperationSnapshot operation, Long request_id, NodeCommandResult result, Integer schema_version, String server_time) {
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
        public final NodeSnapshot snapshot;

        public NodeApiSnapshotResponse(Long request_id, Integer schema_version, String server_time, NodeSnapshot snapshot) {
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
        public final NodeCommandResult result;
        public final Integer schema_version;

        public NodeCommandHttpResponse(NodeCommandResult result, Integer schema_version) {
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

    /** DTO for NodeCreateNetworkCommand. */
    public static final class NodeCreateNetworkCommand {
        public final Boolean auto_connect;
        public final String external_host;
        public final String name;
        public final Integer relay_port;
        public final Integer signaling_port;

        public NodeCreateNetworkCommand(Boolean auto_connect, String external_host, String name, Integer relay_port, Integer signaling_port) {
            this.auto_connect = auto_connect;
            this.external_host = external_host;
            this.name = name;
            this.relay_port = relay_port;
            this.signaling_port = signaling_port;
        }
    }

    /** DTO for NodeCreatedNetwork. */
    public static final class NodeCreatedNetwork {
        public final Boolean config_saved;
        public final String invitation_ticket;
        public final String network_name;
        public final Boolean started;

        public NodeCreatedNetwork(Boolean config_saved, String invitation_ticket, String network_name, Boolean started) {
            this.config_saved = config_saved;
            this.invitation_ticket = invitation_ticket;
            this.network_name = network_name;
            this.started = started;
        }
    }

    /** DTO for NodeDiagnosticsSnapshot. */
    public static final class NodeDiagnosticsSnapshot {
        public final List<String> cleanup_candidate_networks;
        public final NodeLifecycleSnapshot lifecycle;
        public final NodeRuntimeResidueSnapshot residue;
        public final List<String> retained_owner_networks;

        public NodeDiagnosticsSnapshot(List<String> cleanup_candidate_networks, NodeLifecycleSnapshot lifecycle, NodeRuntimeResidueSnapshot residue, List<String> retained_owner_networks) {
            this.cleanup_candidate_networks = cleanup_candidate_networks;
            this.lifecycle = lifecycle;
            this.residue = residue;
            this.retained_owner_networks = retained_owner_networks;
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

    /** DTO for NodeIdentitySnapshot. */
    public static final class NodeIdentitySnapshot {
        public final String display_name;
        public final String node_id;
        public final String public_key_fingerprint;

        public NodeIdentitySnapshot(String display_name, String node_id, String public_key_fingerprint) {
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
        public final String network_name;
        public final Boolean start;
        public final String target;

        public NodeJoinNetworkCommand(String network_name, Boolean start, String target) {
            this.network_name = network_name;
            this.start = start;
            this.target = target;
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
        public final NodeCommandResult result;
        public final String stage;
        public final OperationState state;
        public final DomainObjectRef target;
        public final Long updated_at_unix_ms;

        public NodeOperationSnapshot(DomainActorRef actor, Long attempt, String cancel_blocked_reason, Boolean cancellable, Long created_at_unix_ms, OperationError error, String idempotency_key, OperationKind kind, Long max_attempts, String operation_id, NodeCommandResult result, String stage, OperationState state, DomainObjectRef target, Long updated_at_unix_ms) {
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

    /** DTO for NodeRollbackConfigurationCommand. */
    public static final class NodeRollbackConfigurationCommand {
        public final Long expected_revision;
        public final Long target_revision;

        public NodeRollbackConfigurationCommand(Long expected_revision, Long target_revision) {
            this.expected_revision = expected_revision;
            this.target_revision = target_revision;
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

    /** DTO for NodeSnapshot. */
    public static final class NodeSnapshot {
        public final NodeCapabilityRegistrySnapshot capabilities;
        public final NodeConfigurationSnapshot configuration;
        public final NodeControlHealth control_health;
        public final List<NodeEmbeddedServiceSnapshot> embedded_services;
        public final ExitNodeCapabilities exit_node_capabilities;
        public final NodeIdentitySnapshot identity;
        public final List<NodeProblem> interface_problems;
        public final List<NodeInterfaceSnapshot> interfaces;
        public final NodeProblem last_error;
        public final NodeLifecycleSnapshot lifecycle;
        public final List<NodeLogEntry> logs;
        public final List<NodeNetworkSnapshot> networks;
        public final List<NodePeerSnapshot> peers;
        public final NodePermissionSnapshot permissions;
        public final List<ActivityEntry> recent_activity;
        public final Long revision;
        public final UiModeResolution ui_mode;

        public NodeSnapshot(NodeCapabilityRegistrySnapshot capabilities, NodeConfigurationSnapshot configuration, NodeControlHealth control_health, List<NodeEmbeddedServiceSnapshot> embedded_services, ExitNodeCapabilities exit_node_capabilities, NodeIdentitySnapshot identity, List<NodeProblem> interface_problems, List<NodeInterfaceSnapshot> interfaces, NodeProblem last_error, NodeLifecycleSnapshot lifecycle, List<NodeLogEntry> logs, List<NodeNetworkSnapshot> networks, List<NodePeerSnapshot> peers, NodePermissionSnapshot permissions, List<ActivityEntry> recent_activity, Long revision, UiModeResolution ui_mode) {
            this.capabilities = capabilities;
            this.configuration = configuration;
            this.control_health = control_health;
            this.embedded_services = embedded_services;
            this.exit_node_capabilities = exit_node_capabilities;
            this.identity = identity;
            this.interface_problems = interface_problems;
            this.interfaces = interfaces;
            this.last_error = last_error;
            this.lifecycle = lifecycle;
            this.logs = logs;
            this.networks = networks;
            this.peers = peers;
            this.permissions = permissions;
            this.recent_activity = recent_activity;
            this.revision = revision;
            this.ui_mode = ui_mode;
        }
    }

    /** DTO for NodeSnapshotResponse. */
    public static final class NodeSnapshotResponse {
        public final Integer schema_version;
        public final NodeSnapshot snapshot;

        public NodeSnapshotResponse(Integer schema_version, NodeSnapshot snapshot) {
            this.schema_version = schema_version;
            this.snapshot = snapshot;
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
        public final String title_key;

        public NodeUiConfirmation(String consequence_key, Boolean destructive, String title_key) {
            this.consequence_key = consequence_key;
            this.destructive = destructive;
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
        public final List<NodeUiPage> pages;
        public final Integer schema_version;

        public NodeUiManifest(List<NodeUiPage> pages, Integer schema_version) {
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
            public final NodeSnapshot snapshot;

            public Snapshot(NodeEventCursor cursor, Long request_id, Integer schema_version, String server_time, NodeSnapshot snapshot) {
                super("snapshot");
                this.cursor = cursor;
                this.request_id = request_id;
                this.schema_version = schema_version;
                this.server_time = server_time;
                this.snapshot = snapshot;
            }
        }

        public static final class Event extends NodeApiEvent {
            public final NodeEvent event;
            public final Long request_id;
            public final Integer schema_version;
            public final String server_time;

            public Event(NodeEvent event, Long request_id, Integer schema_version, String server_time) {
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
            public final NodeIdentitySnapshot identity;

            public Node(String id, NodeIdentitySnapshot identity) {
                super("node");
                this.id = id;
                this.identity = identity;
            }
        }

        public static final class Network extends NodeApiObject {
            public final String id;
            public final NodeNetworkSnapshot network;

            public Network(String id, NodeNetworkSnapshot network) {
                super("network");
                this.id = id;
                this.network = network;
            }
        }

        public static final class Peer extends NodeApiObject {
            public final String id;
            public final NodePeerSnapshot peer;

            public Peer(String id, NodePeerSnapshot peer) {
                super("peer");
                this.id = id;
                this.peer = peer;
            }
        }

        public static final class EmbeddedService extends NodeApiObject {
            public final String id;
            public final NodeEmbeddedServiceSnapshot service;

            public EmbeddedService(String id, NodeEmbeddedServiceSnapshot service) {
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

        public static final class JoinNetwork extends NodeCommand {
            public final NodeJoinNetworkCommand payload;

            public JoinNetwork(NodeJoinNetworkCommand payload) {
                super(NodeCommandKind.JOIN_NETWORK);
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

    /** Tagged union DTO for NodeWsMessage. */
    public abstract static class NodeWsMessage {
        public final String type;

        private NodeWsMessage(String type) {
            this.type = type;
        }

        public static final class Snapshot extends NodeWsMessage {
            public final Integer schema_version;
            public final NodeSnapshot snapshot;

            public Snapshot(Integer schema_version, NodeSnapshot snapshot) {
                super("snapshot");
                this.schema_version = schema_version;
                this.snapshot = snapshot;
            }
        }

        public static final class Event extends NodeWsMessage {
            public final NodeEvent event;
            public final Integer schema_version;

            public Event(NodeEvent event, Integer schema_version) {
                super("event");
                this.event = event;
                this.schema_version = schema_version;
            }
        }

        public static final class Resync extends NodeWsMessage {
            public final String reason;
            public final Integer schema_version;
            public final NodeSnapshot snapshot;

            public Resync(String reason, Integer schema_version, NodeSnapshot snapshot) {
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
        return             command == NodeCommandKind.APPLY_CONFIGURATION ||
            command == NodeCommandKind.BAN_CLIENT ||
            command == NodeCommandKind.CLEANUP_NODE_RESIDUALS ||
            command == NodeCommandKind.CREATE_NETWORK ||
            command == NodeCommandKind.GET_BLACKLIST ||
            command == NodeCommandKind.GET_CONFIGURATION ||
            command == NodeCommandKind.GET_CONFIGURATION_HISTORY ||
            command == NodeCommandKind.GET_NETWORK_ALLOCATION ||
            command == NodeCommandKind.GET_NODE_DIAGNOSTICS ||
            command == NodeCommandKind.GET_SNAPSHOT ||
            command == NodeCommandKind.JOIN_NETWORK ||
            command == NodeCommandKind.PREVIEW_CONFIGURATION ||
            command == NodeCommandKind.PROBE_PEER ||
            command == NodeCommandKind.REBUILD_TUN ||
            command == NodeCommandKind.RESTART_NETWORKS ||
            command == NodeCommandKind.RESTART_NODE ||
            command == NodeCommandKind.ROLLBACK_CONFIGURATION ||
            command == NodeCommandKind.SET_NETWORK_ROLE ||
            command == NodeCommandKind.SHUTDOWN ||
            command == NodeCommandKind.START_NETWORKS ||
            command == NodeCommandKind.START_NODE ||
            command == NodeCommandKind.STOP_NETWORKS ||
            command == NodeCommandKind.STOP_NODE ||
            command == NodeCommandKind.UNBAN_CLIENT ||
            command == NodeCommandKind.UPDATE_NETWORK_ALLOCATION ||
            command == NodeCommandKind.UPDATE_ROUTING;
    }
}
