import type {
  NodeApiCommandResponse,
  NodeApiManifestResponse,
  NodeApiSnapshotResponse,
  NodeCommand,
  NodeCommandHttpRequest,
  NodeCommandOutcome,
  ConfigurationApplyResult,
  ConfigurationDocument,
  ConfigurationHistoryEntry,
  ConfigurationPreview,
  NodeNetworkSelection,
  NodeProblem,
  NodeSnapshot,
  NodeUiAction,
  NodeUiField,
  NodeUiManifest,
  NodeUiPage,
} from "../../../contracts/typescript/node-contract-v1";
import { ContractError } from "../api/client";

export const HTTP_SCHEMA_VERSION = 1;
export const MANIFEST_SCHEMA_VERSION = 1;

export const REQUIRED_PAGES = [
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
] as const;

const EXPECTED_ACTIONS: Record<string, string> = {
  refresh_snapshot: "get_snapshot",
  get_node_diagnostics: "get_node_diagnostics",
  start_node: "start_node",
  stop_node: "stop_node",
  restart_node: "restart_node",
  cleanup_node_residuals: "cleanup_node_residuals",
  create_network: "create_network",
  join_network: "join_network",
  start_networks: "start_networks",
  stop_networks: "stop_networks",
  restart_networks: "restart_networks",
  probe_peer: "probe_peer",
  get_blacklist: "get_blacklist",
  ban_client: "ban_client",
  unban_client: "unban_client",
  update_routing: "update_routing",
  set_exit_node_role: "set_network_role",
  set_relay_role: "set_network_role",
  get_network_allocation: "get_network_allocation",
  update_network_allocation: "update_network_allocation",
  get_configuration: "get_configuration",
  preview_configuration: "preview_configuration",
  apply_configuration: "apply_configuration",
  rollback_configuration: "rollback_configuration",
  get_configuration_history: "get_configuration_history",
  shutdown_node: "shutdown",
};

const PERMISSIONS = new Set([
  "view_state",
  "manage_networks",
  "change_routing",
  "manage_roles",
  "probe_peers",
  "stop_node",
  "view_logs",
]);
const AVAILABILITIES = new Set([
  "always",
  "node_running",
  "network_online",
  "network_selected",
]);
const BACKEND_STATUSES = new Set(["implemented", "unavailable"]);
const INPUT_KINDS = new Set([
  "boolean",
  "client_id",
  "external_host",
  "network_mode",
  "network_name",
  "network_selection",
  "peer_target",
  "port",
  "role",
  "routing_mode",
  "url_or_ticket",
  "timeout_ms",
  "revision",
  "configuration_json",
]);
const VALIDATIONS = new Set([
  "none",
  "non_empty",
  "network_name",
  "port",
  "distinct_ports",
  "network_mode",
  "network_selection",
  "role",
  "routing_mode",
  "positive_timeout",
  "revision",
  "configuration_json",
]);
const OUTCOME_STATUSES = new Set([
  "snapshot",
  "ui_manifest",
  "diagnostics",
  "cleanup",
  "blacklist",
  "network_allocation",
  "probe",
  "network_created",
  "configuration",
  "configuration_preview",
  "configuration_applied",
  "configuration_history",
  "accepted",
  "completed",
  "rejected",
]);
const LIFECYCLE_PHASES = new Set([
  "stopped",
  "starting",
  "running",
  "stopping",
  "failed",
]);
const NETWORK_STATUSES = new Set([
  "offline",
  "connecting",
  "online",
  "degraded",
  "stopping",
  "failed",
]);
const NETWORK_ROLES = new Set(["member", "discovery", "relay", "exit_node"]);

export type ActionValues = Record<string, string | boolean>;
export type BlockReason =
  | "permission_denied"
  | "node_not_running"
  | "no_online_network"
  | "no_network_selected"
  | "backend_unavailable";

function fail(message: string): never {
  throw new ContractError(message);
}

function record(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function stringValue(
  value: unknown,
  message: string,
  nonEmpty = false,
): string {
  if (typeof value !== "string" || (nonEmpty && value.length === 0))
    fail(message);
  return value;
}

function booleanValueRaw(value: unknown, message: string): boolean {
  if (typeof value !== "boolean") fail(message);
  return value;
}

function nonNegativeInteger(value: unknown, message: string): number {
  if (!Number.isSafeInteger(value) || Number(value) < 0) fail(message);
  return Number(value);
}

function arrayValue(value: unknown, message: string): unknown[] {
  if (!Array.isArray(value)) fail(message);
  return value;
}

function unique(values: string[], kind: string): void {
  const seen = new Set<string>();
  for (const value of values) {
    if (!value) fail(`${kind} contains an empty value`);
    if (seen.has(value)) fail(`duplicate ${kind}: ${value}`);
    seen.add(value);
  }
}

export function i18n(): I18nRuntime {
  if (!window.FuckIPV6I18n) throw new Error("shared i18n runtime is missing");
  return window.FuckIPV6I18n;
}

export function label(locale: Locale, key: string): string {
  return i18n().t(locale, key);
}

export function validateManifestResponse(
  response: NodeApiManifestResponse,
): NodeUiManifest {
  if (!record(response)) fail("manifest response is missing");
  if (response.schema_version !== HTTP_SCHEMA_VERSION)
    fail("HTTP schema mismatch");
  if (!record(response.manifest)) fail("manifest payload is missing");
  const manifest = response.manifest as unknown as NodeUiManifest;
  if (manifest.schema_version !== MANIFEST_SCHEMA_VERSION)
    fail("manifest schema mismatch");
  if (!Array.isArray(manifest.pages)) fail("manifest pages must be an array");
  const pageIds = manifest.pages.map((page) => page.id);
  unique(pageIds, "page id");
  if (JSON.stringify(pageIds) !== JSON.stringify(REQUIRED_PAGES))
    fail("page set/order mismatch");

  const seenActions = new Set<string>();
  for (const page of manifest.pages) {
    if (!record(page)) fail(`page ${String(page)} is invalid`);
    if (!REQUIRED_PAGES.includes(page.id as (typeof REQUIRED_PAGES)[number]))
      fail(`unknown page ${page.id}`);
    stringValue(page.nav_label_key, "page nav label key missing", true);
    stringValue(page.title_key, "page title key missing", true);
    if (!Array.isArray(page.actions)) fail(`page actions invalid: ${page.id}`);
    const pageActionIds = page.actions.map((action) => action.id);
    unique(pageActionIds, `action id for ${page.id}`);
    for (const action of page.actions) validateAction(action, seenActions);
  }
  const expected = Object.keys(EXPECTED_ACTIONS).sort();
  const actual = [...seenActions].sort();
  if (JSON.stringify(actual) !== JSON.stringify(expected))
    fail("action set mismatch");

  const keys = manifest.pages.flatMap((page) => [
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
  for (const locale of i18n().locales) {
    for (const key of new Set(keys)) {
      const text = label(locale, key);
      if (!text || text === key) fail(`missing ${locale} label: ${key}`);
    }
  }
  return manifest;
}

function validateAction(action: NodeUiAction, seen: Set<string>): void {
  if (!record(action)) fail("action must be an object");
  stringValue(action.id, "action id missing", true);
  stringValue(action.label_key, "action label key missing", true);
  if (seen.has(action.id)) fail(`duplicate action id: ${action.id}`);
  seen.add(action.id);
  const expectedCommand = EXPECTED_ACTIONS[action.id];
  if (!expectedCommand) fail(`unknown action ${action.id}`);
  if (action.command !== expectedCommand)
    fail(`command mismatch for ${action.id}`);
  if (!PERMISSIONS.has(action.permission))
    fail(`unknown permission for ${action.id}`);
  if (!AVAILABILITIES.has(action.availability))
    fail(`unknown availability for ${action.id}`);
  if (!BACKEND_STATUSES.has(action.backend_status))
    fail(`unknown backend status for ${action.id}`);
  if (!Array.isArray(action.fields)) fail(`fields missing for ${action.id}`);
  unique(
    action.fields.map((field) => field.id),
    `field id for ${action.id}`,
  );
  for (const field of action.fields) validateField(action.id, field);
  if (
    action.command === "shutdown" &&
    action.confirmation?.destructive !== true
  ) {
    fail("shutdown requires destructive confirmation");
  }
}

function validateField(actionId: string, field: NodeUiField): void {
  if (!record(field)) fail(`field invalid for ${actionId}`);
  stringValue(field.id, `field id missing for ${actionId}`, true);
  stringValue(
    field.label_key,
    `field label key missing for ${actionId}.${field.id}`,
    true,
  );
  if (!INPUT_KINDS.has(field.input))
    fail(`unknown input for ${actionId}.${field.id}`);
  if (!VALIDATIONS.has(field.validation))
    fail(`unknown validation for ${actionId}.${field.id}`);
  booleanValueRaw(
    field.required,
    `required must be boolean for ${actionId}.${field.id}`,
  );
}

export function validateSnapshotResponse(
  response: NodeApiSnapshotResponse,
): NodeSnapshot {
  if (!record(response)) fail("snapshot response is missing");
  if (response.schema_version !== HTTP_SCHEMA_VERSION)
    fail("snapshot HTTP schema mismatch");
  return validateSnapshot(response.snapshot);
}

export function validateSnapshot(snapshot: NodeSnapshot): NodeSnapshot {
  if (!record(snapshot)) fail("snapshot payload is missing");
  nonNegativeInteger(snapshot.revision, "snapshot revision invalid");
  if (!record(snapshot.lifecycle)) fail("snapshot lifecycle missing");
  if (!LIFECYCLE_PHASES.has(snapshot.lifecycle.phase))
    fail("lifecycle phase invalid");
  nonNegativeInteger(
    snapshot.lifecycle.configured_networks,
    "configured_networks invalid",
  );
  nonNegativeInteger(
    snapshot.lifecycle.active_networks,
    "active_networks invalid",
  );
  arrayValue(snapshot.lifecycle.failed_networks, "failed_networks invalid");
  arrayValue(snapshot.networks, "networks invalid");
  arrayValue(snapshot.peers, "peers invalid");
  arrayValue(snapshot.logs, "logs invalid");
  arrayValue(snapshot.embedded_services, "embedded_services invalid");
  if (
    !record(snapshot.configuration) ||
    !Array.isArray(snapshot.configuration.networks)
  ) {
    fail("configuration networks invalid");
  }
  if (
    !record(snapshot.permissions) ||
    !Array.isArray(snapshot.permissions.permissions)
  ) {
    fail("permissions invalid");
  }
  for (const network of snapshot.networks) {
    if (!record(network)) fail("network snapshot invalid");
    stringValue(network.name, "network name invalid", true);
    if (!NETWORK_STATUSES.has(network.status))
      fail(`network status invalid: ${network.name}`);
    arrayValue(network.roles, `network roles invalid: ${network.name}`);
    for (const role of network.roles)
      if (!NETWORK_ROLES.has(role)) fail(`network role invalid: ${role}`);
    if (!record(network.traffic))
      fail(`network traffic invalid: ${network.name}`);
  }
  for (const peer of snapshot.peers) {
    if (!record(peer)) fail("peer snapshot invalid");
    stringValue(peer.peer_id, "peer id invalid", true);
    stringValue(peer.network_name, "peer network invalid", true);
    stringValue(peer.virtual_ipv6, "peer virtual IPv6 invalid", true);
    arrayValue(peer.endpoints, `peer endpoints invalid: ${peer.peer_id}`);
    if (!record(peer.traffic)) fail(`peer traffic invalid: ${peer.peer_id}`);
  }
  for (const entry of snapshot.logs) {
    if (!record(entry)) fail("log entry invalid");
    stringValue(entry.message, "log message invalid");
    stringValue(entry.target, "log target invalid");
    nonNegativeInteger(entry.unix_ms, "log timestamp invalid");
  }
  return snapshot;
}

export function primaryStatus(
  snapshot: NodeSnapshot,
  locale: Locale,
): { id: string; label: string; detail: string } {
  const phase = snapshot.lifecycle.phase;
  const controlHealth = snapshot.control_health ?? "unknown";
  let id: string;
  if (phase === "starting") id = "starting";
  else if (phase === "stopping") id = "stopping";
  else if (phase === "failed") id = "failed";
  else if (phase === "running") {
    const degraded =
      snapshot.last_error != null ||
      snapshot.lifecycle.failed_networks.length > 0 ||
      snapshot.networks.some((network) =>
        ["connecting", "degraded", "stopping", "failed"].includes(
          network.status,
        ),
      ) ||
      (snapshot.embedded_services ?? []).some((service) =>
        ["starting", "stopping", "failed"].includes(service.status),
      );
    id = degraded
      ? "degraded"
      : controlHealth === "unavailable"
        ? "control_unavailable"
        : "running";
  } else if (phase === "stopped") {
    const uninitialized =
      snapshot.identity == null &&
      (snapshot.configuration?.networks.length ?? 0) === 0 &&
      snapshot.networks.length === 0 &&
      snapshot.lifecycle.configured_networks === 0;
    id = uninitialized ? "uninitialized" : "stopped";
  } else {
    fail(`unknown lifecycle phase ${phase}`);
  }
  return {
    id,
    label: label(locale, `status.primary.${id}`),
    detail: label(locale, `status.primary.${id}.detail`),
  };
}

export function defaultNetworkName(
  snapshot: NodeSnapshot,
  selected: string,
): string {
  if (
    selected &&
    snapshot.networks.some((network) => network.name === selected)
  )
    return selected;
  return (
    snapshot.networks.find((network) => network.status === "online")?.name ??
    snapshot.networks[0]?.name ??
    ""
  );
}

export function executionBlockReason(
  action: NodeUiAction,
  snapshot: NodeSnapshot,
  selectedNetwork: string,
): BlockReason | null {
  if (!snapshot.permissions?.permissions.includes(action.permission))
    return "permission_denied";
  if (
    action.availability === "node_running" &&
    snapshot.lifecycle.phase !== "running"
  )
    return "node_not_running";
  if (
    action.availability === "network_online" &&
    !snapshot.networks.some((network) => network.status === "online")
  ) {
    return "no_online_network";
  }
  if (
    action.availability === "network_selected" &&
    !snapshot.networks.some((network) => network.name === selectedNetwork)
  ) {
    return "no_network_selected";
  }
  if (action.backend_status !== "implemented") return "backend_unavailable";
  return null;
}

export function blockReasonText(
  reason: BlockReason | null,
  locale: Locale,
): string {
  return reason ? label(locale, `blocked.${reason}`) : "";
}

export function defaultFieldValue(
  action: NodeUiAction,
  field: NodeUiField,
  snapshot: NodeSnapshot,
  selectedNetwork: string,
): string | boolean {
  const network = defaultNetworkName(snapshot, selectedNetwork);
  if (field.id === "signaling_port") return "6666";
  if (field.id === "relay_port") return "6667";
  if (field.id === "timeout_ms") return "1500";
  if (
    field.id === "auto_connect" ||
    field.id === "start" ||
    field.id === "is_exit_node"
  )
    return true;
  if (field.id === "all") return action.id === "cleanup_node_residuals";
  if (field.id === "enabled") return false;
  if (field.id === "network_name") return network;
  if (field.input === "network_selection") return network || "default";
  if (field.input === "network_mode") return "dual_stack";
  if (field.id === "ipv4_pool") return "10.8.0.0/24";
  if (field.id === "ipv6_pool") return "fd00::/120";
  const config = (snapshot.configuration?.networks ?? []).find(
    (entry) => entry.network_name === network,
  );
  if (field.id === "mode") return config?.routing_mode ?? "mesh_only";
  if (field.id === "exit_node") return config?.exit_node ?? "";
  if (field.id === "role")
    return action.id === "set_relay_role" ? "relay" : "exit_node";
  return "";
}

export function initialActionValues(
  action: NodeUiAction,
  snapshot: NodeSnapshot,
  selectedNetwork: string,
): ActionValues {
  return Object.fromEntries(
    action.fields.map((field) => [
      field.id,
      defaultFieldValue(action, field, snapshot, selectedNetwork),
    ]),
  );
}

function blank(value: string | boolean | undefined): boolean {
  return (
    value === undefined || (typeof value === "string" && value.trim() === "")
  );
}

function splitNames(value: string | boolean | undefined): string[] {
  return String(value ?? "")
    .split(",")
    .map((item) => item.trim())
    .filter(Boolean);
}

function networkSelection(
  value: string | boolean | undefined,
): NodeNetworkSelection {
  const normalized = String(value ?? "").trim();
  if (normalized === "none" || normalized === "default" || normalized === "all")
    return normalized;
  return { named: splitNames(value) };
}

function booleanValue(
  value: string | boolean | undefined,
  locale: Locale,
): boolean {
  if (typeof value === "boolean") return value;
  const normalized = String(value ?? "")
    .trim()
    .toLowerCase();
  if (["true", "1", "yes", "on"].includes(normalized)) return true;
  if (["false", "0", "no", "off", ""].includes(normalized)) return false;
  throw new Error(
    i18n().tr(locale, "validation.boolean_invalid", { value: normalized }),
  );
}

function required(
  values: ActionValues,
  id: string,
  locale: Locale,
): string | boolean {
  const value = values[id];
  if (blank(value))
    throw new Error(
      i18n().tr(locale, "validation.missing_field", { field: id }),
    );
  return value;
}

function optional(values: ActionValues, id: string): string | null {
  const value = values[id];
  return blank(value) ? null : String(value).trim();
}

function revisionValue(
  value: string | boolean,
  field: string,
  locale: Locale,
): number {
  const revision = Number(value);
  if (!Number.isSafeInteger(revision) || revision < 0) {
    throw new Error(
      i18n().tr(locale, "validation.revision_invalid", { field }),
    );
  }
  return revision;
}

function configurationObject(
  value: unknown,
  locale: Locale,
): Record<string, unknown> {
  if (!record(value)) {
    throw new Error(label(locale, "validation.configuration_object"));
  }
  const candidate = record(value.networks) ? value.networks : value;
  if (!record(candidate) || Object.keys(candidate).length === 0) {
    throw new Error(label(locale, "validation.configuration_networks_empty"));
  }
  for (const [name, network] of Object.entries(candidate)) {
    if (!/^[A-Za-z0-9._-]+$/.test(name) || name === "." || name === "..") {
      throw new Error(
        i18n().tr(locale, "validation.configuration_network_name", { name }),
      );
    }
    if (!record(network)) {
      throw new Error(
        i18n().tr(locale, "validation.configuration_network_object", { name }),
      );
    }
  }
  return candidate;
}

export function parseConfigurationNetworks(
  value: string,
  locale: Locale,
): Record<string, unknown> {
  let parsed: unknown;
  try {
    parsed = JSON.parse(value);
  } catch {
    throw new Error(label(locale, "validation.configuration_json_invalid"));
  }
  return configurationObject(parsed, locale);
}

function configurationDocument(value: unknown): ConfigurationDocument {
  if (
    !record(value) ||
    !Number.isSafeInteger(value.schema_version) ||
    !Number.isSafeInteger(value.revision) ||
    !record(value.networks)
  ) {
    fail("configuration document invalid");
  }
  return value as ConfigurationDocument;
}

function configurationPreview(value: unknown): ConfigurationPreview {
  if (
    !record(value) ||
    !configurationDocument(value.current) ||
    !configurationDocument(value.candidate)
  ) {
    fail("configuration preview invalid");
  }
  if (!Array.isArray(value.diffs) || !Array.isArray(value.validation_errors)) {
    fail("configuration preview details invalid");
  }
  return value as ConfigurationPreview;
}

function configurationApplied(value: unknown): ConfigurationApplyResult {
  if (
    !record(value) ||
    !configurationDocument(value.document) ||
    !Array.isArray(value.diffs)
  ) {
    fail("configuration apply result invalid");
  }
  return value as ConfigurationApplyResult;
}

function configurationHistory(value: unknown): ConfigurationHistoryEntry[] {
  if (!Array.isArray(value)) fail("configuration history invalid");
  for (const entry of value) {
    if (
      !record(entry) ||
      !Number.isSafeInteger(entry.revision) ||
      !Array.isArray(entry.changed_paths)
    ) {
      fail("configuration history entry invalid");
    }
  }
  return value as ConfigurationHistoryEntry[];
}

export function validateActionValues(
  action: NodeUiAction,
  values: ActionValues,
  locale: Locale,
): string[] {
  const errors: string[] = [];
  for (const field of action.fields) {
    const value = values[field.id];
    if (field.required && blank(value)) {
      errors.push(
        i18n().tr(locale, "validation.required", {
          field: label(locale, field.label_key),
        }),
      );
      continue;
    }
    if (blank(value)) continue;
    if (field.validation === "network_name") {
      const name = String(value).trim();
      if (!/^[A-Za-z0-9._-]+$/.test(name) || name === "." || name === "..") {
        errors.push(
          `${label(locale, field.label_key)} ${label(locale, "validation.network_name_invalid")}`,
        );
      }
    } else if (
      field.validation === "port" ||
      field.validation === "distinct_ports"
    ) {
      const port = Number(value);
      if (!Number.isInteger(port) || port < 1 || port > 65535) {
        errors.push(
          i18n().tr(locale, "validation.port_range", {
            field: label(locale, field.label_key),
          }),
        );
      }
    } else if (field.validation === "network_selection") {
      const normalized = String(value).trim();
      if (!["none", "default", "all"].includes(normalized)) {
        const names = splitNames(value);
        if (names.length === 0)
          errors.push(label(locale, "validation.network_selection_empty"));
        const seen = new Set<string>();
        for (const name of names) {
          if (
            !/^[A-Za-z0-9._-]+$/.test(name) ||
            name === "." ||
            name === ".."
          ) {
            errors.push(
              `${name} ${label(locale, "validation.network_name_invalid")}`,
            );
          }
          if (seen.has(name))
            errors.push(
              i18n().tr(locale, "validation.network_duplicate", { name }),
            );
          seen.add(name);
        }
      }
    } else if (
      field.validation === "role" &&
      !NETWORK_ROLES.has(String(value))
    ) {
      errors.push(label(locale, "validation.role_invalid"));
    } else if (
      field.validation === "routing_mode" &&
      !["mesh_only", "bypass_mainland", "global_proxy"].includes(String(value))
    ) {
      errors.push(label(locale, "validation.routing_invalid"));
    } else if (
      field.validation === "network_mode" &&
      !["ipv4_only", "ipv6_only", "dual_stack"].includes(String(value))
    ) {
      errors.push(label(locale, "validation.network_mode_invalid"));
    } else if (field.validation === "positive_timeout") {
      const timeout = Number(value);
      if (!Number.isInteger(timeout) || timeout <= 0) {
        errors.push(
          i18n().tr(locale, "validation.positive_integer", {
            field: label(locale, field.label_key),
          }),
        );
      }
    } else if (field.validation === "revision") {
      const revision = Number(value);
      if (!Number.isSafeInteger(revision) || revision < 0) {
        errors.push(
          i18n().tr(locale, "validation.revision_invalid", {
            field: label(locale, field.label_key),
          }),
        );
      }
    } else if (field.validation === "configuration_json") {
      try {
        parseConfigurationNetworks(String(value), locale);
      } catch (error) {
        errors.push(error instanceof Error ? error.message : String(error));
      }
    }
  }
  if (
    action.id === "create_network" &&
    Number(values.signaling_port) === Number(values.relay_port)
  ) {
    errors.push(label(locale, "validation.port_distinct"));
  }
  if (action.id === "update_network_allocation") {
    const mode = String(values.mode ?? "");
    if (["ipv4_only", "dual_stack"].includes(mode) && blank(values.ipv4_pool)) {
      errors.push(label(locale, "validation.ipv4_pool_required"));
    }
    if (["ipv6_only", "dual_stack"].includes(mode) && blank(values.ipv6_pool)) {
      errors.push(label(locale, "validation.ipv6_pool_required"));
    }
  }
  if (
    ["stop_networks", "restart_networks", "cleanup_node_residuals"].includes(
      action.id,
    )
  ) {
    if (
      !booleanValue(values.all, locale) &&
      splitNames(values.networks).length === 0
    ) {
      const key =
        action.id === "restart_networks"
          ? "validation.restart_networks_selection"
          : action.id === "stop_networks"
            ? "validation.stop_networks_selection"
            : "validation.cleanup_networks_selection";
      errors.push(label(locale, key));
    }
  }
  return errors;
}

export function buildCommand(
  action: NodeUiAction,
  values: ActionValues,
  locale: Locale,
): NodeCommand {
  const errors = validateActionValues(action, values, locale);
  if (errors.length)
    throw new Error(errors.join(label(locale, "common.list_separator")));
  const requireValue = (id: string) => required(values, id, locale);
  const requiredString = (id: string) => String(requireValue(id)).trim();
  switch (action.id) {
    case "refresh_snapshot":
      return { command: "get_snapshot" };
    case "get_node_diagnostics":
      return { command: "get_node_diagnostics" };
    case "get_blacklist":
      return {
        command: "get_blacklist",
        payload: { network_name: requiredString("network_name") },
      };
    case "get_network_allocation":
      return {
        command: "get_network_allocation",
        payload: { network_name: requiredString("network_name") },
      };
    case "ban_client":
      return {
        command: "ban_client",
        payload: {
          network_name: requiredString("network_name"),
          client_id: requiredString("client_id"),
        },
      };
    case "unban_client":
      return {
        command: "unban_client",
        payload: {
          network_name: requiredString("network_name"),
          client_id: requiredString("client_id"),
        },
      };
    case "update_network_allocation":
      return {
        command: "update_network_allocation",
        payload: {
          network_name: requiredString("network_name"),
          allocation: {
            mode: requiredString("mode") as
              "ipv4_only" | "ipv6_only" | "dual_stack",
            ipv4_pool: optional(values, "ipv4_pool") ?? "",
            ipv6_pool: optional(values, "ipv6_pool") ?? "",
          },
        },
      };
    case "create_network":
      return {
        command: "create_network",
        payload: {
          name: requiredString("name"),
          signaling_port: Number(requireValue("signaling_port")),
          relay_port: Number(requireValue("relay_port")),
          external_host: optional(values, "external_host"),
          auto_connect: booleanValue(values.auto_connect, locale),
        },
      };
    case "join_network":
      return {
        command: "join_network",
        payload: {
          target: requiredString("target"),
          network_name: optional(values, "network_name"),
          start: booleanValue(values.start, locale),
        },
      };
    case "start_networks":
      return {
        command: "start_networks",
        payload: { selection: networkSelection(requireValue("selection")) },
      };
    case "stop_networks": {
      const all = booleanValue(values.all, locale);
      return {
        command: "stop_networks",
        payload: { networks: all ? [] : splitNames(values.networks), all },
      };
    }
    case "restart_networks": {
      const all = booleanValue(values.all, locale);
      return {
        command: "restart_networks",
        payload: { networks: all ? [] : splitNames(values.networks), all },
      };
    }
    case "cleanup_node_residuals": {
      const all = booleanValue(requireValue("all"), locale);
      return {
        command: "cleanup_node_residuals",
        payload: { networks: all ? [] : splitNames(values.networks), all },
      };
    }
    case "probe_peer":
      return {
        command: "probe_peer",
        payload: {
          network_name: requiredString("network_name"),
          target: requiredString("target"),
          timeout_ms: Number(requireValue("timeout_ms")),
        },
      };
    case "update_routing":
      return {
        command: "update_routing",
        payload: {
          network_name: requiredString("network_name"),
          mode: requiredString("mode") as
            "mesh_only" | "bypass_mainland" | "global_proxy",
          exit_node: optional(values, "exit_node"),
          is_exit_node: booleanValue(values.is_exit_node, locale),
        },
      };
    case "set_exit_node_role":
    case "set_relay_role":
      return {
        command: "set_network_role",
        payload: {
          network_name: requiredString("network_name"),
          role: requiredString("role") as
            "member" | "discovery" | "relay" | "exit_node",
          enabled: booleanValue(requireValue("enabled"), locale),
        },
      };
    case "get_configuration":
      return { command: "get_configuration" };
    case "preview_configuration":
      return {
        command: "preview_configuration",
        payload: {
          expected_revision: revisionValue(
            requireValue("expected_revision"),
            label(locale, "field.expected_revision"),
            locale,
          ),
          networks: parseConfigurationNetworks(
            requiredString("networks"),
            locale,
          ),
        },
      };
    case "apply_configuration":
      return {
        command: "apply_configuration",
        payload: {
          expected_revision: revisionValue(
            requireValue("expected_revision"),
            label(locale, "field.expected_revision"),
            locale,
          ),
          networks: parseConfigurationNetworks(
            requiredString("networks"),
            locale,
          ),
        },
      };
    case "rollback_configuration":
      return {
        command: "rollback_configuration",
        payload: {
          expected_revision: revisionValue(
            requireValue("expected_revision"),
            label(locale, "field.expected_revision"),
            locale,
          ),
          target_revision: revisionValue(
            requireValue("target_revision"),
            label(locale, "field.target_revision"),
            locale,
          ),
        },
      };
    case "get_configuration_history":
      return { command: "get_configuration_history" };
    case "start_node":
      return { command: "start_node" };
    case "stop_node":
      return { command: "stop_node" };
    case "restart_node":
      return { command: "restart_node" };
    case "shutdown_node":
      return { command: "shutdown" };
    default:
      throw new Error(
        i18n().tr(locale, "error.unknown_action_command", {
          action: action.id,
        }),
      );
  }
}

export function commandEnvelope(
  requestId: number,
  action: NodeUiAction,
  values: ActionValues,
  locale: Locale,
  idempotencyKey: string,
): NodeCommandHttpRequest {
  if (!Number.isSafeInteger(requestId) || requestId <= 0)
    fail("request id must be a positive safe integer");
  return {
    schema_version: HTTP_SCHEMA_VERSION,
    idempotency_key: idempotencyKey,
    request: {
      request_id: requestId,
      command: buildCommand(action, values, locale),
    },
  };
}

export function validateCommandResponse(
  response: NodeApiCommandResponse,
  requestId: number,
): NodeCommandOutcome {
  if (!record(response) || response.schema_version !== HTTP_SCHEMA_VERSION)
    fail("command HTTP schema mismatch");
  if (!record(response.result) || response.result.request_id !== requestId)
    fail("command request id mismatch");
  const outcome = response.result.outcome as NodeCommandOutcome;
  if (!record(outcome) || !OUTCOME_STATUSES.has(outcome.status))
    fail("unknown command outcome");
  if (outcome.status === "snapshot") validateSnapshot(outcome.data);
  if (outcome.status === "configuration") configurationDocument(outcome.data);
  if (outcome.status === "configuration_preview")
    configurationPreview(outcome.data);
  if (outcome.status === "configuration_applied")
    configurationApplied(outcome.data);
  if (outcome.status === "configuration_history")
    configurationHistory(outcome.data);
  if (outcome.status === "rejected" && !record(outcome.data))
    fail("rejected outcome is missing problem");
  if (outcome.status === "accepted" && !record(outcome.data))
    fail("accepted outcome is missing operation");
  return outcome;
}

export function problemText(
  problem: NodeProblem | null | undefined,
  locale: Locale,
): string {
  if (!problem) return label(locale, "common.unknown");
  return label(locale, problem.message_key || `error.${problem.code}`);
}

export function outcomeSummary(
  outcome: NodeCommandOutcome,
  locale: Locale,
): string {
  if (outcome.status === "rejected") return problemText(outcome.data, locale);
  if (outcome.status === "accepted") {
    return i18n().tr(locale, "outcome.accepted", {
      operation: outcome.data.operation_id,
    });
  }
  if (outcome.status === "completed") return label(locale, "outcome.completed");
  if (outcome.status === "configuration") {
    return i18n().tr(locale, "outcome.configuration_loaded", {
      revision: i18n().formatInteger(locale, outcome.data.revision),
    });
  }
  if (outcome.status === "configuration_preview") {
    return i18n().tr(locale, "outcome.configuration_previewed", {
      changes: i18n().formatInteger(locale, outcome.data.diffs.length),
      errors: i18n().formatInteger(
        locale,
        outcome.data.validation_errors.length,
      ),
    });
  }
  if (outcome.status === "configuration_applied") {
    return i18n().tr(locale, "outcome.configuration_applied", {
      previous: i18n().formatInteger(locale, outcome.data.previous_revision),
      revision: i18n().formatInteger(locale, outcome.data.revision),
      status: label(
        locale,
        outcome.data.no_op ? "outcome.unchanged" : "outcome.changed",
      ),
    });
  }
  if (outcome.status === "configuration_history") {
    return i18n().tr(locale, "outcome.configuration_history", {
      count: i18n().formatInteger(locale, outcome.data.length),
    });
  }
  if (outcome.status === "network_created") {
    return i18n().tr(locale, "outcome.network_created", {
      network: outcome.data.network_name,
      persistence: label(
        locale,
        outcome.data.config_saved
          ? "outcome.persisted"
          : "outcome.not_persisted",
      ),
      startup: label(
        locale,
        outcome.data.started ? "outcome.started" : "outcome.not_started",
      ),
    });
  }
  if (outcome.status === "blacklist") {
    return i18n().tr(locale, "outcome.blacklist", {
      nodes: i18n().formatInteger(locale, outcome.data.clients.length),
      change: label(
        locale,
        outcome.data.changed ? "outcome.changed" : "outcome.unchanged",
      ),
    });
  }
  return label(locale, "outcome.completed");
}

export function allActions(manifest: NodeUiManifest): NodeUiAction[] {
  return manifest.pages.flatMap((page: NodeUiPage) => page.actions);
}
