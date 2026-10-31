(function (root, factory) {
  const api = factory();
  if (typeof module !== "undefined" && module.exports) {
    module.exports = api;
  }
  root.FuckIPV6NodeUi = api;
})(typeof globalThis !== "undefined" ? globalThis : this, function () {
  "use strict";

  const HTTP_SCHEMA_VERSION = 1;
  const MANIFEST_SCHEMA_VERSION = 1;
  const EXIT_NODE_CAPABILITY_SCHEMA_VERSION = 1;
  const REQUIRED_PAGES = Object.freeze([
      "overview",
      "networks",
      "peers",
      "topology",
      "traffic",
      "routing",
      "exit_node",
      "relay",
      "logs",
      "identity",
      "settings",
  ]);
  const PERMISSIONS = Object.freeze([
      "view_state",
      "manage_networks",
      "change_routing",
      "manage_roles",
      "probe_peers",
      "stop_node",
      "view_logs",
  ]);
  const AVAILABILITIES = Object.freeze([
      "always",
      "node_running",
      "network_online",
      "network_selected",
  ]);
  const INPUT_KINDS = Object.freeze([
      "boolean",
      "external_host",
      "network_name",
      "network_selection",
      "peer_target",
      "port",
      "role",
      "routing_mode",
      "url_or_ticket",
      "timeout_ms",
  ]);
  const VALIDATIONS = Object.freeze([
      "none",
      "non_empty",
      "network_name",
      "port",
      "distinct_ports",
      "network_selection",
      "role",
      "routing_mode",
      "positive_timeout",
  ]);
  const LIFECYCLE_PHASES = Object.freeze(["stopped", "starting", "running", "stopping", "failed"]);
  const NETWORK_STATUSES = Object.freeze(["offline", "connecting", "online", "degraded", "stopping", "failed"]);
  const NETWORK_ROLES = Object.freeze(["member", "discovery", "relay", "exit_node"]);
  const ROUTING_MODES = Object.freeze(["mesh_only", "bypass_mainland", "global_proxy"]);
  const NETWORK_MODES = Object.freeze(["ipv4_only", "ipv6_only", "dual_stack"]);
  const ACL_ACTIONS = Object.freeze(["allow", "deny"]);
  const LOG_LEVELS = Object.freeze(["trace", "debug", "info", "warn", "error"]);
  const SERVICE_STATUSES = Object.freeze(["starting", "running", "stopping", "stopped", "failed"]);
  const ACCESS_MODELS = Object.freeze(["trusted_control_plane"]);
  const EXIT_NODE_PLATFORMS = Object.freeze(["linux", "windows", "macos", "other"]);
  const EXIT_NODE_UNSUPPORTED_REASONS = Object.freeze([
    "platform_not_linux",
    "privileges_required",
    "routing_tool_unavailable",
    "iptables_unavailable",
    "ipv4_forwarding_unavailable",
  ]);
  const UI_MODES = Object.freeze(["gpui", "web_ui", "headless"]);
  const UI_MODE_REQUESTS = Object.freeze(["auto", "gpui", "web_ui", "headless"]);
  const DISPLAY_ENVIRONMENTS = Object.freeze([
    "linux_x11",
    "linux_wayland",
    "windows_desktop",
    "macos_desktop",
    "headless",
  ]);
  const COMMAND_OUTCOMES = Object.freeze([
    "snapshot",
    "ui_manifest",
    "probe",
    "accepted",
    "completed",
    "rejected",
  ]);
  const PROBLEM_CODES = Object.freeze([
    "invalid_request",
    "not_found",
    "conflict",
    "unavailable",
    "permission_denied",
    "cancelled",
    "unsupported",
    "internal",
  ]);
  // This object is the WebUI-side echo of the canonical manifest. It is kept as
  // strict JSON between the markers so Rust tests can parse and compare it with
  // `node_ui_manifest()` without a browser, fixture file, or Node.js runtime.
  const MANIFEST_CONTRACT = Object.freeze(
    /* NODE_UI_MANIFEST_CONTRACT_BEGIN */
    {
      "schema_version": 1,
      "pages": [
        {
          "id": "overview",
          "nav_label_key": "nav.overview",
          "title_key": "page.overview",
          "actions": [
            {
              "id": "refresh_snapshot",
              "label_key": "action.refresh_snapshot",
              "command": "get_snapshot",
              "backend_status": "implemented",
              "permission": "view_state",
              "availability": "always",
              "fields": [],
              "confirmation": null
            }
          ]
        },
        {
          "id": "networks",
          "nav_label_key": "nav.networks",
          "title_key": "page.networks",
          "actions": [
            {
              "id": "create_network",
              "label_key": "action.create_network",
              "command": "create_network",
              "backend_status": "implemented",
              "permission": "manage_networks",
              "availability": "node_running",
              "fields": [
                {
                  "id": "name",
                  "label_key": "field.network_name",
                  "input": "network_name",
                  "required": true,
                  "validation": "network_name"
                },
                {
                  "id": "signaling_port",
                  "label_key": "field.signaling_port",
                  "input": "port",
                  "required": true,
                  "validation": "port"
                },
                {
                  "id": "relay_port",
                  "label_key": "field.relay_port",
                  "input": "port",
                  "required": true,
                  "validation": "distinct_ports"
                },
                {
                  "id": "external_host",
                  "label_key": "field.external_host",
                  "input": "external_host",
                  "required": false,
                  "validation": "none"
                },
                {
                  "id": "auto_connect",
                  "label_key": "field.auto_connect",
                  "input": "boolean",
                  "required": true,
                  "validation": "none"
                }
              ],
              "confirmation": null
            },
            {
              "id": "join_network",
              "label_key": "action.join_network",
              "command": "join_network",
              "backend_status": "implemented",
              "permission": "manage_networks",
              "availability": "node_running",
              "fields": [
                {
                  "id": "target",
                  "label_key": "field.ticket_or_url",
                  "input": "url_or_ticket",
                  "required": true,
                  "validation": "non_empty"
                },
                {
                  "id": "network_name",
                  "label_key": "field.network_name",
                  "input": "network_name",
                  "required": false,
                  "validation": "network_name"
                },
                {
                  "id": "start",
                  "label_key": "field.start_after_join",
                  "input": "boolean",
                  "required": true,
                  "validation": "none"
                }
              ],
              "confirmation": null
            },
            {
              "id": "start_networks",
              "label_key": "action.start_networks",
              "command": "start_networks",
              "backend_status": "implemented",
              "permission": "manage_networks",
              "availability": "node_running",
              "fields": [
                {
                  "id": "selection",
                  "label_key": "field.network_selection",
                  "input": "network_selection",
                  "required": true,
                  "validation": "network_selection"
                }
              ],
              "confirmation": null
            },
            {
              "id": "stop_networks",
              "label_key": "action.stop_networks",
              "command": "stop_networks",
              "backend_status": "implemented",
              "permission": "manage_networks",
              "availability": "network_online",
              "fields": [
                {
                  "id": "networks",
                  "label_key": "field.network_selection",
                  "input": "network_selection",
                  "required": false,
                  "validation": "network_selection"
                },
                {
                  "id": "all",
                  "label_key": "field.all_networks",
                  "input": "boolean",
                  "required": false,
                  "validation": "none"
                }
              ],
              "confirmation": {
                "title_key": "confirm.stop_networks.title",
                "consequence_key": "confirm.stop_networks.consequence",
                "destructive": false
              }
            }
          ]
        },
        {
          "id": "peers",
          "nav_label_key": "nav.peers",
          "title_key": "page.peers",
          "actions": [
            {
              "id": "probe_peer",
              "label_key": "action.probe_peer",
              "command": "probe_peer",
              "backend_status": "implemented",
              "permission": "probe_peers",
              "availability": "network_online",
              "fields": [
                {
                  "id": "network_name",
                  "label_key": "field.network_name",
                  "input": "network_name",
                  "required": true,
                  "validation": "network_name"
                },
                {
                  "id": "target",
                  "label_key": "field.peer_target",
                  "input": "peer_target",
                  "required": true,
                  "validation": "non_empty"
                },
                {
                  "id": "timeout_ms",
                  "label_key": "field.timeout_ms",
                  "input": "timeout_ms",
                  "required": true,
                  "validation": "positive_timeout"
                }
              ],
              "confirmation": null
            }
          ]
        },
        {
          "id": "topology",
          "nav_label_key": "nav.topology",
          "title_key": "page.topology",
          "actions": []
        },
        {
          "id": "traffic",
          "nav_label_key": "nav.traffic",
          "title_key": "page.traffic",
          "actions": []
        },
        {
          "id": "routing",
          "nav_label_key": "nav.routing",
          "title_key": "page.routing",
          "actions": [
            {
              "id": "update_routing",
              "label_key": "action.update_routing",
              "command": "update_routing",
              "backend_status": "implemented",
              "permission": "change_routing",
              "availability": "network_selected",
              "fields": [
                {
                  "id": "network_name",
                  "label_key": "field.network_name",
                  "input": "network_name",
                  "required": true,
                  "validation": "network_name"
                },
                {
                  "id": "mode",
                  "label_key": "field.routing_mode",
                  "input": "routing_mode",
                  "required": true,
                  "validation": "routing_mode"
                },
                {
                  "id": "exit_node",
                  "label_key": "field.exit_node",
                  "input": "peer_target",
                  "required": false,
                  "validation": "none"
                },
                {
                  "id": "is_exit_node",
                  "label_key": "field.is_exit_node",
                  "input": "boolean",
                  "required": true,
                  "validation": "none"
                }
              ],
              "confirmation": {
                "title_key": "confirm.update_routing.title",
                "consequence_key": "confirm.update_routing.consequence",
                "destructive": false
              }
            }
          ]
        },
        {
          "id": "exit_node",
          "nav_label_key": "nav.exit_node",
          "title_key": "page.exit_node",
          "actions": [
            {
              "id": "set_exit_node_role",
              "label_key": "action.set_exit_node_role",
              "command": "set_network_role",
              "backend_status": "implemented",
              "permission": "manage_roles",
              "availability": "network_selected",
              "fields": [
                {
                  "id": "network_name",
                  "label_key": "field.network_name",
                  "input": "network_name",
                  "required": true,
                  "validation": "network_name"
                },
                {
                  "id": "role",
                  "label_key": "field.role",
                  "input": "role",
                  "required": true,
                  "validation": "role"
                },
                {
                  "id": "enabled",
                  "label_key": "field.enabled",
                  "input": "boolean",
                  "required": true,
                  "validation": "none"
                }
              ],
              "confirmation": {
                "title_key": "confirm.set_exit_node_role.title",
                "consequence_key": "confirm.set_exit_node_role.consequence",
                "destructive": false
              }
            }
          ]
        },
        {
          "id": "relay",
          "nav_label_key": "nav.relay",
          "title_key": "page.relay",
          "actions": [
            {
              "id": "set_relay_role",
              "label_key": "action.set_relay_role",
              "command": "set_network_role",
              "backend_status": "unavailable",
              "permission": "manage_roles",
              "availability": "network_selected",
              "fields": [
                {
                  "id": "network_name",
                  "label_key": "field.network_name",
                  "input": "network_name",
                  "required": true,
                  "validation": "network_name"
                },
                {
                  "id": "role",
                  "label_key": "field.role",
                  "input": "role",
                  "required": true,
                  "validation": "role"
                },
                {
                  "id": "enabled",
                  "label_key": "field.enabled",
                  "input": "boolean",
                  "required": true,
                  "validation": "none"
                }
              ],
              "confirmation": {
                "title_key": "confirm.set_relay_role.title",
                "consequence_key": "confirm.set_relay_role.consequence",
                "destructive": false
              }
            }
          ]
        },
        {
          "id": "logs",
          "nav_label_key": "nav.logs",
          "title_key": "page.logs",
          "actions": []
        },
        {
          "id": "identity",
          "nav_label_key": "nav.identity",
          "title_key": "page.identity",
          "actions": []
        },
        {
          "id": "settings",
          "nav_label_key": "nav.settings",
          "title_key": "page.settings",
          "actions": [
            {
              "id": "shutdown_node",
              "label_key": "action.shutdown_node",
              "command": "shutdown",
              "backend_status": "implemented",
              "permission": "stop_node",
              "availability": "node_running",
              "fields": [],
              "confirmation": {
                "title_key": "confirm.shutdown_node.title",
                "consequence_key": "confirm.shutdown_node.consequence",
                "destructive": true
              }
            }
          ]
        }
      ]
    }
    /* NODE_UI_MANIFEST_CONTRACT_END */
  );
  const ACTION_CONTRACT = Object.freeze(Object.fromEntries(
    MANIFEST_CONTRACT.pages
      .flatMap((page) => page.actions)
      .map((action) => [action.id, action]),
  ));

  const LABELS = Object.freeze(
    /* LABELS_BEGIN */
    {
      "nav.overview": "概览",
      "nav.networks": "网络",
      "nav.peers": "节点",
      "nav.topology": "拓扑",
      "nav.traffic": "流量",
      "nav.routing": "路由",
      "nav.exit_node": "Exit Node",
      "nav.relay": "中继",
      "nav.logs": "日志",
      "nav.identity": "身份",
      "nav.settings": "设置",
      "page.overview": "节点概览",
      "page.networks": "网络管理",
      "page.peers": "对等节点",
      "page.topology": "拓扑视图",
      "page.traffic": "流量统计",
      "page.routing": "路由策略",
      "page.exit_node": "Exit Node",
      "page.relay": "中继能力",
      "page.logs": "运行日志",
      "page.identity": "节点身份",
      "page.settings": "权限与设置",
      "action.refresh_snapshot": "刷新快照",
      "action.create_network": "创建网络",
      "action.join_network": "加入网络",
      "action.start_networks": "启动网络",
      "action.stop_networks": "停止网络",
      "action.probe_peer": "探测节点",
      "action.update_routing": "更新路由",
      "action.set_exit_node_role": "设置 Exit Node 角色",
      "action.set_relay_role": "设置中继角色",
      "action.shutdown_node": "关闭节点",
      "field.network_name": "网络名称",
      "field.signaling_port": "信令端口",
      "field.relay_port": "中继端口",
      "field.external_host": "外部主机",
      "field.auto_connect": "创建后连接",
      "field.ticket_or_url": "票据或 URL",
      "field.start_after_join": "加入后启动",
      "field.network_selection": "网络选择",
      "field.all_networks": "全部网络",
      "field.peer_target": "对等节点",
      "field.timeout_ms": "超时（毫秒）",
      "field.routing_mode": "路由模式",
      "field.exit_node": "出口节点",
      "field.is_exit_node": "启用本节点 Exit Node",
      "field.role": "角色",
      "field.enabled": "启用",
      "confirm.stop_networks.title": "确认停止网络",
      "confirm.stop_networks.consequence": "所选网络及其 Peer 会话会停止。",
      "confirm.update_routing.title": "确认更新路由",
      "confirm.update_routing.consequence": "新路由会热应用到当前网络，不重启网络。",
      "confirm.set_exit_node_role.title": "确认角色变更",
      "confirm.set_exit_node_role.consequence": "角色变更会立即影响此节点在网络中的能力。",
      "confirm.set_relay_role.title": "确认角色变更",
      "confirm.set_relay_role.consequence": "角色变更会立即影响此节点在网络中的能力。",
      "confirm.shutdown_node.title": "确认关闭节点",
      "confirm.shutdown_node.consequence": "所有网络和数据面会停止，且当前操作不可撤销。"
    }
    /* LABELS_END */
  );

  const ERROR_TEXT = Object.freeze(
    /* ERROR_TEXT_BEGIN */
    {
      "schema_version_mismatch": "协议版本不兼容，已拒绝静默降级。",
      "invalid_request": "请求字段无效。",
      "backend_rejected": "节点后端拒绝了命令。",
      "backend_unavailable": "节点命令总线当前不可用。",
      "contract_violation": "共享 UI 契约违规，界面已失败关闭。",
      "not_found": "目标不存在。",
      "conflict": "操作与当前节点状态冲突。",
      "unavailable": "所需能力当前不可用。",
      "permission_denied": "共享权限快照未授予此操作。",
      "cancelled": "操作已取消。",
      "unsupported": "当前平台或主机不支持此操作。",
      "internal": "节点内部错误。"
    }
    /* ERROR_TEXT_END */
  );

  function assertContract(condition, message) {
    if (!condition) {
      throw new Error("node-ui-contract: " + message);
    }
  }

  function label(key) {
    return LABELS[key] || key;
  }

  function unique(values, kind) {
    const seen = new Set();
    for (const value of values) {
      assertContract(typeof value === "string" && value.length > 0, kind + " contains an empty value");
      assertContract(!seen.has(value), "duplicate " + kind + ": " + value);
      seen.add(value);
    }
  }

  function contractValue(condition, message) {
    assertContract(condition, message);
    return true;
  }

  function isRecord(value) {
    return value !== null && typeof value === "object" && !Array.isArray(value);
  }

  function requireString(value, message, nonEmpty) {
    contractValue(typeof value === "string" && (!nonEmpty || value.length > 0), message);
  }

  function requireBoolean(value, message) {
    contractValue(typeof value === "boolean", message);
  }

  function requireNumber(value, message) {
    contractValue(typeof value === "number" && Number.isFinite(value) && value >= 0, message);
  }

  function requireInteger(value, message) {
    contractValue(Number.isSafeInteger(value) && value >= 0, message);
  }

  function requireEnum(value, allowed, message) {
    contractValue(allowed.includes(value), message + ": " + value);
  }

  function requireStringArray(value, message) {
    contractValue(Array.isArray(value), message);
    for (const item of value) requireString(item, message + " entry");
  }

  function canonicalValue(value) {
    if (Array.isArray(value)) return value.map(canonicalValue);
    if (isRecord(value)) {
      return Object.fromEntries(Object.keys(value).sort().map((key) => [key, canonicalValue(value[key])]));
    }
    return value;
  }

  function canonicalJson(value) {
    return JSON.stringify(canonicalValue(value));
  }

  function validateManifestResponse(response) {
    assertContract(response && typeof response === "object", "manifest response is missing");
    assertContract(
      response.schema_version === HTTP_SCHEMA_VERSION,
      "HTTP schema mismatch: expected=" + HTTP_SCHEMA_VERSION + " actual=" + response.schema_version,
    );
    const manifest = response.manifest;
    assertContract(manifest && typeof manifest === "object", "manifest payload is missing");
    assertContract(
      manifest.schema_version === MANIFEST_SCHEMA_VERSION,
      "manifest schema mismatch: expected=" + MANIFEST_SCHEMA_VERSION + " actual=" + manifest.schema_version,
    );
    assertContract(Array.isArray(manifest.pages), "manifest pages must be an array");
    const pageIds = manifest.pages.map((page) => page.id);
    unique(pageIds, "page id");
    assertContract(
      JSON.stringify(pageIds) === JSON.stringify(REQUIRED_PAGES),
      "page set/order mismatch: expected=" + REQUIRED_PAGES.join(",") + " actual=" + pageIds.join(","),
    );

    const actionIds = [];
    for (const page of manifest.pages) {
      const expectedPage = MANIFEST_CONTRACT.pages.find((candidate) => candidate.id === page.id);
      assertContract(expectedPage, "unknown WebUI page: " + page.id);
      assertContract(
        canonicalJson(page) === canonicalJson(expectedPage),
        "page contract mismatch for " + page.id,
      );
      requireString(page.nav_label_key, "page missing nav label key", true);
      requireString(page.title_key, "page missing title key", true);
      assertContract(Array.isArray(page.actions), "page actions must be an array: " + page.id);
      const pageActionIds = page.actions.map((action) => action.id);
      unique(pageActionIds, "action id for " + page.id);
      for (const action of page.actions) {
        actionIds.push(action.id);
        assertContract(
          PERMISSIONS.includes(action.permission),
          "unknown permission for " + action.id,
        );
        assertContract(
          AVAILABILITIES.includes(action.availability),
          "unknown availability for " + action.id,
        );
        assertContract(
          action.backend_status === "implemented" || action.backend_status === "unavailable",
          "unknown backend status for " + action.id,
        );
        assertContract(Array.isArray(action.fields), "action fields must be an array: " + action.id);
        for (const field of action.fields) {
          requireString(field.label_key, "field missing label key for " + action.id + "." + field.id, true);
          assertContract(INPUT_KINDS.includes(field.input), "unknown input kind for " + action.id + "." + field.id);
          assertContract(VALIDATIONS.includes(field.validation), "unknown validation for " + action.id + "." + field.id);
          requireBoolean(field.required, "required must be boolean for " + action.id + "." + field.id);
        }
        if (action.command === "shutdown") {
          assertContract(action.confirmation && action.confirmation.destructive === true, "shutdown requires destructive confirmation");
        }
      }
    }
    unique(actionIds, "action id");
    const expectedActions = Object.keys(ACTION_CONTRACT).sort();
    assertContract(
      JSON.stringify(actionIds.slice().sort()) === JSON.stringify(expectedActions),
      "action set mismatch",
    );
    const labelKeys = manifest.pages.flatMap((page) => [
      page.nav_label_key,
      page.title_key,
      ...page.actions.flatMap((action) => [
        action.label_key,
        ...action.fields.map((field) => field.label_key),
        ...(action.confirmation
          ? [action.confirmation.title_key, action.confirmation.consequence_key]
          : []),
      ]),
    ]);
    for (const key of new Set(labelKeys)) {
      assertContract(typeof LABELS[key] === "string" && LABELS[key].length > 0, "missing label: " + key);
    }
    return manifest;
  }

  function validateTraffic(traffic, context) {
    assertContract(isRecord(traffic), "traffic snapshot is missing: " + context);
    for (const key of [
      "tun_rx_packets", "tun_rx_bytes", "tun_tx_packets", "tun_tx_bytes",
      "p2p_rx_packets", "p2p_rx_bytes", "p2p_tx_packets", "p2p_tx_bytes", "error_count",
    ]) {
      requireNumber(traffic[key], "invalid traffic field " + context + "." + key);
    }
  }

  function validateConfiguration(configuration) {
    assertContract(isRecord(configuration), "snapshot configuration is missing");
    assertContract(Array.isArray(configuration.networks), "configuration networks must be an array");
    for (const network of configuration.networks) {
      assertContract(isRecord(network), "configuration network must be an object");
      requireString(network.network_name, "configuration network_name is missing", true);
      requireString(network.coordinator_url, "configuration coordinator_url is missing", true);
      assertContract(network.virtual_mode === null || NETWORK_MODES.includes(network.virtual_mode), "invalid configuration virtual_mode");
      assertContract(network.preferred_ipv4 === null || typeof network.preferred_ipv4 === "string", "invalid configuration preferred_ipv4");
      assertContract(network.preferred_ipv6 === null || typeof network.preferred_ipv6 === "string", "invalid configuration preferred_ipv6");
      assertContract(network.virtual_network === null || typeof network.virtual_network === "string", "invalid configuration virtual_network");
      requireString(network.tun_name, "configuration tun_name is missing");
      requireInteger(network.mtu, "invalid configuration mtu");
      requireInteger(network.p2p_listen_port, "invalid configuration p2p_listen_port");
      requireInteger(network.p2p_connect_timeout_secs, "invalid configuration p2p_connect_timeout_secs");
      requireInteger(network.p2p_heartbeat_interval_secs, "invalid configuration p2p_heartbeat_interval_secs");
      requireEnum(network.routing_mode, ROUTING_MODES, "unknown configuration routing mode");
      assertContract(network.exit_node === null || typeof network.exit_node === "string", "invalid configuration exit_node");
      requireStringArray(network.direct_cidrs, "configuration direct_cidrs must be an array");
      requireStringArray(network.proxy_cidrs, "configuration proxy_cidrs must be an array");
      requireBoolean(network.exit_node_enabled, "configuration exit_node_enabled is missing");
      requireStringArray(network.exit_node_allowed_clients, "configuration exit_node_allowed_clients must be an array");
      requireBoolean(network.exit_node_allow_lan_access, "configuration exit_node_allow_lan_access is missing");
      assertContract(network.exit_node_region === null || typeof network.exit_node_region === "string", "invalid configuration exit_node_region");
      requireBoolean(network.acl_enabled, "configuration acl_enabled is missing");
      requireEnum(network.acl_default_action, ACL_ACTIONS, "unknown configuration acl_default_action");
      requireInteger(network.acl_rule_count, "invalid configuration acl_rule_count");
      requireString(network.log_level, "configuration log_level is missing");
      requireBoolean(network.log_file_configured, "configuration log_file_configured is missing");
      requireBoolean(network.identity_configured, "configuration identity_configured is missing");
    }
  }

  function validateSnapshotPayload(snapshot) {
    assertContract(isRecord(snapshot), "snapshot payload is missing");
    requireInteger(snapshot.revision, "snapshot revision must be a non-negative integer");
    assertContract(isRecord(snapshot.lifecycle), "snapshot lifecycle is missing");
    requireEnum(snapshot.lifecycle.phase, LIFECYCLE_PHASES, "unknown lifecycle phase");
    requireInteger(snapshot.lifecycle.configured_networks, "invalid configured_networks");
    requireInteger(snapshot.lifecycle.active_networks, "invalid active_networks");
    requireStringArray(snapshot.lifecycle.failed_networks, "failed_networks must be an array");

    validateConfiguration(snapshot.configuration);
    assertContract(isRecord(snapshot.exit_node_capabilities), "exit node capabilities are missing");
    assertContract(
      snapshot.exit_node_capabilities.schema_version === EXIT_NODE_CAPABILITY_SCHEMA_VERSION,
      "exit node capability schema mismatch: expected=" + EXIT_NODE_CAPABILITY_SCHEMA_VERSION +
        " actual=" + snapshot.exit_node_capabilities.schema_version,
    );
    requireEnum(snapshot.exit_node_capabilities.platform, EXIT_NODE_PLATFORMS, "unknown exit capability platform");
    for (const key of ["supported", "ipv4_forwarding", "ipv6_forwarding", "nat", "cleanup"]) {
      requireBoolean(snapshot.exit_node_capabilities[key], "invalid exit capability " + key);
    }
    assertContract(Array.isArray(snapshot.exit_node_capabilities.unsupported_reasons), "unsupported_reasons must be an array");
    for (const reason of snapshot.exit_node_capabilities.unsupported_reasons) {
      requireEnum(reason, EXIT_NODE_UNSUPPORTED_REASONS, "unknown exit capability reason");
    }

    assertContract(isRecord(snapshot.permissions), "snapshot permissions are missing");
    requireEnum(snapshot.permissions.access_model, ACCESS_MODELS, "unknown access model");
    assertContract(Array.isArray(snapshot.permissions.permissions), "snapshot permissions must be an array");
    unique(snapshot.permissions.permissions, "permission");
    for (const permission of snapshot.permissions.permissions) {
      requireEnum(permission, PERMISSIONS, "unknown snapshot permission");
    }

    assertContract(Array.isArray(snapshot.networks), "snapshot networks must be an array");
    for (const network of snapshot.networks) {
      assertContract(isRecord(network), "network snapshot must be an object");
      requireString(network.name, "network name is missing", true);
      requireString(network.tun_name, "network tun_name is missing");
      requireEnum(network.status, NETWORK_STATUSES, "unknown network status");
      assertContract(network.assigned_ipv6 === null || typeof network.assigned_ipv6 === "string", "invalid assigned_ipv6");
      assertContract(network.assigned_ipv4 === null || typeof network.assigned_ipv4 === "string", "invalid assigned_ipv4");
      assertContract(network.p2p_listen_addr === null || typeof network.p2p_listen_addr === "string", "invalid p2p_listen_addr");
      requireNumber(network.uptime_secs, "invalid network uptime");
      assertContract(Array.isArray(network.roles), "network roles must be an array");
      for (const role of network.roles) requireEnum(role, NETWORK_ROLES, "unknown network role");
      requireInteger(network.peer_count, "invalid network peer_count");
      validateTraffic(network.traffic, network.name);
    }

    assertContract(Array.isArray(snapshot.peers), "snapshot peers must be an array");
    for (const peer of snapshot.peers) {
      assertContract(isRecord(peer), "peer snapshot must be an object");
      requireString(peer.peer_id, "peer id is missing", true);
      requireString(peer.network_name, "peer network_name is missing", true);
      requireString(peer.virtual_ipv6, "peer virtual_ipv6 is missing", true);
      assertContract(peer.virtual_ipv4 === null || typeof peer.virtual_ipv4 === "string", "invalid peer virtual_ipv4");
      requireStringArray(peer.endpoints, "peer endpoints must be an array");
      for (const key of ["online", "p2p_active", "p2p_healthy"]) {
        requireBoolean(peer[key], "invalid peer field " + key);
      }
      assertContract(peer.rtt_us === null || (Number.isSafeInteger(peer.rtt_us) && peer.rtt_us >= 0), "invalid peer rtt_us");
      requireNumber(peer.loss_rate, "invalid peer loss_rate");
      assertContract(peer.loss_rate <= 1, "peer loss_rate must be between 0 and 1");
      validateTraffic(peer.traffic, peer.peer_id);
    }

    assertContract(Array.isArray(snapshot.logs), "snapshot logs must be an array");
    for (const entry of snapshot.logs) {
      assertContract(isRecord(entry), "log entry must be an object");
      requireInteger(entry.unix_ms, "invalid log timestamp");
      requireEnum(entry.level, LOG_LEVELS, "unknown log level");
      requireString(entry.target, "log target is missing");
      requireString(entry.message, "log message is missing");
    }

    if (snapshot.identity !== null) {
      assertContract(isRecord(snapshot.identity), "identity snapshot must be an object or null");
      requireString(snapshot.identity.node_id, "identity node_id is missing", true);
      assertContract(snapshot.identity.display_name === null || typeof snapshot.identity.display_name === "string", "invalid identity display_name");
      requireString(snapshot.identity.public_key_fingerprint, "identity fingerprint is missing", true);
    }

    assertContract(Array.isArray(snapshot.embedded_services), "embedded_services must be an array");
    for (const service of snapshot.embedded_services) {
      assertContract(isRecord(service), "embedded service must be an object");
      requireString(service.network_name, "embedded service network_name is missing", true);
      requireEnum(service.status, SERVICE_STATUSES, "unknown embedded service status");
      assertContract(service.signaling_addr === null || typeof service.signaling_addr === "string", "invalid embedded signaling_addr");
      assertContract(service.relay_addr === null || typeof service.relay_addr === "string", "invalid embedded relay_addr");
    }

    if (snapshot.ui_mode !== null) {
      assertContract(isRecord(snapshot.ui_mode), "ui_mode must be an object or null");
      requireEnum(snapshot.ui_mode.requested, UI_MODE_REQUESTS, "unknown requested UI mode");
      requireEnum(snapshot.ui_mode.effective, UI_MODES, "unknown effective UI mode");
      requireEnum(snapshot.ui_mode.display, DISPLAY_ENVIRONMENTS, "unknown display environment");
      requireBoolean(snapshot.ui_mode.gpui_available, "ui_mode gpui_available is missing");
    }

    if (snapshot.last_error !== null) {
      assertContract(isRecord(snapshot.last_error), "last_error must be an object or null");
      requireEnum(snapshot.last_error.code, PROBLEM_CODES, "unknown last_error code");
      requireString(snapshot.last_error.message, "last_error message is missing");
      assertContract(snapshot.last_error.network_name === null || typeof snapshot.last_error.network_name === "string", "invalid last_error network_name");
      requireBoolean(snapshot.last_error.retryable, "last_error retryable is missing");
    }
    return snapshot;
  }

  function validateSnapshotResponse(response) {
    assertContract(response && typeof response === "object", "snapshot response is missing");
    assertContract(
      response.schema_version === HTTP_SCHEMA_VERSION,
      "snapshot HTTP schema mismatch: expected=" + HTTP_SCHEMA_VERSION + " actual=" + response.schema_version,
    );
    return validateSnapshotPayload(response.snapshot);
  }

  function permissionGranted(snapshot, action) {
    return Array.isArray(snapshot.permissions.permissions) &&
      snapshot.permissions.permissions.includes(action.permission);
  }

  function availabilityReason(action, snapshot, selectedNetwork) {
    if (!permissionGranted(snapshot, action)) {
      return "permission_denied";
    }
    switch (action.availability) {
      case "always":
        return null;
      case "node_running":
        return snapshot.lifecycle.phase === "running" ? null : "node_not_running";
      case "network_online":
        return snapshot.networks.some((network) => network.status === "online")
          ? null
          : "no_online_network";
      case "network_selected": {
        if (!selectedNetwork) {
          return "no_network_selected";
        }
        return snapshot.networks.some((network) => network.name === selectedNetwork)
          ? null
          : "no_network_selected";
      }
      default:
        throw new Error("node-ui-contract: unknown availability " + action.availability);
    }
  }

  function executionBlockReason(action, snapshot, selectedNetwork) {
    const reason = availabilityReason(action, snapshot, selectedNetwork);
    if (reason) {
      return reason;
    }
    if (action.backend_status !== "implemented") {
      return "backend_unavailable";
    }
    return null;
  }

  function blockReasonText(reason) {
    switch (reason) {
      case "permission_denied":
        return "共享权限快照未授予此动作。";
      case "node_not_running":
        return "节点尚未运行。";
      case "no_online_network":
        return "没有在线网络。";
      case "no_network_selected":
        return "请先选择网络。";
      case "backend_unavailable":
        return "后端成熟度为 Unavailable，已禁止伪成功。";
      default:
        return "";
    }
  }

  function defaultNetworkName(snapshot, selectedNetwork) {
    if (selectedNetwork && snapshot.networks.some((network) => network.name === selectedNetwork)) {
      return selectedNetwork;
    }
    const online = snapshot.networks.find((network) => network.status === "online");
    if (online) {
      return online.name;
    }
    return snapshot.networks.length ? snapshot.networks[0].name : "";
  }

  function defaultFieldValue(action, field, snapshot, selectedNetwork) {
    const network = defaultNetworkName(snapshot, selectedNetwork);
    if (field.id === "signaling_port") return "6666";
    if (field.id === "relay_port") return "6667";
    if (field.id === "timeout_ms") return "1500";
    if (field.id === "auto_connect" || field.id === "start" || field.id === "is_exit_node") return true;
    if (field.id === "all" || field.id === "enabled") return field.id === "enabled";
    if (field.id === "network_name") return network;
    if (field.input === "network_selection") return network || "default";
    if (field.id === "mode") {
      const config = (snapshot.configuration && snapshot.configuration.networks || [])
        .find((entry) => entry.network_name === network);
      return config && config.routing_mode || "mesh_only";
    }
    if (field.id === "exit_node") {
      const config = (snapshot.configuration && snapshot.configuration.networks || [])
        .find((entry) => entry.network_name === network);
      return config && config.exit_node || "";
    }
    if (field.id === "role") {
      return action.id === "set_relay_role" ? "relay" : "exit_node";
    }
    return "";
  }

  function blank(value) {
    return value === undefined || value === null || String(value).trim() === "";
  }

  function booleanValue(value) {
    if (typeof value === "boolean") return value;
    const normalized = String(value === undefined || value === null ? "" : value).trim().toLowerCase();
    if (["true", "1", "yes", "on"].includes(normalized)) return true;
    if (["false", "0", "no", "off", ""].includes(normalized)) return false;
    throw new Error("布尔值无效: " + value);
  }

  function splitNames(value) {
    if (Array.isArray(value)) {
      return value.map((item) => String(item).trim()).filter(Boolean);
    }
    return String(value || "").split(",").map((item) => item.trim()).filter(Boolean);
  }

  function networkSelection(value) {
    const normalized = String(value || "").trim();
    if (normalized === "none") return "none";
    if (normalized === "default") return "default";
    if (normalized === "all") return "all";
    const names = splitNames(normalized);
    if (!names.length) throw new Error("网络选择不能为空");
    return { named: names };
  }

  function networkNameValidation(value) {
    const name = String(value).trim();
    return /^[A-Za-z0-9._-]+$/.test(name) && name !== "." && name !== ".."
      ? null
      : "只能包含 ASCII 字母、数字、点、短横线和下划线";
  }

  function validateNetworkSelection(value) {
    const normalized = String(value || "").trim();
    if (["none", "default", "all"].includes(normalized)) return [];
    const names = splitNames(normalized);
    if (!names.length) return ["不能为空"];
    const errors = [];
    const seen = new Set();
    for (const name of names) {
      const invalid = networkNameValidation(name);
      if (invalid) errors.push(name + " " + invalid);
      if (seen.has(name)) errors.push("网络重复: " + name);
      seen.add(name);
    }
    return errors;
  }

  function validateActionValues(action, values) {
    const errors = [];
    for (const field of action.fields) {
      const value = values[field.id];
      if (field.required && blank(value)) {
        errors.push(label(field.label_key) + "不能为空");
        continue;
      }
      if (blank(value)) continue;
      try {
        if (field.validation === "network_name") {
          const invalid = networkNameValidation(value);
          if (invalid) errors.push(label(field.label_key) + invalid);
        } else if (field.validation === "port" || field.validation === "distinct_ports") {
          const port = Number(value);
          if (!Number.isInteger(port) || port < 1 || port > 65535) {
            errors.push(label(field.label_key) + "必须是 1-65535 的端口");
          }
        } else if (field.validation === "network_selection") {
          for (const invalid of validateNetworkSelection(value)) {
            errors.push(label(field.label_key) + ": " + invalid);
          }
        } else if (field.validation === "role") {
          if (!["member", "discovery", "relay", "exit_node"].includes(String(value))) {
            errors.push("角色值无效");
          }
        } else if (field.validation === "routing_mode") {
          if (!["mesh_only", "bypass_mainland", "global_proxy"].includes(String(value))) {
            errors.push("路由模式无效");
          }
        } else if (field.validation === "positive_timeout") {
          const timeout = Number(value);
          if (!Number.isInteger(timeout) || timeout <= 0) {
            errors.push(label(field.label_key) + "必须是正整数");
          }
        }
      } catch (error) {
        errors.push(error.message);
      }
    }
    if (action.id === "create_network") {
      if (Number(values.signaling_port) === Number(values.relay_port)) {
        errors.push("信令端口和中继端口不能相同");
      }
    }
    if (action.id === "stop_networks" && !booleanValue(values.all) && !splitNames(values.networks).length) {
      errors.push("停止网络时必须指定网络名称，或勾选全部网络");
    }
    return errors;
  }

  function required(values, fieldId) {
    if (blank(values[fieldId])) {
      throw new Error("缺少字段 " + fieldId);
    }
    return values[fieldId];
  }

  function optional(values, fieldId) {
    return blank(values[fieldId]) ? null : String(values[fieldId]).trim();
  }

  function commandForAction(action, values) {
    const errors = validateActionValues(action, values);
    if (errors.length) {
      throw new Error(errors.join("；"));
    }
    switch (action.id) {
      case "refresh_snapshot":
        return { command: "get_snapshot" };
      case "create_network":
        return {
          command: "create_network",
          payload: {
            name: String(required(values, "name")).trim(),
            signaling_port: Number(required(values, "signaling_port")),
            relay_port: Number(required(values, "relay_port")),
            external_host: optional(values, "external_host"),
            auto_connect: booleanValue(values.auto_connect),
          },
        };
      case "join_network":
        return {
          command: "join_network",
          payload: {
            target: String(required(values, "target")).trim(),
            network_name: optional(values, "network_name"),
            start: booleanValue(values.start),
          },
        };
      case "start_networks":
        return {
          command: "start_networks",
          payload: { selection: networkSelection(required(values, "selection")) },
        };
      case "stop_networks": {
        const all = booleanValue(values.all);
        return {
          command: "stop_networks",
          payload: { networks: all ? [] : splitNames(values.networks), all },
        };
      }
      case "probe_peer":
        return {
          command: "probe_peer",
          payload: {
            network_name: String(required(values, "network_name")).trim(),
            target: String(required(values, "target")).trim(),
            timeout_ms: Number(required(values, "timeout_ms")),
          },
        };
      case "update_routing":
        return {
          command: "update_routing",
          payload: {
            network_name: String(required(values, "network_name")).trim(),
            mode: String(required(values, "mode")),
            exit_node: optional(values, "exit_node"),
            is_exit_node: booleanValue(values.is_exit_node),
          },
        };
      case "set_exit_node_role":
      case "set_relay_role":
        return {
          command: "set_network_role",
          payload: {
            network_name: String(required(values, "network_name")).trim(),
            role: String(required(values, "role")),
            enabled: booleanValue(required(values, "enabled")),
          },
        };
      case "shutdown_node":
        return { command: "shutdown" };
      default:
        throw new Error("拒绝为未知动作构造命令: " + action.id);
    }
  }

  function commandEnvelope(requestId, action, values) {
    assertContract(Number.isSafeInteger(requestId) && requestId > 0, "request id must be a positive safe integer");
    return {
      schema_version: HTTP_SCHEMA_VERSION,
      request: {
        request_id: requestId,
        command: commandForAction(action, values),
      },
    };
  }

  function validateCommandResponse(response, requestId) {
    assertContract(response && typeof response === "object", "command response is missing");
    assertContract(
      response.schema_version === HTTP_SCHEMA_VERSION,
      "command HTTP schema mismatch: expected=" + HTTP_SCHEMA_VERSION + " actual=" + response.schema_version,
    );
    assertContract(response.result && typeof response.result === "object", "command result is missing");
    assertContract(
      Number.isSafeInteger(response.result.request_id) && response.result.request_id > 0,
      "command request id must be a positive integer",
    );
    assertContract(response.result.request_id === requestId, "command request id mismatch");
    assertContract(response.result.outcome && response.result.outcome.status, "command outcome is missing");
    const outcome = response.result.outcome;
    requireEnum(outcome.status, COMMAND_OUTCOMES, "unknown command outcome");
    switch (outcome.status) {
      case "snapshot":
        validateSnapshotPayload(outcome.data);
        break;
      case "ui_manifest":
        validateManifestResponse({ schema_version: HTTP_SCHEMA_VERSION, manifest: outcome.data });
        break;
      case "rejected":
        assertContract(isRecord(outcome.data), "rejected outcome is missing its problem");
        requireEnum(outcome.data.code, PROBLEM_CODES, "unknown rejected problem code");
        requireString(outcome.data.message, "rejected problem message is missing");
        assertContract(outcome.data.network_name === null || typeof outcome.data.network_name === "string", "invalid rejected problem network_name");
        requireBoolean(outcome.data.retryable, "rejected problem retryable is missing");
        break;
      case "probe":
        assertContract(isRecord(outcome.data), "probe outcome is missing its data");
        requireString(outcome.data.peer_id, "probe peer_id is missing", true);
        requireString(outcome.data.virtual_ipv6, "probe virtual_ipv6 is missing", true);
        assertContract(outcome.data.endpoint === null || typeof outcome.data.endpoint === "string", "invalid probe endpoint");
        assertContract(outcome.data.rtt_us === null || (Number.isSafeInteger(outcome.data.rtt_us) && outcome.data.rtt_us >= 0), "invalid probe rtt_us");
        requireBoolean(outcome.data.p2p_active, "probe p2p_active is missing");
        requireBoolean(outcome.data.timed_out, "probe timed_out is missing");
        break;
      case "accepted":
        requireString(outcome.data && outcome.data.operation_id, "accepted operation_id is missing", true);
        break;
      case "completed":
        requireString(outcome.data && outcome.data.message, "completed message is missing");
        break;
    }
    return outcome;
  }

  function httpErrorText(payload, fallback) {
    if (payload && typeof payload === "object") {
      if (payload.problem && payload.problem.code && ERROR_TEXT[payload.problem.code]) {
        return ERROR_TEXT[payload.problem.code] + " " + (payload.problem.message || "");
      }
      if (payload.code && ERROR_TEXT[payload.code]) {
        return ERROR_TEXT[payload.code] + (payload.message ? " " + payload.message : "");
      }
    }
    return fallback || "请求失败";
  }

  function outcomeErrorText(outcome) {
    if (outcome && outcome.status === "rejected" && outcome.data) {
      return httpErrorText({ problem: outcome.data }, "节点命令被拒绝");
    }
    return "节点返回了未预期的结果";
  }

  function networkRows(snapshot) {
    return snapshot.networks.map((network) => ({
      title: network.name,
      detail: [
        "状态 " + network.status,
        "角色 " + ((network.roles || []).join(", ") || "member"),
        "Peers " + (network.peer_count || 0),
        "IPv6 " + (network.assigned_ipv6 || "--"),
      ].join(" · "),
    }));
  }

  function peerRows(snapshot) {
    return snapshot.peers.map((peer) => ({
      title: peer.peer_id || peer.virtual_ipv6,
      detail: [
        peer.network_name,
        peer.virtual_ipv6,
        peer.online ? "online" : "offline",
        peer.p2p_active ? "P2P" : "relay",
        peer.rtt_us === null || peer.rtt_us === undefined ? "--" : peer.rtt_us + "us",
      ].join(" · "),
    }));
  }

  function configRows(snapshot) {
    return ((snapshot.configuration && snapshot.configuration.networks) || []).map((network) => ({
      title: network.network_name,
      detail: [
        network.coordinator_url,
        "routing=" + network.routing_mode,
        "exit=" + (network.exit_node || "--"),
        "ExitNode=" + (network.exit_node_enabled ? "enabled" : "disabled"),
      ].join(" · "),
    }));
  }

  function serviceRows(snapshot) {
    return (snapshot.embedded_services || []).map((service) => ({
      title: service.network_name,
      detail: [
        service.status,
        "signal=" + (service.signaling_addr || "--"),
        "relay=" + (service.relay_addr || "--"),
      ].join(" · "),
    }));
  }

  function trafficTotals(snapshot) {
    return snapshot.networks.reduce((total, network) => {
      const traffic = network.traffic || {};
      total.rx += Number(traffic.tun_rx_bytes || 0);
      total.tx += Number(traffic.tun_tx_bytes || 0);
      total.errors += Number(traffic.error_count || 0);
      return total;
    }, { rx: 0, tx: 0, errors: 0 });
  }

  function projectPage(page, snapshot, selectedNetwork) {
    assertContract(REQUIRED_PAGES.includes(page.id), "unknown page: " + page.id);
    const selected = defaultNetworkName(snapshot, selectedNetwork);
    let rows = [];
    let empty = "";
    switch (page.id) {
      case "overview":
        rows = [
          { title: "节点生命周期", detail: snapshot.lifecycle.phase + " · active=" + snapshot.lifecycle.active_networks + " · configured=" + snapshot.lifecycle.configured_networks },
          { title: "网络", detail: snapshot.networks.length + " 个 · Peers " + snapshot.peers.length },
          { title: "最近错误", detail: snapshot.last_error ? snapshot.last_error.code + " · " + snapshot.last_error.message : "无" },
        ];
        break;
      case "networks":
        rows = networkRows(snapshot);
        empty = "当前没有配置或运行的网络。";
        break;
      case "peers":
        rows = peerRows(snapshot);
        empty = "当前没有对等节点。";
        break;
      case "topology":
        rows = snapshot.networks.map((network) => ({
          title: network.name,
          detail: "本节点 ↔ " + snapshot.peers.filter((peer) => peer.network_name === network.name).length + " 个 Peer",
        }));
        empty = "没有可绘制的拓扑节点。";
        break;
      case "traffic": {
        const totals = trafficTotals(snapshot);
        rows = [
          { title: "TUN 接收", detail: totals.rx + " bytes" },
          { title: "TUN 发送", detail: totals.tx + " bytes" },
          { title: "错误计数", detail: String(totals.errors) },
        ];
        break;
      }
      case "routing":
        rows = configRows(snapshot);
        empty = "没有可用路由配置。";
        break;
      case "exit_node": {
        const capabilities = snapshot.exit_node_capabilities || {};
        rows = [
          { title: "平台能力", detail: capabilities.supported ? "supported" : "unsupported · " + (capabilities.unsupported_reasons || []).join(", ") },
          { title: "转发/NAT/清理", detail: "ipv4=" + !!capabilities.ipv4_forwarding + " · ipv6=" + !!capabilities.ipv6_forwarding + " · nat=" + !!capabilities.nat + " · cleanup=" + !!capabilities.cleanup },
          { title: "当前网络角色", detail: (snapshot.networks.find((network) => network.name === selected)?.roles || []).join(", ") || "member" },
        ];
        break;
      }
      case "relay":
        rows = serviceRows(snapshot);
        empty = "当前没有内嵌中继服务。";
        break;
      case "logs":
        rows = snapshot.logs.map((entry) => ({
          title: entry.level + " · " + entry.target,
          detail: entry.message,
        }));
        empty = "当前没有日志。";
        break;
      case "identity":
        rows = snapshot.identity ? [
          { title: "Node ID", detail: snapshot.identity.node_id },
          { title: "显示名称", detail: snapshot.identity.display_name || "--" },
          { title: "公钥指纹", detail: snapshot.identity.public_key_fingerprint },
        ] : [];
        empty = "节点身份尚未初始化。";
        break;
      case "settings":
        rows = [
          { title: "访问模型", detail: snapshot.permissions.access_model || "unknown" },
          { title: "权限", detail: snapshot.permissions.permissions.join(", ") || "无" },
          { title: "UI 模式", detail: snapshot.ui_mode ? snapshot.ui_mode.effective : "unknown" },
        ];
        break;
      default:
        throw new Error("node-ui-contract: unknown page " + page.id);
    }
    return {
      page_id: page.id,
      title: label(page.title_key),
      selected_network: selected || null,
      rows,
      empty,
      actions: page.actions.map((action) => ({
        id: action.id,
        label: label(action.label_key),
        permission: action.permission,
        availability: action.availability,
        backend_status: action.backend_status,
        block_reason: executionBlockReason(action, snapshot, selected || null),
      })),
    };
  }

  return Object.freeze({
    HTTP_SCHEMA_VERSION,
    MANIFEST_SCHEMA_VERSION,
    REQUIRED_PAGES,
    ACTION_CONTRACT,
    ERROR_TEXT,
    label,
    validateManifestResponse,
    validateSnapshotResponse,
    permissionGranted,
    availabilityReason,
    executionBlockReason,
    blockReasonText,
    defaultNetworkName,
    defaultFieldValue,
    validateActionValues,
    commandForAction,
    commandEnvelope,
    validateCommandResponse,
    httpErrorText,
    outcomeErrorText,
    projectPage,
  });
});
