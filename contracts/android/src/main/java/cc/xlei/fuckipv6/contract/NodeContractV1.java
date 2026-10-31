// Generated from contracts/schema/*.schema.json. Do not edit by hand.
// Regenerate with scripts/generate-contracts.sh.
package cc.xlei.fuckipv6.contract;

/** Versioned Android-safe bindings for node wire enum values. */
public final class NodeContractV1 {
    public static final int SCHEMA_VERSION = 1;

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

    public enum NodeCommandKind {
        CREATE_NETWORK("create_network"),
        GET_SNAPSHOT("get_snapshot"),
        GET_UI_MANIFEST("get_ui_manifest"),
        JOIN_NETWORK("join_network"),
        PROBE_PEER("probe_peer"),
        SET_NETWORK_ROLE("set_network_role"),
        SHUTDOWN("shutdown"),
        START_NETWORKS("start_networks"),
        STOP_NETWORKS("stop_networks"),
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

    public enum NodeHttpErrorCode {
        SCHEMA_VERSION_MISMATCH("schema_version_mismatch"),
        INVALID_REQUEST("invalid_request"),
        BACKEND_REJECTED("backend_rejected"),
        BACKEND_UNAVAILABLE("backend_unavailable"),
        CONTRACT_VIOLATION("contract_violation");

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
        EXTERNAL_HOST("external_host"),
        NETWORK_NAME("network_name"),
        NETWORK_SELECTION("network_selection"),
        PEER_TARGET("peer_target"),
        PORT("port"),
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
        NETWORK_SELECTION("network_selection"),
        ROLE("role"),
        ROUTING_MODE("routing_mode"),
        POSITIVE_TIMEOUT("positive_timeout");

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
        return             command == NodeCommandKind.CREATE_NETWORK ||
            command == NodeCommandKind.GET_SNAPSHOT ||
            command == NodeCommandKind.JOIN_NETWORK ||
            command == NodeCommandKind.PROBE_PEER ||
            command == NodeCommandKind.SET_NETWORK_ROLE ||
            command == NodeCommandKind.SHUTDOWN ||
            command == NodeCommandKind.START_NETWORKS ||
            command == NodeCommandKind.STOP_NETWORKS ||
            command == NodeCommandKind.UPDATE_ROUTING;
    }
}
