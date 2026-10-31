// Generated from contracts/schema/*.schema.json. Do not edit by hand.
// Regenerate with scripts/generate-contracts.sh.

export const NODE_CONTRACT_SCHEMA_VERSION = 1 as const;
export const NODE_COMMAND_KINDS = ["create_network", "get_snapshot", "get_ui_manifest", "join_network", "probe_peer", "restart_networks", "set_network_role", "shutdown", "start_networks", "stop_networks", "update_routing"] as const;
export type NodeCommandKindWire = (typeof NODE_COMMAND_KINDS)[number];
export const NODE_MANIFEST_ACTION_COMMAND_KINDS = ["create_network", "get_snapshot", "join_network", "probe_peer", "restart_networks", "set_network_role", "shutdown", "start_networks", "stop_networks", "update_routing"] as const;
export type NodeManifestActionCommandKind =
  (typeof NODE_MANIFEST_ACTION_COMMAND_KINDS)[number];

export type AclAction = "allow" | "deny";

export type CapabilityKind = "coordinator" | "relay" | "exit" | "file";

export type CapabilityState = "disabled" | "starting" | "enabled" | "degraded" | "unavailable" | "unsupported" | "unknown";

export type DisplayEnvironment = "linux_x11" | "linux_wayland" | "windows_desktop" | "macos_desktop" | "headless";

export type ExitNodeCapabilities = {
  "schema_version": number;
  "platform": ExitNodePlatform;
  "supported": boolean;
  "ipv4_forwarding": boolean;
  "ipv6_forwarding": boolean;
  "nat": boolean;
  "cleanup": boolean;
  "unsupported_reasons": Array<ExitNodeUnsupportedReason>;
};

export type ExitNodePlatform = "linux" | "windows" | "macos" | "other";

export type ExitNodeUnsupportedReason = "platform_not_linux" | "privileges_required" | "routing_tool_unavailable" | "iptables_unavailable" | "ipv4_forwarding_unavailable";

export type NetworkMode = "ipv4_only" | "ipv6_only" | "dual_stack";

export type NodeAccessModel = "trusted_control_plane";

export type NodeCapabilityRegistrySnapshot = {
  "capabilities": Array<NodeCapabilitySnapshot>;
};

export type NodeCapabilitySnapshot = {
  "kind": CapabilityKind;
  "state": CapabilityState;
};

export type NodeCommand = {
  "command": "get_snapshot";
} | {
  "command": "get_ui_manifest";
} | {
  "command": "start_networks";
  "payload": {
  "selection": NodeNetworkSelection;
};
} | {
  "command": "stop_networks";
  "payload": {
  "networks": Array<string>;
  "all": boolean;
};
} | {
  "command": "restart_networks";
  "payload": {
  "networks": Array<string>;
  "all": boolean;
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
  "command": "probe_peer";
  "payload": NodeProbePeerCommand;
} | {
  "command": "shutdown";
};

export type NodeCommandHttpRequest = {
  "schema_version": number;
  "request": NodeCommandRequest;
};

export type NodeCommandHttpResponse = {
  "schema_version": number;
  "result": NodeCommandResult;
};

export type NodeCommandOutcome = {
  "status": "snapshot";
  "data": NodeSnapshot;
} | {
  "status": "ui_manifest";
  "data": NodeUiManifest;
} | {
  "status": "probe";
  "data": NodePeerProbeSnapshot;
} | {
  "status": "accepted";
  "data": {
  "operation_id": string;
};
} | {
  "status": "completed";
  "data": {
  "message": string;
};
} | {
  "status": "rejected";
  "data": NodeProblem;
};

export type NodeCommandRequest = {
  "request_id": number;
  "command": NodeCommand;
};

export type NodeCommandResult = {
  "request_id": number;
  "outcome": NodeCommandOutcome;
};

export type NodeConfigurationSnapshot = {
  "networks": Array<NodeNetworkConfigSnapshot>;
};

export type NodeCreateNetworkCommand = {
  "name": string;
  "signaling_port": number;
  "relay_port": number;
  "external_host"?: string | null;
  "auto_connect": boolean;
};

export type NodeEmbeddedServiceSnapshot = {
  "network_name": string;
  "status": NodeEmbeddedServiceStatus;
  "signaling_addr"?: string | null;
  "relay_addr"?: string | null;
};

export type NodeEmbeddedServiceStatus = "starting" | "running" | "stopping" | "stopped" | "failed";

export type NodeEvent = {
  "sequence": number;
  "kind": NodeEventKind;
};

export type NodeEventKind = {
  "type": "state_changed";
  "data": {
  "revision": number;
};
} | {
  "type": "network_changed";
  "data": {
  "network_name": string;
  "status": NodeNetworkStatus;
};
} | {
  "type": "embedded_service_changed";
  "data": {
  "network_name": string;
  "status": NodeEmbeddedServiceStatus;
};
} | {
  "type": "log_appended";
  "data": {
  "level": NodeLogLevel;
  "message": string;
};
} | {
  "type": "routing_changed";
  "data": {
  "network_name": string;
  "mode": NodeRoutingMode;
  "exit_node"?: string | null;
  "is_exit_node": boolean;
};
} | {
  "type": "role_changed";
  "data": {
  "network_name": string;
  "role": NodeNetworkRole;
  "enabled": boolean;
  "roles": Array<NodeNetworkRole>;
};
} | {
  "type": "command_completed";
  "data": {
  "request_id": number;
  "success": boolean;
};
};

export type NodeHttpContract = {
  "command_request": NodeCommandHttpRequest;
  "command_response": NodeCommandHttpResponse;
  "manifest_response": NodeManifestResponse;
  "snapshot_response": NodeSnapshotResponse;
  "error_response": NodeHttpErrorResponse;
  "websocket_message": NodeWsMessage;
};

export type NodeHttpErrorCode = "schema_version_mismatch" | "invalid_request" | "backend_rejected" | "backend_unavailable" | "contract_violation";

export type NodeHttpErrorResponse = {
  "schema_version": number;
  "code": NodeHttpErrorCode;
  "message": string;
  "requested_schema_version"?: number | null;
  "supported_schema_version"?: number | null;
  "problem"?: NodeProblem | null;
};

export type NodeIdentitySnapshot = {
  "node_id": string;
  "display_name"?: string | null;
  "public_key_fingerprint": string;
};

export type NodeJoinNetworkCommand = {
  "target": string;
  "network_name"?: string | null;
  "start": boolean;
};

export type NodeLifecycleSnapshot = {
  "phase": NodePhase;
  "configured_networks": number;
  "active_networks": number;
  "failed_networks": Array<string>;
};

export type NodeLogEntry = {
  "unix_ms": number;
  "level": NodeLogLevel;
  "target": string;
  "message": string;
};

export type NodeLogLevel = "trace" | "debug" | "info" | "warn" | "error";

export type NodeManifestResponse = {
  "schema_version": number;
  "manifest": NodeUiManifest;
};

export type NodeNetworkConfigSnapshot = {
  "network_name": string;
  "coordinator_url": string;
  "virtual_mode"?: NetworkMode | null;
  "preferred_ipv4"?: string | null;
  "preferred_ipv6"?: string | null;
  "virtual_network"?: string | null;
  "tun_name": string;
  "mtu": number;
  "p2p_listen_port": number;
  "p2p_connect_timeout_secs": number;
  "p2p_heartbeat_interval_secs": number;
  "routing_mode": NodeRoutingMode;
  "exit_node"?: string | null;
  "direct_cidrs": Array<string>;
  "proxy_cidrs": Array<string>;
  "exit_node_enabled": boolean;
  "exit_node_allowed_clients": Array<string>;
  "exit_node_allow_lan_access": boolean;
  "exit_node_region"?: string | null;
  "acl_enabled": boolean;
  "acl_default_action": AclAction;
  "acl_rule_count": number;
  "log_level": string;
  "log_file_configured": boolean;
  "identity_configured": boolean;
};

export type NodeNetworkRole = "member" | "discovery" | "relay" | "exit_node";

export type NodeNetworkSelection = "default" | "all" | "none" | {
  "named": Array<string>;
};

export type NodeNetworkSnapshot = {
  "name": string;
  "tun_name": string;
  "status": NodeNetworkStatus;
  "assigned_ipv6"?: string | null;
  "assigned_ipv4"?: string | null;
  "p2p_listen_addr"?: string | null;
  "uptime_secs": number;
  "roles": Array<NodeNetworkRole>;
  "peer_count": number;
  "traffic": NodeTrafficSnapshot;
};

export type NodeNetworkStatus = "offline" | "connecting" | "online" | "degraded" | "stopping" | "failed";

export type NodePeerProbeSnapshot = {
  "peer_id": string;
  "virtual_ipv6": string;
  "endpoint"?: string | null;
  "rtt_us"?: number | null;
  "p2p_active": boolean;
  "timed_out": boolean;
};

export type NodePeerSnapshot = {
  "peer_id": string;
  "network_name": string;
  "virtual_ipv6": string;
  "virtual_ipv4"?: string | null;
  "endpoints": Array<string>;
  "online": boolean;
  "p2p_active": boolean;
  "p2p_healthy": boolean;
  "rtt_us"?: number | null;
  "loss_rate": number;
  "traffic": NodeTrafficSnapshot;
};

export type NodePermission = "view_state" | "manage_networks" | "change_routing" | "manage_roles" | "probe_peers" | "stop_node" | "view_logs";

export type NodePermissionSnapshot = {
  "access_model": NodeAccessModel;
  "permissions": Array<NodePermission>;
};

export type NodePhase = "stopped" | "starting" | "running" | "stopping" | "failed";

export type NodeProbePeerCommand = {
  "network_name": string;
  "target": string;
  "timeout_ms": number;
};

export type NodeProblem = {
  "code": NodeProblemCode;
  "message": string;
  "network_name"?: string | null;
  "retryable": boolean;
};

export type NodeProblemCode = "invalid_request" | "not_found" | "conflict" | "unavailable" | "permission_denied" | "cancelled" | "internal" | "unsupported";

export type NodeRoutingMode = "mesh_only" | "bypass_mainland" | "global_proxy";

export type NodeSetNetworkRoleCommand = {
  "network_name": string;
  "role": NodeNetworkRole;
  "enabled": boolean;
};

export type NodeSnapshot = {
  "revision": number;
  "lifecycle": NodeLifecycleSnapshot;
  "identity"?: NodeIdentitySnapshot | null;
  "configuration"?: NodeConfigurationSnapshot;
  "exit_node_capabilities"?: ExitNodeCapabilities;
  "capabilities"?: NodeCapabilityRegistrySnapshot;
  "permissions"?: NodePermissionSnapshot;
  "embedded_services"?: Array<NodeEmbeddedServiceSnapshot>;
  "ui_mode"?: UiModeResolution | null;
  "networks": Array<NodeNetworkSnapshot>;
  "peers": Array<NodePeerSnapshot>;
  "logs": Array<NodeLogEntry>;
  "last_error"?: NodeProblem | null;
};

export type NodeSnapshotResponse = {
  "schema_version": number;
  "snapshot": NodeSnapshot;
};

export type NodeTrafficSnapshot = {
  "tun_rx_packets": number;
  "tun_rx_bytes": number;
  "tun_tx_packets": number;
  "tun_tx_bytes": number;
  "p2p_rx_packets": number;
  "p2p_rx_bytes": number;
  "p2p_tx_packets": number;
  "p2p_tx_bytes": number;
  "error_count": number;
};

export type NodeUiAction = {
  "id": string;
  "label_key": string;
  "command": "get_snapshot" | "start_networks" | "stop_networks" | "restart_networks" | "create_network" | "join_network" | "update_routing" | "set_network_role" | "probe_peer" | "shutdown";
  "backend_status": NodeUiBackendStatus;
  "permission": NodePermission;
  "audit": NodeUiAudit;
  "availability": NodeUiAvailability;
  "fields": Array<NodeUiField>;
  "confirmation"?: NodeUiConfirmation | null;
};

export type NodeUiAudit = {
  "policy": NodeUiAuditPolicy;
  "action"?: string | null;
};

export type NodeUiAuditPolicy = "none" | "record_outcome";

export type NodeUiAvailability = "always" | "node_running" | "network_online" | "network_selected";

export type NodeUiBackendStatus = "implemented" | "unavailable";

export type NodeUiConfirmation = {
  "title_key": string;
  "consequence_key": string;
  "destructive": boolean;
};

export type NodeUiField = {
  "id": string;
  "label_key": string;
  "input": NodeUiInputKind;
  "required": boolean;
  "validation": NodeUiValidation;
};

export type NodeUiInputKind = "boolean" | "external_host" | "network_name" | "network_selection" | "peer_target" | "port" | "role" | "routing_mode" | "url_or_ticket" | "timeout_ms";

export type NodeUiManifest = {
  "schema_version": number;
  "pages": Array<NodeUiPage>;
};

export type NodeUiPage = {
  "id": string;
  "nav_label_key": string;
  "title_key": string;
  "actions": Array<NodeUiAction>;
};

export type NodeUiValidation = "none" | "non_empty" | "network_name" | "port" | "distinct_ports" | "network_selection" | "role" | "routing_mode" | "positive_timeout";

export type NodeUpdateRoutingCommand = {
  "network_name": string;
  "mode": NodeRoutingMode;
  "exit_node"?: string | null;
  "is_exit_node": boolean;
};

export type NodeWsMessage = {
  "type": "snapshot";
  "schema_version": number;
  "snapshot": NodeSnapshot;
} | {
  "type": "event";
  "schema_version": number;
  "event": NodeEvent;
} | {
  "type": "resync";
  "schema_version": number;
  "reason": NodeWsResyncReason;
  "snapshot": NodeSnapshot;
} | {
  "type": "error";
  "schema_version": number;
  "code": NodeHttpErrorCode;
  "message": string;
};

export type NodeWsResyncReason = "lagged" | "revision_gap";

export type UiMode = "gpui" | "web_ui" | "headless";

export type UiModeRequest = "auto" | "gpui" | "web_ui" | "headless";

export type UiModeResolution = {
  "requested": UiModeRequest;
  "effective": UiMode;
  "display": DisplayEnvironment;
  "gpui_available": boolean;
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
