// Generated from contracts/schema/*.schema.json. Do not edit by hand.
// Regenerate with scripts/generate-contracts.sh.
package cc.xlei.fuckipv6.contract;

import java.util.List;
import java.util.Map;

/** Versioned Android-safe DTO and enum bindings for the node wire contract. */
public final class NodeContractV1 {
    public static final int SCHEMA_VERSION = 1;
    public static final String[] DTO_CLASS_NAMES = {
        "ExitNodeCapabilities", "NodeCapabilityRegistrySnapshot", "NodeCapabilitySnapshot", "NodeCommandHttpRequest", "NodeCommandHttpResponse", "NodeCommandRequest", "NodeCommandResult", "NodeConfigurationSnapshot", "NodeCreateNetworkCommand", "NodeEmbeddedServiceSnapshot", "NodeEvent", "NodeHttpContract", "NodeHttpErrorResponse", "NodeIdentitySnapshot", "NodeJoinNetworkCommand", "NodeLifecycleSnapshot", "NodeLogEntry", "NodeManifestResponse", "NodeNetworkConfigSnapshot", "NodeNetworkSnapshot", "NodePeerProbeSnapshot", "NodePeerSnapshot", "NodePermissionSnapshot", "NodeProbePeerCommand", "NodeProblem", "NodeSetNetworkRoleCommand", "NodeSnapshot", "NodeSnapshotResponse", "NodeTrafficSnapshot", "NodeUiAction", "NodeUiAudit", "NodeUiConfirmation", "NodeUiField", "NodeUiManifest", "NodeUiPage", "NodeUpdateRoutingCommand", "UiModeResolution", "NodeCommand", "NodeCommandOutcome", "NodeEventKind", "NodeNetworkSelection", "NodeWsMessage"
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
        RESTART_NETWORKS("restart_networks"),
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

    public enum NodeProblemCode {
        INVALID_REQUEST("invalid_request"),
        NOT_FOUND("not_found"),
        CONFLICT("conflict"),
        UNAVAILABLE("unavailable"),
        PERMISSION_DENIED("permission_denied"),
        CANCELLED("cancelled"),
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

    /** DTO for ExitNodeCapabilities. */
    public static final class ExitNodeCapabilities {
        public final Integer schema_version;
        public final ExitNodePlatform platform;
        public final Boolean supported;
        public final Boolean ipv4_forwarding;
        public final Boolean ipv6_forwarding;
        public final Boolean nat;
        public final Boolean cleanup;
        public final List<ExitNodeUnsupportedReason> unsupported_reasons;

        public ExitNodeCapabilities(Integer schema_version, ExitNodePlatform platform, Boolean supported, Boolean ipv4_forwarding, Boolean ipv6_forwarding, Boolean nat, Boolean cleanup, List<ExitNodeUnsupportedReason> unsupported_reasons) {
            this.schema_version = schema_version;
            this.platform = platform;
            this.supported = supported;
            this.ipv4_forwarding = ipv4_forwarding;
            this.ipv6_forwarding = ipv6_forwarding;
            this.nat = nat;
            this.cleanup = cleanup;
            this.unsupported_reasons = unsupported_reasons;
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
        public final CapabilityState state;

        public NodeCapabilitySnapshot(CapabilityKind kind, CapabilityState state) {
            this.kind = kind;
            this.state = state;
        }
    }

    /** DTO for NodeCommandHttpRequest. */
    public static final class NodeCommandHttpRequest {
        public final Integer schema_version;
        public final NodeCommandRequest request;

        public NodeCommandHttpRequest(Integer schema_version, NodeCommandRequest request) {
            this.schema_version = schema_version;
            this.request = request;
        }
    }

    /** DTO for NodeCommandHttpResponse. */
    public static final class NodeCommandHttpResponse {
        public final Integer schema_version;
        public final NodeCommandResult result;

        public NodeCommandHttpResponse(Integer schema_version, NodeCommandResult result) {
            this.schema_version = schema_version;
            this.result = result;
        }
    }

    /** DTO for NodeCommandRequest. */
    public static final class NodeCommandRequest {
        public final Long request_id;
        public final NodeCommand command;

        public NodeCommandRequest(Long request_id, NodeCommand command) {
            this.request_id = request_id;
            this.command = command;
        }
    }

    /** DTO for NodeCommandResult. */
    public static final class NodeCommandResult {
        public final Long request_id;
        public final NodeCommandOutcome outcome;

        public NodeCommandResult(Long request_id, NodeCommandOutcome outcome) {
            this.request_id = request_id;
            this.outcome = outcome;
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
        public final String name;
        public final Integer signaling_port;
        public final Integer relay_port;
        public final String external_host;
        public final Boolean auto_connect;

        public NodeCreateNetworkCommand(String name, Integer signaling_port, Integer relay_port, String external_host, Boolean auto_connect) {
            this.name = name;
            this.signaling_port = signaling_port;
            this.relay_port = relay_port;
            this.external_host = external_host;
            this.auto_connect = auto_connect;
        }
    }

    /** DTO for NodeEmbeddedServiceSnapshot. */
    public static final class NodeEmbeddedServiceSnapshot {
        public final String network_name;
        public final NodeEmbeddedServiceStatus status;
        public final String signaling_addr;
        public final String relay_addr;

        public NodeEmbeddedServiceSnapshot(String network_name, NodeEmbeddedServiceStatus status, String signaling_addr, String relay_addr) {
            this.network_name = network_name;
            this.status = status;
            this.signaling_addr = signaling_addr;
            this.relay_addr = relay_addr;
        }
    }

    /** DTO for NodeEvent. */
    public static final class NodeEvent {
        public final Long sequence;
        public final NodeEventKind kind;

        public NodeEvent(Long sequence, NodeEventKind kind) {
            this.sequence = sequence;
            this.kind = kind;
        }
    }

    /** DTO for NodeHttpContract. */
    public static final class NodeHttpContract {
        public final NodeCommandHttpRequest command_request;
        public final NodeCommandHttpResponse command_response;
        public final NodeManifestResponse manifest_response;
        public final NodeSnapshotResponse snapshot_response;
        public final NodeHttpErrorResponse error_response;
        public final NodeWsMessage websocket_message;

        public NodeHttpContract(NodeCommandHttpRequest command_request, NodeCommandHttpResponse command_response, NodeManifestResponse manifest_response, NodeSnapshotResponse snapshot_response, NodeHttpErrorResponse error_response, NodeWsMessage websocket_message) {
            this.command_request = command_request;
            this.command_response = command_response;
            this.manifest_response = manifest_response;
            this.snapshot_response = snapshot_response;
            this.error_response = error_response;
            this.websocket_message = websocket_message;
        }
    }

    /** DTO for NodeHttpErrorResponse. */
    public static final class NodeHttpErrorResponse {
        public final Integer schema_version;
        public final NodeHttpErrorCode code;
        public final String message;
        public final Integer requested_schema_version;
        public final Integer supported_schema_version;
        public final NodeProblem problem;

        public NodeHttpErrorResponse(Integer schema_version, NodeHttpErrorCode code, String message, Integer requested_schema_version, Integer supported_schema_version, NodeProblem problem) {
            this.schema_version = schema_version;
            this.code = code;
            this.message = message;
            this.requested_schema_version = requested_schema_version;
            this.supported_schema_version = supported_schema_version;
            this.problem = problem;
        }
    }

    /** DTO for NodeIdentitySnapshot. */
    public static final class NodeIdentitySnapshot {
        public final String node_id;
        public final String display_name;
        public final String public_key_fingerprint;

        public NodeIdentitySnapshot(String node_id, String display_name, String public_key_fingerprint) {
            this.node_id = node_id;
            this.display_name = display_name;
            this.public_key_fingerprint = public_key_fingerprint;
        }
    }

    /** DTO for NodeJoinNetworkCommand. */
    public static final class NodeJoinNetworkCommand {
        public final String target;
        public final String network_name;
        public final Boolean start;

        public NodeJoinNetworkCommand(String target, String network_name, Boolean start) {
            this.target = target;
            this.network_name = network_name;
            this.start = start;
        }
    }

    /** DTO for NodeLifecycleSnapshot. */
    public static final class NodeLifecycleSnapshot {
        public final NodePhase phase;
        public final Long configured_networks;
        public final Long active_networks;
        public final List<String> failed_networks;

        public NodeLifecycleSnapshot(NodePhase phase, Long configured_networks, Long active_networks, List<String> failed_networks) {
            this.phase = phase;
            this.configured_networks = configured_networks;
            this.active_networks = active_networks;
            this.failed_networks = failed_networks;
        }
    }

    /** DTO for NodeLogEntry. */
    public static final class NodeLogEntry {
        public final Long unix_ms;
        public final NodeLogLevel level;
        public final String target;
        public final String message;

        public NodeLogEntry(Long unix_ms, NodeLogLevel level, String target, String message) {
            this.unix_ms = unix_ms;
            this.level = level;
            this.target = target;
            this.message = message;
        }
    }

    /** DTO for NodeManifestResponse. */
    public static final class NodeManifestResponse {
        public final Integer schema_version;
        public final NodeUiManifest manifest;

        public NodeManifestResponse(Integer schema_version, NodeUiManifest manifest) {
            this.schema_version = schema_version;
            this.manifest = manifest;
        }
    }

    /** DTO for NodeNetworkConfigSnapshot. */
    public static final class NodeNetworkConfigSnapshot {
        public final String network_name;
        public final String coordinator_url;
        public final NetworkMode virtual_mode;
        public final String preferred_ipv4;
        public final String preferred_ipv6;
        public final String virtual_network;
        public final String tun_name;
        public final Integer mtu;
        public final Integer p2p_listen_port;
        public final Long p2p_connect_timeout_secs;
        public final Long p2p_heartbeat_interval_secs;
        public final NodeRoutingMode routing_mode;
        public final String exit_node;
        public final List<String> direct_cidrs;
        public final List<String> proxy_cidrs;
        public final Boolean exit_node_enabled;
        public final List<String> exit_node_allowed_clients;
        public final Boolean exit_node_allow_lan_access;
        public final String exit_node_region;
        public final Boolean acl_enabled;
        public final AclAction acl_default_action;
        public final Long acl_rule_count;
        public final String log_level;
        public final Boolean log_file_configured;
        public final Boolean identity_configured;

        public NodeNetworkConfigSnapshot(String network_name, String coordinator_url, NetworkMode virtual_mode, String preferred_ipv4, String preferred_ipv6, String virtual_network, String tun_name, Integer mtu, Integer p2p_listen_port, Long p2p_connect_timeout_secs, Long p2p_heartbeat_interval_secs, NodeRoutingMode routing_mode, String exit_node, List<String> direct_cidrs, List<String> proxy_cidrs, Boolean exit_node_enabled, List<String> exit_node_allowed_clients, Boolean exit_node_allow_lan_access, String exit_node_region, Boolean acl_enabled, AclAction acl_default_action, Long acl_rule_count, String log_level, Boolean log_file_configured, Boolean identity_configured) {
            this.network_name = network_name;
            this.coordinator_url = coordinator_url;
            this.virtual_mode = virtual_mode;
            this.preferred_ipv4 = preferred_ipv4;
            this.preferred_ipv6 = preferred_ipv6;
            this.virtual_network = virtual_network;
            this.tun_name = tun_name;
            this.mtu = mtu;
            this.p2p_listen_port = p2p_listen_port;
            this.p2p_connect_timeout_secs = p2p_connect_timeout_secs;
            this.p2p_heartbeat_interval_secs = p2p_heartbeat_interval_secs;
            this.routing_mode = routing_mode;
            this.exit_node = exit_node;
            this.direct_cidrs = direct_cidrs;
            this.proxy_cidrs = proxy_cidrs;
            this.exit_node_enabled = exit_node_enabled;
            this.exit_node_allowed_clients = exit_node_allowed_clients;
            this.exit_node_allow_lan_access = exit_node_allow_lan_access;
            this.exit_node_region = exit_node_region;
            this.acl_enabled = acl_enabled;
            this.acl_default_action = acl_default_action;
            this.acl_rule_count = acl_rule_count;
            this.log_level = log_level;
            this.log_file_configured = log_file_configured;
            this.identity_configured = identity_configured;
        }
    }

    /** DTO for NodeNetworkSnapshot. */
    public static final class NodeNetworkSnapshot {
        public final String name;
        public final String tun_name;
        public final NodeNetworkStatus status;
        public final String assigned_ipv6;
        public final String assigned_ipv4;
        public final String p2p_listen_addr;
        public final Long uptime_secs;
        public final List<NodeNetworkRole> roles;
        public final Long peer_count;
        public final NodeTrafficSnapshot traffic;

        public NodeNetworkSnapshot(String name, String tun_name, NodeNetworkStatus status, String assigned_ipv6, String assigned_ipv4, String p2p_listen_addr, Long uptime_secs, List<NodeNetworkRole> roles, Long peer_count, NodeTrafficSnapshot traffic) {
            this.name = name;
            this.tun_name = tun_name;
            this.status = status;
            this.assigned_ipv6 = assigned_ipv6;
            this.assigned_ipv4 = assigned_ipv4;
            this.p2p_listen_addr = p2p_listen_addr;
            this.uptime_secs = uptime_secs;
            this.roles = roles;
            this.peer_count = peer_count;
            this.traffic = traffic;
        }
    }

    /** DTO for NodePeerProbeSnapshot. */
    public static final class NodePeerProbeSnapshot {
        public final String peer_id;
        public final String virtual_ipv6;
        public final String endpoint;
        public final Long rtt_us;
        public final Boolean p2p_active;
        public final Boolean timed_out;

        public NodePeerProbeSnapshot(String peer_id, String virtual_ipv6, String endpoint, Long rtt_us, Boolean p2p_active, Boolean timed_out) {
            this.peer_id = peer_id;
            this.virtual_ipv6 = virtual_ipv6;
            this.endpoint = endpoint;
            this.rtt_us = rtt_us;
            this.p2p_active = p2p_active;
            this.timed_out = timed_out;
        }
    }

    /** DTO for NodePeerSnapshot. */
    public static final class NodePeerSnapshot {
        public final String peer_id;
        public final String network_name;
        public final String virtual_ipv6;
        public final String virtual_ipv4;
        public final List<String> endpoints;
        public final Boolean online;
        public final Boolean p2p_active;
        public final Boolean p2p_healthy;
        public final Long rtt_us;
        public final Double loss_rate;
        public final NodeTrafficSnapshot traffic;

        public NodePeerSnapshot(String peer_id, String network_name, String virtual_ipv6, String virtual_ipv4, List<String> endpoints, Boolean online, Boolean p2p_active, Boolean p2p_healthy, Long rtt_us, Double loss_rate, NodeTrafficSnapshot traffic) {
            this.peer_id = peer_id;
            this.network_name = network_name;
            this.virtual_ipv6 = virtual_ipv6;
            this.virtual_ipv4 = virtual_ipv4;
            this.endpoints = endpoints;
            this.online = online;
            this.p2p_active = p2p_active;
            this.p2p_healthy = p2p_healthy;
            this.rtt_us = rtt_us;
            this.loss_rate = loss_rate;
            this.traffic = traffic;
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
        public final NodeProblemCode code;
        public final String message;
        public final String network_name;
        public final Boolean retryable;

        public NodeProblem(NodeProblemCode code, String message, String network_name, Boolean retryable) {
            this.code = code;
            this.message = message;
            this.network_name = network_name;
            this.retryable = retryable;
        }
    }

    /** DTO for NodeSetNetworkRoleCommand. */
    public static final class NodeSetNetworkRoleCommand {
        public final String network_name;
        public final NodeNetworkRole role;
        public final Boolean enabled;

        public NodeSetNetworkRoleCommand(String network_name, NodeNetworkRole role, Boolean enabled) {
            this.network_name = network_name;
            this.role = role;
            this.enabled = enabled;
        }
    }

    /** DTO for NodeSnapshot. */
    public static final class NodeSnapshot {
        public final Long revision;
        public final NodeLifecycleSnapshot lifecycle;
        public final NodeControlHealth control_health;
        public final NodeIdentitySnapshot identity;
        public final NodeConfigurationSnapshot configuration;
        public final ExitNodeCapabilities exit_node_capabilities;
        public final NodeCapabilityRegistrySnapshot capabilities;
        public final NodePermissionSnapshot permissions;
        public final List<NodeEmbeddedServiceSnapshot> embedded_services;
        public final UiModeResolution ui_mode;
        public final List<NodeNetworkSnapshot> networks;
        public final List<NodePeerSnapshot> peers;
        public final List<NodeLogEntry> logs;
        public final NodeProblem last_error;

        public NodeSnapshot(Long revision, NodeLifecycleSnapshot lifecycle, NodeControlHealth control_health, NodeIdentitySnapshot identity, NodeConfigurationSnapshot configuration, ExitNodeCapabilities exit_node_capabilities, NodeCapabilityRegistrySnapshot capabilities, NodePermissionSnapshot permissions, List<NodeEmbeddedServiceSnapshot> embedded_services, UiModeResolution ui_mode, List<NodeNetworkSnapshot> networks, List<NodePeerSnapshot> peers, List<NodeLogEntry> logs, NodeProblem last_error) {
            this.revision = revision;
            this.lifecycle = lifecycle;
            this.control_health = control_health;
            this.identity = identity;
            this.configuration = configuration;
            this.exit_node_capabilities = exit_node_capabilities;
            this.capabilities = capabilities;
            this.permissions = permissions;
            this.embedded_services = embedded_services;
            this.ui_mode = ui_mode;
            this.networks = networks;
            this.peers = peers;
            this.logs = logs;
            this.last_error = last_error;
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
        public final Long tun_rx_packets;
        public final Long tun_rx_bytes;
        public final Long tun_tx_packets;
        public final Long tun_tx_bytes;
        public final Long p2p_rx_packets;
        public final Long p2p_rx_bytes;
        public final Long p2p_tx_packets;
        public final Long p2p_tx_bytes;
        public final Long error_count;

        public NodeTrafficSnapshot(Long tun_rx_packets, Long tun_rx_bytes, Long tun_tx_packets, Long tun_tx_bytes, Long p2p_rx_packets, Long p2p_rx_bytes, Long p2p_tx_packets, Long p2p_tx_bytes, Long error_count) {
            this.tun_rx_packets = tun_rx_packets;
            this.tun_rx_bytes = tun_rx_bytes;
            this.tun_tx_packets = tun_tx_packets;
            this.tun_tx_bytes = tun_tx_bytes;
            this.p2p_rx_packets = p2p_rx_packets;
            this.p2p_rx_bytes = p2p_rx_bytes;
            this.p2p_tx_packets = p2p_tx_packets;
            this.p2p_tx_bytes = p2p_tx_bytes;
            this.error_count = error_count;
        }
    }

    /** DTO for NodeUiAction. */
    public static final class NodeUiAction {
        public final String id;
        public final String label_key;
        public final String command;
        public final NodeUiBackendStatus backend_status;
        public final NodePermission permission;
        public final NodeUiAudit audit;
        public final NodeUiAvailability availability;
        public final List<NodeUiField> fields;
        public final NodeUiConfirmation confirmation;

        public NodeUiAction(String id, String label_key, String command, NodeUiBackendStatus backend_status, NodePermission permission, NodeUiAudit audit, NodeUiAvailability availability, List<NodeUiField> fields, NodeUiConfirmation confirmation) {
            this.id = id;
            this.label_key = label_key;
            this.command = command;
            this.backend_status = backend_status;
            this.permission = permission;
            this.audit = audit;
            this.availability = availability;
            this.fields = fields;
            this.confirmation = confirmation;
        }
    }

    /** DTO for NodeUiAudit. */
    public static final class NodeUiAudit {
        public final NodeUiAuditPolicy policy;
        public final String action;

        public NodeUiAudit(NodeUiAuditPolicy policy, String action) {
            this.policy = policy;
            this.action = action;
        }
    }

    /** DTO for NodeUiConfirmation. */
    public static final class NodeUiConfirmation {
        public final String title_key;
        public final String consequence_key;
        public final Boolean destructive;

        public NodeUiConfirmation(String title_key, String consequence_key, Boolean destructive) {
            this.title_key = title_key;
            this.consequence_key = consequence_key;
            this.destructive = destructive;
        }
    }

    /** DTO for NodeUiField. */
    public static final class NodeUiField {
        public final String id;
        public final String label_key;
        public final NodeUiInputKind input;
        public final Boolean required;
        public final NodeUiValidation validation;

        public NodeUiField(String id, String label_key, NodeUiInputKind input, Boolean required, NodeUiValidation validation) {
            this.id = id;
            this.label_key = label_key;
            this.input = input;
            this.required = required;
            this.validation = validation;
        }
    }

    /** DTO for NodeUiManifest. */
    public static final class NodeUiManifest {
        public final Integer schema_version;
        public final List<NodeUiPage> pages;

        public NodeUiManifest(Integer schema_version, List<NodeUiPage> pages) {
            this.schema_version = schema_version;
            this.pages = pages;
        }
    }

    /** DTO for NodeUiPage. */
    public static final class NodeUiPage {
        public final String id;
        public final String nav_label_key;
        public final String title_key;
        public final List<NodeUiAction> actions;

        public NodeUiPage(String id, String nav_label_key, String title_key, List<NodeUiAction> actions) {
            this.id = id;
            this.nav_label_key = nav_label_key;
            this.title_key = title_key;
            this.actions = actions;
        }
    }

    /** DTO for NodeUpdateRoutingCommand. */
    public static final class NodeUpdateRoutingCommand {
        public final String network_name;
        public final NodeRoutingMode mode;
        public final String exit_node;
        public final Boolean is_exit_node;

        public NodeUpdateRoutingCommand(String network_name, NodeRoutingMode mode, String exit_node, Boolean is_exit_node) {
            this.network_name = network_name;
            this.mode = mode;
            this.exit_node = exit_node;
            this.is_exit_node = is_exit_node;
        }
    }

    /** DTO for UiModeResolution. */
    public static final class UiModeResolution {
        public final UiModeRequest requested;
        public final UiMode effective;
        public final DisplayEnvironment display;
        public final Boolean gpui_available;

        public UiModeResolution(UiModeRequest requested, UiMode effective, DisplayEnvironment display, Boolean gpui_available) {
            this.requested = requested;
            this.effective = effective;
            this.display = display;
            this.gpui_available = gpui_available;
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
                public final List<String> networks;
                public final Boolean all;

                public StopNetworksPayload(List<String> networks, Boolean all) {
                    this.networks = networks;
                    this.all = all;
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
                public final List<String> networks;
                public final Boolean all;

                public RestartNetworksPayload(List<String> networks, Boolean all) {
                    this.networks = networks;
                    this.all = all;
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

        public static final class ProbePeer extends NodeCommand {
            public final NodeProbePeerCommand payload;

            public ProbePeer(NodeProbePeerCommand payload) {
                super(NodeCommandKind.PROBE_PEER);
                this.payload = payload;
            }
        }

        public static final class Shutdown extends NodeCommand {
            public Shutdown() {
                super(NodeCommandKind.SHUTDOWN);
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

        public static final class Probe extends NodeCommandOutcome {
            public final NodePeerProbeSnapshot data;

            public Probe(NodePeerProbeSnapshot data) {
                super("probe");
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
                public final String network_name;
                public final NodeRoutingMode mode;
                public final String exit_node;
                public final Boolean is_exit_node;

                public RoutingChangedData(String network_name, NodeRoutingMode mode, String exit_node, Boolean is_exit_node) {
                    this.network_name = network_name;
                    this.mode = mode;
                    this.exit_node = exit_node;
                    this.is_exit_node = is_exit_node;
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
                public final String network_name;
                public final NodeNetworkRole role;
                public final Boolean enabled;
                public final List<NodeNetworkRole> roles;

                public RoleChangedData(String network_name, NodeNetworkRole role, Boolean enabled, List<NodeNetworkRole> roles) {
                    this.network_name = network_name;
                    this.role = role;
                    this.enabled = enabled;
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
            public final Integer schema_version;
            public final NodeEvent event;

            public Event(Integer schema_version, NodeEvent event) {
                super("event");
                this.schema_version = schema_version;
                this.event = event;
            }
        }

        public static final class Resync extends NodeWsMessage {
            public final Integer schema_version;
            public final NodeWsResyncReason reason;
            public final NodeSnapshot snapshot;

            public Resync(Integer schema_version, NodeWsResyncReason reason, NodeSnapshot snapshot) {
                super("resync");
                this.schema_version = schema_version;
                this.reason = reason;
                this.snapshot = snapshot;
            }
        }

        public static final class Error extends NodeWsMessage {
            public final Integer schema_version;
            public final NodeHttpErrorCode code;
            public final String message;

            public Error(Integer schema_version, NodeHttpErrorCode code, String message) {
                super("error");
                this.schema_version = schema_version;
                this.code = code;
                this.message = message;
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
        return             command == NodeCommandKind.CREATE_NETWORK ||
            command == NodeCommandKind.GET_SNAPSHOT ||
            command == NodeCommandKind.JOIN_NETWORK ||
            command == NodeCommandKind.PROBE_PEER ||
            command == NodeCommandKind.RESTART_NETWORKS ||
            command == NodeCommandKind.SET_NETWORK_ROLE ||
            command == NodeCommandKind.SHUTDOWN ||
            command == NodeCommandKind.START_NETWORKS ||
            command == NodeCommandKind.STOP_NETWORKS ||
            command == NodeCommandKind.UPDATE_ROUTING;
    }
}
