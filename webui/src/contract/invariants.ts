import type {
  CapabilityKind,
  NodeApiCommandResponse,
  NodeApiManifestResponse,
  NodeApiOperationResponse,
  NodeCommand,
  NodeCommandHttpRequest,
  NodeCommandOutcome,
  ConfigurationApplyResult,
  ConfigurationDocument,
  ConfigurationHistoryEntry,
  ConfigurationPreview,
  NodeNetworkSelection,
  NetworkProfileApplyResult,
  NetworkProfilePreview,
  NodeOperationSnapshot,
  PolicyAffectedObject,
  PolicyApplyResult,
  PolicyCurrent,
  PolicyDiffEntry,
  PolicyHistoryEntry,
  PolicyHistorySnapshot,
  PolicyObjectKind,
  PolicyPreview,
  PolicyRiskLevel,
  PolicyRiskReason,
  PolicyRiskSummary,
  PolicyRollbackResult,
  PolicySnapshot,
  NodeProblem,
  NodeSnapshot,
  NodeUiAction,
  NodeUiActionBlockReason,
  NodeUiField,
  NodeJoinRequestReviewItem,
  NodeUiManifest,
  NodeUiPageState,
  NodeUiPage,
  NetworkMemberDirectory,
} from "../../../contracts/typescript/node-contract-v1";
import { ContractError } from "../api/client";
import {
  parsePolicySnapshotJson,
  parsePolicyTestInputJson,
  PolicyJsonError,
  validatePolicySnapshot,
  validatePolicyTestResult,
} from "./policy-test";

export const HTTP_SCHEMA_VERSION = 1;
export const MANIFEST_SCHEMA_VERSION = 1;

const PAGE_STATES = [
  "initial_loading",
  "error",
  "maintenance",
  "permission_denied",
  "unsupported",
  "offline",
  "control_unavailable",
  "node_failed",
  "degraded",
  "stale",
  "refreshing",
  "empty",
  "ready",
] as const satisfies readonly NodeUiPageState[];

const ACTION_BLOCK_REASONS = [
  "node_not_running",
  "no_online_network",
  "no_network_selected",
  "permission_denied",
  "platform_unsupported",
  "exit_node_unsupported",
  "tun_platform_unsupported",
  "tun_permission_denied",
  "tun_device_unavailable",
  "tun_tool_unavailable",
  "tun_unavailable",
  "backend_unavailable",
] as const satisfies readonly NodeUiActionBlockReason[];

export const REQUIRED_PAGES = [
  "overview",
  "networks",
  "peers",
  "node_runtime",
  "topology",
  "traffic",
  "activity",
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
  preview_network_profile: "preview_network_profile",
  update_network_profile: "update_network_profile",
  publish_network_discovery: "publish_network_discovery",
  update_network_discovery: "update_network_discovery",
  unpublish_network_discovery: "unpublish_network_discovery",
  refresh_discovery_index: "refresh_discovery_index",
  retry_discovery_index: "retry_discovery_index",
  preview_join_network: "preview_join_network",
  join_network: "join_network",
  submit_join_request: "submit_join_request",
  list_join_requests: "list_join_requests",
  list_network_members: "list_network_members",
  preview_member_removal: "preview_member_removal",
  change_member_lifecycle: "change_member_lifecycle",
  review_join_request: "review_join_request",
  batch_review_join_requests: "batch_review_join_requests",
  withdraw_join_request: "withdraw_join_request",
  resubmit_join_request: "resubmit_join_request",
  create_invitation: "create_invitation",
  list_invitations: "list_invitations",
  get_invitation_status: "get_invitation_status",
  revoke_invitation: "revoke_invitation",
  preview_invitation: "preview_invitation",
  redeem_invitation: "redeem_invitation",
  leave_network: "leave_network",
  acknowledge_membership_policy: "acknowledge_membership_policy",
  start_networks: "start_networks",
  stop_networks: "stop_networks",
  restart_networks: "restart_networks",
  rebuild_tun: "rebuild_tun",
  probe_peer: "probe_peer",
  get_blacklist: "get_blacklist",
  ban_client: "ban_client",
  unban_client: "unban_client",
  update_routing: "update_routing",
  set_exit_node_role: "set_network_role",
  set_relay_role: "set_network_role",
  set_node_display_name: "set_node_display_name",
  rotate_node_identity: "rotate_node_identity",
  get_network_allocation: "get_network_allocation",
  update_network_allocation: "update_network_allocation",
  get_configuration: "get_configuration",
  preview_configuration: "preview_configuration",
  apply_configuration: "apply_configuration",
  rollback_configuration: "rollback_configuration",
  get_configuration_history: "get_configuration_history",
  test_policy: "test_policy",
  get_policy: "get_policy",
  preview_policy: "preview_policy",
  preview_policy_rollback: "preview_policy_rollback",
  apply_policy: "apply_policy",
  rollback_policy: "rollback_policy",
  get_policy_history: "get_policy_history",
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
const OPERATION_STATES = new Set([
  "queued",
  "running",
  "waiting_for_reauth",
  "succeeded",
  "failed",
  "cancelled",
  "timed_out",
]);
const OPERATION_TERMINAL_STATES = new Set([
  "succeeded",
  "failed",
  "cancelled",
  "timed_out",
]);
const INPUT_KINDS = new Set([
  "boolean",
  "client_id",
  "external_host",
  "network_mode",
  "network_name",
  "network_selection",
  "peer_target",
  "purpose",
  "icon",
  "language",
  "region",
  "tags",
  "capability_declarations",
  "visibility",
  "join_mode",
  "exit_contribution",
  "exit_contribution_scope",
  "join_request_id",
  "join_request_note",
  "join_request_reason",
  "join_request_review_action",
  "join_request_reviews",
  "node_id",
  "member_lifecycle_action",
  "member_lifecycle_reason",
  "port",
  "role",
  "routing_mode",
  "url_or_ticket",
  "coordinator_url",
  "invitation_code",
  "timeout_ms",
  "revision",
  "configuration_json",
  "policy_json",
  "policy_snapshot_json",
  "policy_network_ids",
  "display_name",
  "fingerprint",
  "discovery_index_url",
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
  "policy_json",
  "policy_snapshot_json",
  "policy_network_ids",
  "display_name",
  "fingerprint",
  "network_purpose",
  "network_icon",
  "network_language",
  "region",
  "tags",
  "capability_declarations",
  "visibility",
  "join_mode",
  "exit_contribution",
  "exit_contribution_scope",
  "join_request_id",
  "join_request_note",
  "join_request_reason",
  "join_request_review_action",
  "join_request_reviews",
  "node_id",
  "member_lifecycle_action",
  "member_lifecycle_reason",
  "coordinator_url",
  "invitation_code",
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
  "network_profile_preview",
  "network_profile_updated",
  "network_join_preview",
  "network_joined",
  "join_request_submitted",
  "join_requests",
  "network_members",
  "member_removal_preview",
  "member_lifecycle_changed",
  "join_requests_reviewed",
  "join_request_updated",
  "invitation_created",
  "invitations",
  "invitation_status",
  "invitation_revoked",
  "invitation_preview",
  "invitation_redeemed",
  "network_left",
  "membership_policy_acknowledged",
  "configuration",
  "configuration_preview",
  "configuration_applied",
  "configuration_history",
  "policy",
  "policy_preview",
  "policy_applied",
  "policy_history",
  "policy_rolled_back",
  "policy_test",
  "identity_rotated",
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
const MEMBERSHIP_ROLES = new Set(["owner", "administrator", "member"]);
const MEMBER_LIFECYCLE_ACTIONS = new Set([
  "invite",
  "approve",
  "reject",
  "suspend",
  "resume",
  "exit",
  "remove",
  "undo_remove",
]);
const MEMBER_LIFECYCLE_STATUSES = new Set([
  "invited",
  "pending_approval",
  "approved",
  "active",
  "degraded",
  "suspended",
  "exited",
  "removed",
  "expired",
  "rejected",
]);
const MEMBER_REMOVAL_IMPACT_KEYS = new Set([
  "member_removal.impact.network_access",
  "member_removal.impact.file_access",
  "member_removal.impact.incomplete_transfers",
  "member_removal.impact.credentials",
  "member_removal.impact.local_files_kept",
  "member_removal.impact.safe_undo",
]);
const CAPABILITY_KINDS = new Set<CapabilityKind>([
  "coordinator",
  "relay",
  "exit",
  "file",
]);
const NETWORK_PROFILE_WARNING_CODES = new Set([
  "unsigned_legacy",
  "profile_expired",
  "revision_conflict",
  "capability_not_observed",
  "capability_undeclared",
]);
const NETWORK_VISIBILITIES = new Set(["public", "unlisted", "private"]);
const NETWORK_JOIN_MODES = new Set([
  "open",
  "approval_required",
  "invite_only",
]);
const EXIT_CONTRIBUTION_OBLIGATIONS = new Set(["required", "not_required"]);
const EXIT_CONTRIBUTION_SCOPES = new Set([
  "not_applicable",
  "joining_members",
  "all_members",
]);
const TUN_PLATFORMS = new Set(["linux", "macos", "windows", "other"]);
const TUN_CAPABILITY_REASONS = new Set([
  "platform_unsupported",
  "permission_denied",
  "device_unavailable",
  "tool_unavailable",
  "unknown",
]);

const POLICY_OBJECT_KINDS = new Set<PolicyObjectKind>([
  "local_policy",
  "network_acl",
  "permission_rule",
  "network_acl_grant",
  "contribution_veto",
  "network_scope",
]);
const POLICY_CHANGE_KINDS = new Set<string>([
  "local_policy_added",
  "local_policy_updated",
  "local_policy_removed",
  "network_acl_added",
  "network_acl_updated",
  "network_acl_removed",
  "permission_rule_added",
  "permission_rule_updated",
  "permission_rule_removed",
  "grant_added",
  "grant_updated",
  "grant_removed",
  "veto_added",
  "veto_updated",
  "veto_removed",
  "network_scope_changed",
  "validity_changed",
]);
const POLICY_RISK_LEVELS = new Set<PolicyRiskLevel>([
  "none",
  "elevated",
  "high",
]);
const POLICY_RISK_REASONS = new Set<PolicyRiskReason>([
  "allow_rule_added",
  "deny_rule_removed",
  "permission_rule_expanded",
  "network_acl_grant_added",
  "network_acl_grant_expanded",
  "grant_reauthentication_removed",
  "grant_validity_extended",
  "local_policy_removed",
  "network_acl_removed",
  "network_scope_expanded",
  "contribution_veto_removed",
  "policy_validity_extended",
]);
const POLICY_CHANGE_ACTIONS = new Set(["apply", "rollback"]);

export type ActionValues = Record<string, string | boolean>;
export type BlockReason = NodeUiActionBlockReason;

function tunBlockReason(
  capabilities: NodeSnapshot["tun_capabilities"] | undefined,
): BlockReason {
  if (!capabilities || !capabilities.supported)
    return "tun_platform_unsupported";
  switch (capabilities.reasons?.[0]) {
    case "platform_unsupported":
      return "tun_platform_unsupported";
    case "permission_denied":
      return "tun_permission_denied";
    case "device_unavailable":
      return "tun_device_unavailable";
    case "tool_unavailable":
      return "tun_tool_unavailable";
    case "unknown":
    default:
      return "tun_unavailable";
  }
}

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
  if (JSON.stringify(manifest.page_states) !== JSON.stringify(PAGE_STATES))
    fail("page state vocabulary mismatch");
  if (
    JSON.stringify(manifest.action_block_reasons) !==
    JSON.stringify(ACTION_BLOCK_REASONS)
  )
    fail("action block reason vocabulary mismatch");
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
  if (action.confirmation !== undefined && action.confirmation !== null) {
    if (!record(action.confirmation))
      fail(`confirmation invalid for ${action.id}`);
    stringValue(
      action.confirmation.title_key,
      `confirmation title missing for ${action.id}`,
      true,
    );
    stringValue(
      action.confirmation.consequence_key,
      `confirmation consequence missing for ${action.id}`,
      true,
    );
    if (typeof action.confirmation.destructive !== "boolean")
      fail(`confirmation destructive flag invalid for ${action.id}`);
    if (typeof action.confirmation.recent_authentication_required !== "boolean")
      fail(`confirmation reauthentication flag invalid for ${action.id}`);
  }
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

export function validateSnapshotResponse(response: unknown): NodeSnapshot {
  if (!record(response)) fail("snapshot response is missing");
  if (response.schema_version !== HTTP_SCHEMA_VERSION)
    fail("snapshot HTTP schema mismatch");
  return validateSnapshot(response.snapshot);
}

export function validateSnapshot(snapshot: unknown): NodeSnapshot {
  if (!record(snapshot)) fail("snapshot payload is missing");
  const value = snapshot as NodeSnapshot;
  nonNegativeInteger(value.revision, "snapshot revision invalid");
  if (!record(value.lifecycle)) fail("snapshot lifecycle missing");
  if (!LIFECYCLE_PHASES.has(value.lifecycle.phase))
    fail("lifecycle phase invalid");
  nonNegativeInteger(
    value.lifecycle.configured_networks,
    "configured_networks invalid",
  );
  nonNegativeInteger(
    value.lifecycle.active_networks,
    "active_networks invalid",
  );
  arrayValue(value.lifecycle.failed_networks, "failed_networks invalid");
  arrayValue(value.networks, "networks invalid");
  arrayValue(value.peers, "peers invalid");
  arrayValue(value.logs, "logs invalid");
  arrayValue(value.embedded_services, "embedded_services invalid");
  if (
    !record(value.configuration) ||
    !Array.isArray(value.configuration.networks)
  ) {
    fail("configuration networks invalid");
  }
  if (
    !record(value.permissions) ||
    !Array.isArray(value.permissions.permissions)
  ) {
    fail("permissions invalid");
  }
  if (value.tun_capabilities !== undefined) {
    if (
      !record(value.tun_capabilities) ||
      !Number.isSafeInteger(value.tun_capabilities.schema_version) ||
      value.tun_capabilities.schema_version < 0 ||
      !TUN_PLATFORMS.has(value.tun_capabilities.platform) ||
      typeof value.tun_capabilities.supported !== "boolean" ||
      typeof value.tun_capabilities.can_rebuild !== "boolean" ||
      !Array.isArray(value.tun_capabilities.reasons)
    ) {
      fail("TUN capabilities invalid");
    }
    for (const reason of value.tun_capabilities.reasons) {
      if (!TUN_CAPABILITY_REASONS.has(reason))
        fail(`TUN capability reason invalid: ${reason}`);
    }
  }
  for (const network of value.networks) {
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
  if (value.network_profiles !== undefined) {
    for (const profile of value.network_profiles) {
      if (!record(profile)) fail("network profile snapshot invalid");
      stringValue(profile.name, "network profile snapshot name invalid", true);
      stringValue(
        profile.network_id,
        "network profile snapshot id invalid",
        true,
      );
      stringValue(
        profile.purpose,
        "network profile snapshot purpose invalid",
        true,
      );
      stringValue(profile.icon, "network profile snapshot icon invalid", true);
      stringValue(
        profile.language,
        "network profile snapshot language invalid",
        true,
      );
      if (!/^[A-Z]{2}$/.test(String(profile.region)))
        fail("network profile snapshot region invalid");
      stringArray(profile.tags, "network profile snapshot tags invalid");
      const capabilities = stringArray(
        profile.capability_declarations,
        "network profile snapshot capabilities invalid",
      );
      for (const capability of capabilities) {
        if (!CAPABILITY_KINDS.has(capability as CapabilityKind))
          fail("network profile snapshot capability invalid");
      }
      if (!NETWORK_VISIBILITIES.has(String(profile.visibility)))
        fail("network profile snapshot visibility invalid");
      if (!NETWORK_JOIN_MODES.has(String(profile.join_mode)))
        fail("network profile snapshot join mode invalid");
      nonNegativeInteger(
        profile.revision,
        "network profile snapshot revision invalid",
      );
      nonNegativeInteger(
        profile.policy_revision,
        "network profile snapshot policy revision invalid",
      );
      if (
        profile.signed_profile !== undefined &&
        profile.signed_profile !== null
      ) {
        if (!record(profile.signed_profile))
          fail("signed network profile snapshot invalid");
        nonNegativeInteger(
          profile.signed_profile.schema_version,
          "signed network profile snapshot schema invalid",
        );
        nonNegativeInteger(
          profile.signed_profile.issued_at_unix_ms,
          "signed network profile snapshot issue time invalid",
        );
        nonNegativeInteger(
          profile.signed_profile.expires_at_unix_ms,
          "signed network profile snapshot expiry invalid",
        );
      }
    }
  }
  for (const peer of value.peers) {
    if (!record(peer)) fail("peer snapshot invalid");
    stringValue(peer.peer_id, "peer id invalid", true);
    stringValue(peer.network_name, "peer network invalid", true);
    stringValue(peer.virtual_ipv6, "peer virtual IPv6 invalid", true);
    arrayValue(peer.endpoints, `peer endpoints invalid: ${peer.peer_id}`);
    if (!record(peer.traffic)) fail(`peer traffic invalid: ${peer.peer_id}`);
  }
  for (const entry of value.logs) {
    if (!record(entry)) fail("log entry invalid");
    stringValue(entry.message, "log message invalid");
    stringValue(entry.target, "log target invalid");
    nonNegativeInteger(entry.unix_ms, "log timestamp invalid");
  }
  return value;
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
  if (action.id === "set_exit_node_role") {
    const exitCapability = snapshot.capabilities?.capabilities.find(
      (capability) => capability.kind === "exit",
    );
    if (
      exitCapability?.state === "unsupported" ||
      (exitCapability?.state === "unknown" &&
        snapshot.exit_node_capabilities?.supported === false)
    ) {
      return "exit_node_unsupported";
    }
  }
  if (action.id === "rebuild_tun" && !snapshot.tun_capabilities?.can_rebuild) {
    return tunBlockReason(snapshot.tun_capabilities);
  }
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
  const profileAction =
    action.id === "preview_network_profile" ||
    action.id === "update_network_profile";
  const profile = profileAction
    ? (snapshot.network_profiles ?? []).find((entry) => entry.name === network)
    : undefined;
  if (profile) {
    if (field.id === "network_name") return profile.name;
    if (field.id === "expected_revision") return String(profile.revision);
    if (field.id === "purpose") return profile.purpose;
    if (field.id === "icon") return profile.icon;
    if (field.id === "language") return profile.language;
    if (field.id === "region") return profile.region;
    if (field.id === "tags") return profile.tags.join(",");
    if (field.id === "capability_declarations")
      return profile.capability_declarations.join(",");
    if (field.id === "visibility") return profile.visibility;
    if (field.id === "join_mode") return profile.join_mode;
    if (field.id === "exit_contribution") return profile.exit_contribution;
    if (field.id === "exit_contribution_scope")
      return profile.exit_contribution_scope;
  }
  if (field.id === "purpose") return "Owner-created network";
  if (field.id === "icon") return "network";
  if (field.id === "language") return "und";
  if (field.id === "region") return "ZZ";
  if (field.id === "tags") return "";
  if (field.id === "capability_declarations") return "";
  if (field.id === "index_url")
    return snapshot.discovery_index?.index_url ?? "";
  if (field.id === "visibility") return "unlisted";
  if (field.id === "join_mode") return "invite_only";
  if (field.id === "exit_contribution") return "not_required";
  if (field.id === "exit_contribution_scope") return "not_applicable";
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
  if (field.id === "role") {
    if (action.id === "create_invitation") return "member";
    return action.id === "set_relay_role" ? "relay" : "exit_node";
  }
  if (field.id === "display_name") return snapshot.identity?.display_name ?? "";
  if (field.id === "expected_fingerprint")
    return snapshot.identity?.public_key_fingerprint ?? "";
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

function validJoinRequestId(value: string): boolean {
  if (!/^join_[A-Za-z0-9_-]{43}$/.test(value)) return false;
  try {
    const decoded = atob(
      value.slice("join_".length).replace(/-/g, "+").replace(/_/g, "/") + "=",
    );
    return decoded.length === 32;
  } catch {
    return false;
  }
}

function validJoinRequestText(value: string, maxLength: number): boolean {
  return (
    value.trim() !== "" &&
    value.trim() === value &&
    !/\p{Cc}/u.test(value) &&
    [...value].length <= maxLength
  );
}

function parseJoinRequestReviews(
  value: string | boolean | undefined,
  locale: Locale,
): NodeJoinRequestReviewItem[] {
  let parsed: unknown;
  try {
    parsed = JSON.parse(String(value ?? ""));
  } catch {
    throw new Error(label(locale, "join_request.error.invalid_request"));
  }
  if (!Array.isArray(parsed)) {
    throw new Error(label(locale, "join_request.error.invalid_request"));
  }
  if (parsed.length === 0) {
    throw new Error(label(locale, "join_request.error.batch_empty"));
  }
  if (parsed.length > 100) {
    throw new Error(label(locale, "join_request.error.batch_too_large"));
  }

  const seen = new Set<string>();
  const reviews: NodeJoinRequestReviewItem[] = [];
  for (const item of parsed) {
    if (!record(item)) {
      throw new Error(label(locale, "join_request.error.invalid_request"));
    }
    const requestId =
      typeof item.request_id === "string" ? item.request_id : "";
    if (!validJoinRequestId(requestId)) {
      throw new Error(label(locale, "join_request.error.invalid_request_id"));
    }
    if (seen.has(requestId)) {
      throw new Error(label(locale, "join_request.error.batch_duplicate"));
    }
    seen.add(requestId);
    if (
      !Number.isSafeInteger(item.expected_revision) ||
      Number(item.expected_revision) <= 0
    ) {
      throw new Error(label(locale, "join_request.error.invalid_revision"));
    }
    if (
      item.action !== "approve" &&
      item.action !== "reject" &&
      item.action !== "request_information"
    ) {
      throw new Error(label(locale, "join_request.error.invalid_request"));
    }
    const note = item.note;
    if (note !== undefined && note !== null && typeof note !== "string") {
      throw new Error(label(locale, "join_request.error.invalid_action_note"));
    }
    if (item.action === "approve" && note !== undefined && note !== null) {
      throw new Error(label(locale, "join_request.error.invalid_action_note"));
    }
    if (
      item.action === "request_information" &&
      (note === undefined || note === null)
    ) {
      throw new Error(label(locale, "join_request.error.information_required"));
    }
    if (typeof note === "string" && !validJoinRequestText(note, 500)) {
      throw new Error(label(locale, "join_request.error.invalid_action_note"));
    }
    reviews.push({
      request_id: requestId,
      expected_revision: Number(item.expected_revision),
      action: item.action,
      ...(typeof note === "string" ? { note } : {}),
    });
  }
  return reviews;
}

function splitNames(value: string | boolean | undefined): string[] {
  return String(value ?? "")
    .split(",")
    .map((item) => item.trim())
    .filter(Boolean);
}

function policyNetworkIds(
  value: string | boolean | undefined,
  locale: Locale,
): string[] {
  const ids = String(value ?? "")
    .split(/[,\s]+/)
    .map((item) => item.trim())
    .filter(Boolean);
  const seen = new Set<string>();
  for (const id of ids) {
    if (!/^net_[A-Za-z0-9_-]{43}$/.test(id) || seen.has(id)) {
      throw new Error(label(locale, "validation.policy_network_ids_invalid"));
    }
    seen.add(id);
  }
  if (ids.length === 0)
    throw new Error(label(locale, "validation.policy_network_ids_required"));
  return ids;
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

function policySnapshot(value: unknown): PolicySnapshot {
  try {
    return validatePolicySnapshot(value);
  } catch (error) {
    fail(error instanceof Error ? error.message : "policy snapshot invalid");
  }
}

function policyAffectedObjects(value: unknown): PolicyAffectedObject[] {
  for (const entry of arrayValue(value, "policy affected objects invalid")) {
    if (
      !record(entry) ||
      !POLICY_OBJECT_KINDS.has(entry.kind as PolicyObjectKind)
    ) {
      fail("policy affected object invalid");
    }
    if (entry.network_id !== undefined && entry.network_id !== null)
      stringValue(entry.network_id, "policy affected network id invalid", true);
    if (entry.object_id !== undefined && entry.object_id !== null)
      stringValue(entry.object_id, "policy affected object id invalid", true);
  }
  return value as PolicyAffectedObject[];
}

function policyDiffs(value: unknown): PolicyDiffEntry[] {
  for (const entry of arrayValue(value, "policy diff entries invalid")) {
    if (!record(entry)) fail("policy diff entry invalid");
    stringValue(entry.path, "policy diff path invalid", true);
    if (!POLICY_CHANGE_KINDS.has(String(entry.change)))
      fail("policy diff change invalid");
  }
  return value as PolicyDiffEntry[];
}

function policyRisk(value: unknown): PolicyRiskSummary {
  if (!record(value)) fail("policy risk summary invalid");
  if (!POLICY_RISK_LEVELS.has(value.level as PolicyRiskLevel))
    fail("policy risk level invalid");
  for (const reason of arrayValue(value.reasons, "policy risk reasons invalid"))
    if (!POLICY_RISK_REASONS.has(reason as PolicyRiskReason))
      fail("policy risk reason invalid");
  booleanValueRaw(
    value.privilege_expansion,
    "policy privilege expansion flag invalid",
  );
  booleanValueRaw(
    value.requires_explicit_confirmation,
    "policy explicit confirmation flag invalid",
  );
  booleanValueRaw(
    value.requires_recent_reauthentication,
    "policy recent authentication flag invalid",
  );
  return value as PolicyRiskSummary;
}

function policyCurrent(value: unknown): PolicyCurrent {
  if (!record(value)) fail("current policy invalid");
  nonNegativeInteger(value.revision, "current policy revision invalid");
  policySnapshot(value.snapshot);
  return value as PolicyCurrent;
}

function policyPreview(value: unknown): PolicyPreview {
  if (!record(value)) fail("policy preview invalid");
  nonNegativeInteger(
    value.expected_revision,
    "policy expected revision invalid",
  );
  nonNegativeInteger(value.current_revision, "policy current revision invalid");
  policySnapshot(value.current);
  policySnapshot(value.candidate);
  policyDiffs(value.diffs);
  policyAffectedObjects(value.affected_objects);
  policyRisk(value.risk);
  for (const networkId of stringArray(
    value.network_ids,
    "policy network ids invalid",
  )) {
    if (!/^net_[A-Za-z0-9_-]{43}$/.test(networkId))
      fail("policy network id invalid");
  }
  for (const field of [
    "can_apply",
    "has_changes",
    "includes_local_policy",
    "revision_conflict",
  ] as const)
    booleanValueRaw(value[field], `policy ${field} flag invalid`);
  if (
    value.rollback_target_revision !== undefined &&
    value.rollback_target_revision !== null
  )
    nonNegativeInteger(
      value.rollback_target_revision,
      "policy rollback target invalid",
    );
  return value as PolicyPreview;
}

function policyHistoryEntry(value: unknown): PolicyHistoryEntry {
  if (!record(value)) fail("policy history entry invalid");
  if (!POLICY_CHANGE_ACTIONS.has(String(value.action)))
    fail("policy history action invalid");
  stringValue(value.actor, "policy history actor invalid", true);
  stringValue(value.source, "policy history source invalid", true);
  stringValue(value.transaction_id, "policy transaction id invalid", true);
  stringValue(value.occurred_at, "policy history timestamp invalid", true);
  nonNegativeInteger(
    value.previous_revision,
    "policy previous revision invalid",
  );
  nonNegativeInteger(value.revision, "policy history revision invalid");
  policyDiffs(value.diffs);
  policyAffectedObjects(value.affected_objects);
  policyRisk(value.risk);
  if (!record(value.network_acl_revisions))
    fail("policy ACL revisions invalid");
  for (const revision of Object.values(value.network_acl_revisions))
    nonNegativeInteger(revision, "policy ACL revision invalid");
  for (const field of ["local_policy_revision", "target_revision"] as const) {
    if (value[field] !== undefined && value[field] !== null)
      nonNegativeInteger(value[field], `policy ${field} invalid`);
  }
  return value as PolicyHistoryEntry;
}

function policyHistory(value: unknown): PolicyHistorySnapshot {
  if (!record(value)) fail("policy history invalid");
  nonNegativeInteger(value.current_revision, "policy history revision invalid");
  for (const entry of arrayValue(
    value.entries,
    "policy history entries invalid",
  ))
    policyHistoryEntry(entry);
  return value as PolicyHistorySnapshot;
}

function policyApplied(value: unknown): PolicyApplyResult {
  if (!record(value)) fail("policy apply result invalid");
  nonNegativeInteger(
    value.previous_revision,
    "policy previous revision invalid",
  );
  nonNegativeInteger(value.revision, "policy applied revision invalid");
  policySnapshot(value.snapshot);
  policyDiffs(value.diffs);
  policyAffectedObjects(value.affected_objects);
  policyRisk(value.risk);
  const noOp = booleanValueRaw(value.no_op, "policy no-op flag invalid");
  if (noOp && value.revision !== value.previous_revision)
    fail("policy no-op moved revision");
  if (noOp && value.audit !== undefined && value.audit !== null)
    fail("policy no-op unexpectedly has audit");
  if (!noOp && (!record(value.audit) || value.audit === null))
    fail("policy apply is missing audit");
  if (record(value.audit)) policyHistoryEntry(value.audit);
  return value as PolicyApplyResult;
}

function policyRolledBack(value: unknown): PolicyRollbackResult {
  if (!record(value)) fail("policy rollback result invalid");
  nonNegativeInteger(
    value.previous_revision,
    "policy previous revision invalid",
  );
  nonNegativeInteger(value.revision, "policy rolled back revision invalid");
  nonNegativeInteger(value.target_revision, "policy rollback target invalid");
  policySnapshot(value.snapshot);
  policyDiffs(value.diffs);
  policyAffectedObjects(value.affected_objects);
  policyRisk(value.risk);
  if (!record(value.audit)) fail("policy rollback is missing audit");
  const audit = policyHistoryEntry(value.audit);
  if (audit.action !== "rollback")
    fail("policy rollback audit action mismatch");
  return value as PolicyRollbackResult;
}

function stringArray(value: unknown, message: string): string[] {
  const items = arrayValue(value, message);
  for (const item of items) stringValue(item, message);
  return items as string[];
}

function networkProfileDocument(value: unknown): void {
  if (!record(value)) fail("network profile document invalid");
  stringValue(value.network_id, "network profile id invalid", true);
  stringValue(value.public_key, "network profile public key invalid", true);
  stringValue(value.owner, "network profile owner invalid", true);
  stringValue(value.name, "network profile name invalid", true);
  stringValue(value.purpose, "network profile purpose invalid", true);
  stringValue(value.icon, "network profile icon invalid", true);
  stringValue(value.language, "network profile language invalid", true);
  if (!/^[A-Z]{2}$/.test(stringValue(value.region, "network region invalid")))
    fail("network profile region invalid");
  stringArray(value.tags, "network profile tags invalid");
  const capabilities = stringArray(
    value.capability_declarations,
    "network profile capabilities invalid",
  );
  for (const capability of capabilities) {
    if (!CAPABILITY_KINDS.has(capability as CapabilityKind))
      fail("network profile capability invalid");
  }
  if (!NETWORK_VISIBILITIES.has(String(value.visibility)))
    fail("network profile visibility invalid");
  if (!NETWORK_JOIN_MODES.has(String(value.join_mode)))
    fail("network profile join mode invalid");
  nonNegativeInteger(value.revision, "network profile revision invalid");
  nonNegativeInteger(value.schema_version, "network profile schema invalid");
  nonNegativeInteger(
    value.issued_at_unix_ms,
    "network profile issue time invalid",
  );
  nonNegativeInteger(
    value.expires_at_unix_ms,
    "network profile expiry invalid",
  );
}

function networkProfileRecord(value: unknown): void {
  if (!record(value)) fail("signed network profile invalid");
  networkProfileDocument(value.document);
  stringValue(value.signature, "network profile signature invalid", true);
}

function networkProfileWarnings(value: unknown): void {
  const warnings = arrayValue(value, "network profile warnings invalid");
  for (const warning of warnings) {
    if (
      !record(warning) ||
      !NETWORK_PROFILE_WARNING_CODES.has(String(warning.code))
    ) {
      fail("network profile warning invalid");
    }
    stringValue(warning.message, "network profile warning message invalid");
    if (warning.capability !== undefined && warning.capability !== null) {
      if (!CAPABILITY_KINDS.has(warning.capability as CapabilityKind))
        fail("network profile warning capability invalid");
    }
  }
}

function networkProfileProjections(value: unknown): void {
  if (!record(value)) fail("network profile projections invalid");
  if (!record(value.public_card)) fail("network public card invalid");
  networkProfileDocument(value.public_card.candidate);
  stringArray(
    value.public_card.withheld_categories,
    "network profile withheld categories invalid",
  );
  const published = value.public_card.currently_published;
  if (published !== undefined && published !== null)
    networkProfileRecord(published);

  if (!record(value.member_view)) fail("network member projection invalid");
  networkProfileDocument(value.member_view.profile);
  nonNegativeInteger(
    value.member_view.member_count,
    "network member count invalid",
  );
  nonNegativeInteger(
    value.member_view.administrator_count,
    "network administrator count invalid",
  );

  if (!record(value.administrator_view))
    fail("network administrator projection invalid");
  networkProfileDocument(value.administrator_view.profile);
  nonNegativeInteger(
    value.administrator_view.member_count,
    "network admin member count invalid",
  );
  nonNegativeInteger(
    value.administrator_view.administrator_count,
    "network admin administrator count invalid",
  );
  nonNegativeInteger(
    value.administrator_view.revision,
    "network admin profile revision invalid",
  );
  nonNegativeInteger(
    value.administrator_view.policy_revision,
    "network admin policy revision invalid",
  );

  if (!record(value.local_secret))
    fail("network local secret projection invalid");
  booleanValueRaw(
    value.local_secret.identity_present,
    "network local identity flag invalid",
  );
  booleanValueRaw(
    value.local_secret.secret_material_included,
    "network local secret flag invalid",
  );
  stringValue(
    value.local_secret.public_key_fingerprint,
    "network local fingerprint invalid",
    true,
  );
  stringValue(
    value.local_secret.storage,
    "network local storage invalid",
    true,
  );
}

function networkProfileDiff(value: unknown): void {
  for (const entry of arrayValue(value, "network profile diff invalid")) {
    if (!record(entry)) fail("network profile diff entry invalid");
    stringValue(entry.path, "network profile diff path invalid", true);
    for (const side of [entry.before, entry.after]) {
      if (side !== undefined && side !== null && typeof side !== "string")
        fail("network profile diff value invalid");
    }
  }
}

function networkProfilePreview(value: unknown): NetworkProfilePreview {
  if (!record(value)) fail("network profile preview invalid");
  stringValue(
    value.network_name,
    "network profile preview network invalid",
    true,
  );
  booleanValueRaw(value.can_apply, "network profile can_apply invalid");
  booleanValueRaw(value.conflict, "network profile conflict invalid");
  nonNegativeInteger(
    value.current_revision,
    "network profile current revision invalid",
  );
  nonNegativeInteger(
    value.expected_revision,
    "network profile expected revision invalid",
  );
  nonNegativeInteger(
    value.candidate_revision,
    "network profile candidate revision invalid",
  );
  networkProfileDiff(value.diff);
  networkProfileWarnings(value.warnings);
  networkProfileProjections(value.projections);
  return value as NetworkProfilePreview;
}

function networkProfileApplyResult(value: unknown): NetworkProfileApplyResult {
  if (!record(value)) fail("network profile apply result invalid");
  stringValue(
    value.network_name,
    "network profile apply network invalid",
    true,
  );
  stringValue(value.audit_action, "network profile audit action invalid", true);
  stringArray(value.changed_paths, "network profile changed paths invalid");
  nonNegativeInteger(
    value.applied_at_unix_ms,
    "network profile applied time invalid",
  );
  nonNegativeInteger(
    value.previous_revision,
    "network profile previous revision invalid",
  );
  nonNegativeInteger(value.revision, "network profile revision invalid");
  networkProfileRecord(value.signed_profile);
  networkProfileWarnings(value.warnings);
  networkProfileProjections(value.projections);
  return value as NetworkProfileApplyResult;
}

export function validateActionValues(
  action: NodeUiAction,
  values: ActionValues,
  locale: Locale,
): string[] {
  const errors: string[] = [];
  const previewJoin = action.id === "join_network" && !blank(values.network_id);
  for (const field of action.fields) {
    const value = values[field.id];
    if (
      field.required &&
      !(previewJoin && field.id === "target") &&
      blank(value)
    ) {
      errors.push(
        i18n().tr(locale, "validation.required", {
          field: label(locale, field.label_key),
        }),
      );
      continue;
    }
    if (blank(value)) continue;
    if (field.validation === "network_purpose") {
      const purpose = String(value);
      if (
        !purpose.trim() ||
        [...purpose].length > 512 ||
        purpose.trim() !== purpose ||
        /\p{Cc}/u.test(purpose)
      ) {
        errors.push(label(locale, "validation.network_purpose_invalid"));
      }
    } else if (field.validation === "network_icon") {
      const icon = String(value);
      if (!/^[a-z0-9._-]{1,64}$/.test(icon) || icon.trim() !== icon) {
        errors.push(label(locale, "validation.network_icon_invalid"));
      }
    } else if (field.validation === "network_language") {
      const language = String(value);
      if (
        language.length > 35 ||
        !/^[A-Za-z]{2,8}(?:-[A-Za-z0-9]{1,8})*$/.test(language)
      ) {
        errors.push(label(locale, "validation.network_language_invalid"));
      }
    } else if (field.validation === "region") {
      if (!/^[A-Z]{2}$/.test(String(value))) {
        errors.push(label(locale, "validation.network_region_invalid"));
      }
    } else if (field.validation === "tags") {
      const tags = splitNames(value);
      if (tags.length > 12) {
        errors.push(label(locale, "validation.network_tags_too_many"));
      }
      const seen = new Set<string>();
      for (const tag of tags) {
        if ([...tag].length > 32 || !/^[\p{L}\p{N}._-]+$/u.test(tag)) {
          errors.push(
            i18n().tr(locale, "validation.network_tag_invalid", { tag }),
          );
        }
        const key = tag.replace(/[A-Z]/g, (character) =>
          character.toLowerCase(),
        );
        if (seen.has(key)) {
          errors.push(
            i18n().tr(locale, "validation.network_tag_duplicate", { tag }),
          );
        }
        seen.add(key);
      }
    } else if (field.validation === "capability_declarations") {
      const capabilities = splitNames(value);
      const seen = new Set<string>();
      for (const capability of capabilities) {
        if (!CAPABILITY_KINDS.has(capability as CapabilityKind)) {
          errors.push(
            i18n().tr(locale, "validation.network_capability_invalid", {
              capability,
            }),
          );
        }
        if (seen.has(capability)) {
          errors.push(
            i18n().tr(locale, "validation.network_capability_duplicate", {
              capability,
            }),
          );
        }
        seen.add(capability);
      }
    } else if (
      field.validation === "visibility" &&
      !NETWORK_VISIBILITIES.has(String(value))
    ) {
      errors.push(label(locale, "validation.network_visibility_invalid"));
    } else if (
      field.validation === "join_mode" &&
      !NETWORK_JOIN_MODES.has(String(value))
    ) {
      errors.push(label(locale, "validation.network_join_mode_invalid"));
    } else if (
      field.validation === "exit_contribution" &&
      !EXIT_CONTRIBUTION_OBLIGATIONS.has(String(value))
    ) {
      errors.push(label(locale, "validation.exit_contribution_invalid"));
    } else if (
      field.validation === "exit_contribution_scope" &&
      !EXIT_CONTRIBUTION_SCOPES.has(String(value))
    ) {
      errors.push(label(locale, "validation.exit_contribution_scope_invalid"));
    } else if (
      field.validation === "join_request_id" &&
      !validJoinRequestId(String(value))
    ) {
      errors.push(label(locale, "join_request.error.invalid_request_id"));
    } else if (
      field.validation === "join_request_reason" &&
      !validJoinRequestText(String(value), 1000)
    ) {
      errors.push(label(locale, "join_request.error.invalid_reason"));
    } else if (
      field.validation === "join_request_note" &&
      !validJoinRequestText(String(value), 500)
    ) {
      errors.push(label(locale, "join_request.error.invalid_action_note"));
    } else if (
      field.validation === "join_request_review_action" &&
      !["approve", "reject", "request_information"].includes(String(value))
    ) {
      errors.push(label(locale, "join_request.error.invalid_request"));
    } else if (field.validation === "join_request_reviews") {
      try {
        parseJoinRequestReviews(value, locale);
      } catch (error) {
        errors.push(error instanceof Error ? error.message : String(error));
      }
    } else if (field.validation === "node_id") {
      if (!/^node_[A-Za-z0-9_-]{43}$/.test(String(value).trim())) {
        errors.push(label(locale, "validation.node_id"));
      }
    } else if (field.validation === "member_lifecycle_action") {
      if (!MEMBER_LIFECYCLE_ACTIONS.has(String(value))) {
        errors.push(label(locale, "validation.member_lifecycle_action"));
      }
    } else if (field.validation === "member_lifecycle_reason") {
      const reason = String(value).trim();
      if (
        reason.length === 0 ||
        [...reason].length > 512 ||
        /\p{Cc}/u.test(reason)
      ) {
        errors.push(label(locale, "validation.member_lifecycle_reason"));
      }
    } else if (field.validation === "network_name") {
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
    } else if (field.validation === "role") {
      const allowedRoles =
        field.label_key === "field.invitation_role"
          ? new Set(["member", "administrator"])
          : NETWORK_ROLES;
      if (!allowedRoles.has(String(value)))
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
    } else if (field.validation === "policy_json") {
      try {
        parsePolicyTestInputJson(String(value));
      } catch (error) {
        errors.push(policyJsonErrorText(error, locale));
      }
    } else if (field.validation === "policy_snapshot_json") {
      try {
        parsePolicySnapshotJson(String(value));
      } catch (error) {
        errors.push(policyJsonErrorText(error, locale));
      }
    } else if (field.validation === "policy_network_ids") {
      try {
        policyNetworkIds(value, locale);
      } catch (error) {
        errors.push(error instanceof Error ? error.message : String(error));
      }
    } else if (field.validation === "display_name") {
      const displayName = String(value).trim();
      if (/\p{Cc}/u.test(displayName)) {
        errors.push(label(locale, "validation.display_name_control_character"));
      }
      if ([...displayName].length > 80) {
        errors.push(label(locale, "validation.display_name_too_long"));
      }
    } else if (field.validation === "fingerprint") {
      const fingerprint = String(value);
      const prefix =
        field.id === "invitation_id"
          ? "inv_"
          : ["preview_digest", "terms_hash", "expected_terms_hash"].includes(
                field.id,
              )
            ? "hash_"
            : "";
      const encoded = prefix ? fingerprint.slice(prefix.length) : fingerprint;
      if (
        (prefix && !fingerprint.startsWith(prefix)) ||
        !/^[A-Za-z0-9_-]{43}$/.test(encoded)
      ) {
        errors.push(label(locale, "validation.fingerprint_invalid"));
      } else {
        try {
          const decoded = atob(
            encoded.replace(/-/g, "+").replace(/_/g, "/") + "=",
          );
          if (decoded.length !== 32) {
            errors.push(label(locale, "validation.fingerprint_invalid"));
          }
        } catch {
          errors.push(label(locale, "validation.fingerprint_invalid"));
        }
      }
    } else if (field.validation === "invitation_code") {
      const invitationCode = String(value).trim();
      if (
        !/^fkinv_[A-Za-z0-9_-]{43}$/.test(invitationCode) ||
        invitationCode.includes("=")
      ) {
        errors.push(label(locale, "validation.invitation_code_invalid"));
      }
    }
  }
  if (previewJoin) {
    const digest = String(values.preview_digest ?? "").trim();
    const termsHash = String(values.terms_hash ?? "").trim();
    const policyRevision = Number(values.network_policy_revision);
    const privacyRevision = Number(values.privacy_policy_revision);
    const acknowledgedAt = Number(values.acknowledged_at_unix_ms);
    if (!digest || !termsHash) {
      errors.push(label(locale, "validation.join_confirmation_incomplete"));
    }
    if (
      !Number.isSafeInteger(policyRevision) ||
      policyRevision <= 0 ||
      !Number.isSafeInteger(privacyRevision) ||
      privacyRevision <= 0
    ) {
      errors.push(label(locale, "validation.join_policy_revision_invalid"));
    }
    if (!Number.isSafeInteger(acknowledgedAt) || acknowledgedAt <= 0) {
      errors.push(label(locale, "validation.join_acknowledgement_invalid"));
    }
  }
  if (
    action.id === "create_network" &&
    Number(values.signaling_port) === Number(values.relay_port)
  ) {
    errors.push(label(locale, "validation.port_distinct"));
  }
  if (action.id === "review_join_request") {
    const reviewAction = String(values.action ?? "");
    const noteError = label(locale, "join_request.error.invalid_action_note");
    if (reviewAction === "approve" && !blank(values.note)) {
      if (!errors.includes(noteError)) errors.push(noteError);
    }
    if (reviewAction === "request_information" && blank(values.note)) {
      errors.push(label(locale, "join_request.error.information_required"));
    }
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

function policyJsonErrorText(error: unknown, locale: Locale): string {
  if (error instanceof PolicyJsonError) {
    return i18n().tr(locale, "validation.policy_json_path", {
      path: error.path,
      message: error.message,
    });
  }
  return error instanceof Error ? error.message : String(error);
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
          purpose: requiredString("purpose"),
          icon: requiredString("icon"),
          language: requiredString("language"),
          region: requiredString("region"),
          tags: splitNames(values.tags),
          capability_declarations: splitNames(
            values.capability_declarations,
          ) as CapabilityKind[],
          visibility: requiredString("visibility") as
            "public" | "unlisted" | "private",
          join_mode: requiredString("join_mode") as
            "open" | "approval_required" | "invite_only",
          exit_contribution: requiredString("exit_contribution") as
            "required" | "not_required",
          exit_contribution_scope: requiredString("exit_contribution_scope") as
            "not_applicable" | "joining_members" | "all_members",
          signaling_port: Number(requireValue("signaling_port")),
          relay_port: Number(requireValue("relay_port")),
          external_host: optional(values, "external_host"),
          auto_connect: booleanValue(values.auto_connect, locale),
        },
      };
    case "preview_network_profile":
    case "update_network_profile": {
      const command = {
        network_name: requiredString("network_name"),
        expected_revision: revisionValue(
          requireValue("expected_revision"),
          label(locale, "field.expected_revision"),
          locale,
        ),
        update: {
          purpose: requiredString("purpose"),
          icon: requiredString("icon"),
          language: requiredString("language"),
          region: requiredString("region"),
          tags: splitNames(values.tags),
          capability_declarations: splitNames(
            values.capability_declarations,
          ) as CapabilityKind[],
          visibility: requiredString("visibility") as
            "public" | "unlisted" | "private",
          join_mode: requiredString("join_mode") as
            "open" | "approval_required" | "invite_only",
          exit_contribution: requiredString("exit_contribution") as
            "required" | "not_required",
          exit_contribution_scope: requiredString("exit_contribution_scope") as
            "not_applicable" | "joining_members" | "all_members",
        },
      };
      return action.id === "preview_network_profile"
        ? { command: "preview_network_profile", payload: command }
        : { command: "update_network_profile", payload: command };
    }
    case "publish_network_discovery":
    case "update_network_discovery":
    case "unpublish_network_discovery": {
      const command = {
        network_name: requiredString("network_name"),
        index_url: requiredString("index_url"),
      };
      if (action.id === "publish_network_discovery")
        return { command: "publish_network_discovery", payload: command };
      if (action.id === "update_network_discovery")
        return { command: "update_network_discovery", payload: command };
      return { command: "unpublish_network_discovery", payload: command };
    }
    case "refresh_discovery_index":
      return {
        command: "refresh_discovery_index",
        payload: { index_url: optional(values, "index_url") },
      };
    case "retry_discovery_index":
      return { command: "retry_discovery_index" };
    case "preview_join_network":
      return {
        command: "preview_join_network",
        payload: { network_id: requiredString("network_id") },
      };
    case "join_network": {
      const networkId = optional(values, "network_id");
      const payload = {
        target: optional(values, "target") ?? "",
        network_name: optional(values, "network_name"),
        start: booleanValue(values.start, locale),
      };
      if (!networkId) {
        return { command: "join_network", payload };
      }
      return {
        command: "join_network",
        payload: {
          ...payload,
          network_id: networkId,
          preview_digest: requiredString("preview_digest"),
          network_policy_revision: Number(
            requireValue("network_policy_revision"),
          ),
          privacy_policy_revision: Number(
            requireValue("privacy_policy_revision"),
          ),
          terms_hash: requiredString("terms_hash"),
          acknowledged_at_unix_ms: Number(
            requireValue("acknowledged_at_unix_ms"),
          ),
          ...(optional(values, "exit_contribution_acknowledged") === "true"
            ? { exit_contribution_acknowledged: true }
            : {}),
        },
      };
    }
    case "submit_join_request":
      return {
        command: "submit_join_request",
        payload: {
          network_id: requiredString("network_id"),
          reason: requiredString("reason"),
        },
      };
    case "list_join_requests":
      return {
        command: "list_join_requests",
        payload: { network_id: requiredString("network_id") },
      };
    case "list_network_members":
      return {
        command: "list_network_members",
        payload: { network_id: requiredString("network_id") },
      };
    case "preview_member_removal":
      return {
        command: "preview_member_removal",
        payload: {
          network_id: requiredString("network_id"),
          target_node_id: requiredString("target_node_id"),
        },
      };
    case "change_member_lifecycle": {
      const reason = optional(values, "reason")?.trim();
      const role = optional(values, "role") ?? "member";
      if (!MEMBERSHIP_ROLES.has(role))
        throw new Error(label(locale, "validation.role_invalid"));
      return {
        command: "change_member_lifecycle",
        payload: {
          network_id: requiredString("network_id"),
          target_node_id: requiredString("target_node_id"),
          action: requiredString("action") as
            | "invite"
            | "approve"
            | "reject"
            | "suspend"
            | "resume"
            | "exit"
            | "remove"
            | "undo_remove",
          role: role as "owner" | "administrator" | "member",
          expected_revision: revisionValue(
            requireValue("expected_revision"),
            label(locale, "field.expected_revision"),
            locale,
          ),
          ...(reason ? { reason } : {}),
        },
      };
    }
    case "review_join_request": {
      const note = optional(values, "note");
      return {
        command: "review_join_request",
        payload: {
          network_id: requiredString("network_id"),
          review: {
            request_id: requiredString("request_id"),
            expected_revision: revisionValue(
              requireValue("expected_revision"),
              label(locale, "field.expected_revision"),
              locale,
            ),
            action: requiredString("action") as
              "approve" | "reject" | "request_information",
            ...(note ? { note } : {}),
          },
        },
      };
    }
    case "batch_review_join_requests":
      return {
        command: "batch_review_join_requests",
        payload: {
          network_id: requiredString("network_id"),
          reviews: parseJoinRequestReviews(values.reviews, locale),
        },
      };
    case "withdraw_join_request":
      return {
        command: "withdraw_join_request",
        payload: {
          network_id: requiredString("network_id"),
          request_id: requiredString("request_id"),
        },
      };
    case "resubmit_join_request":
      return {
        command: "resubmit_join_request",
        payload: {
          network_id: requiredString("network_id"),
          request_id: requiredString("request_id"),
          expected_revision: revisionValue(
            requireValue("expected_revision"),
            label(locale, "field.expected_revision"),
            locale,
          ),
          reason: requiredString("reason"),
        },
      };
    case "create_invitation":
      return {
        command: "create_invitation",
        payload: {
          network_id: requiredString("network_id"),
          role: requiredString("role") as "member" | "administrator",
          capabilities: splitNames(values.capabilities) as CapabilityKind[],
          max_uses: revisionValue(
            requireValue("max_uses"),
            label(locale, "field.invitation_max_uses"),
            locale,
          ),
          expires_in_ms: Number(requireValue("expires_in_ms")),
        },
      };
    case "list_invitations":
      return {
        command: "list_invitations",
        payload: { network_id: requiredString("network_id") },
      };
    case "get_invitation_status":
      return {
        command: "get_invitation_status",
        payload: {
          network_id: requiredString("network_id"),
          invitation_id: requiredString("invitation_id"),
        },
      };
    case "revoke_invitation":
      return {
        command: "revoke_invitation",
        payload: {
          network_id: requiredString("network_id"),
          invitation_id: requiredString("invitation_id"),
          expected_revision: revisionValue(
            requireValue("expected_revision"),
            label(locale, "field.expected_revision"),
            locale,
          ),
        },
      };
    case "preview_invitation":
      return {
        command: "preview_invitation",
        payload: {
          network_id: requiredString("network_id"),
          coordinator_url: requiredString("coordinator_url"),
          code: requiredString("code"),
        },
      };
    case "redeem_invitation":
      return {
        command: "redeem_invitation",
        payload: {
          coordinator_url: requiredString("coordinator_url"),
          request: {
            network_id: requiredString("network_id"),
            code: requiredString("code"),
            invitation_id: requiredString("invitation_id"),
            expected_invitation_revision: revisionValue(
              requireValue("expected_invitation_revision"),
              label(locale, "field.expected_revision"),
              locale,
            ),
            preview_digest: requiredString("preview_digest"),
            terms_hash: requiredString("terms_hash"),
            acknowledged_at_unix_ms: revisionValue(
              requireValue("acknowledged_at_unix_ms"),
              label(locale, "field.acknowledged_at"),
              locale,
            ),
            ...(optional(values, "exit_contribution_acknowledged") === "true"
              ? { exit_contribution_acknowledged: true }
              : {}),
          },
        },
      };
    case "leave_network":
      return {
        command: "leave_network",
        payload: { network_name: requiredString("network_name") },
      };
    case "acknowledge_membership_policy":
      return {
        command: "acknowledge_membership_policy",
        payload: {
          network_id: requiredString("network_id"),
          expected_terms_hash: requiredString("expected_terms_hash"),
          acknowledged_at_unix_ms: revisionValue(
            requireValue("acknowledged_at_unix_ms"),
            label(locale, "field.acknowledged_at"),
            locale,
          ),
          ...(optional(values, "exit_contribution_acknowledged") === "true"
            ? { exit_contribution_acknowledged: true }
            : {}),
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
    case "rebuild_tun":
      return {
        command: "rebuild_tun",
        payload: { network_name: requiredString("network_name") },
      };
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
    case "set_node_display_name":
      return {
        command: "set_node_display_name",
        payload: {
          display_name: optional(values, "display_name"),
        },
      };
    case "rotate_node_identity":
      return {
        command: "rotate_node_identity",
        payload: {
          expected_fingerprint: requiredString("expected_fingerprint"),
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
    case "test_policy":
      try {
        return {
          command: "test_policy",
          payload: parsePolicyTestInputJson(requiredString("policy_json")),
        };
      } catch (error) {
        throw new Error(policyJsonErrorText(error, locale));
      }
    case "get_policy":
      return { command: "get_policy" };
    case "preview_policy":
      try {
        return {
          command: "preview_policy",
          payload: {
            expected_revision: revisionValue(
              requireValue("expected_revision"),
              label(locale, "field.expected_revision"),
              locale,
            ),
            candidate: parsePolicySnapshotJson(
              requiredString("candidate_json"),
            ),
          },
        };
      } catch (error) {
        throw new Error(policyJsonErrorText(error, locale));
      }
    case "preview_policy_rollback":
      return {
        command: "preview_policy_rollback",
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
    case "apply_policy":
      try {
        return {
          command: "apply_policy",
          payload: {
            expected_revision: revisionValue(
              requireValue("expected_revision"),
              label(locale, "field.expected_revision"),
              locale,
            ),
            candidate: parsePolicySnapshotJson(
              requiredString("candidate_json"),
            ),
            network_ids: policyNetworkIds(requireValue("network_ids"), locale),
            includes_local_policy: booleanValue(
              requireValue("includes_local_policy"),
              locale,
            ),
            confirm_privilege_expansion: booleanValue(
              requireValue("confirm_privilege_expansion"),
              locale,
            ),
          },
        };
      } catch (error) {
        throw new Error(policyJsonErrorText(error, locale));
      }
    case "rollback_policy":
      return {
        command: "rollback_policy",
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
          network_ids: policyNetworkIds(requireValue("network_ids"), locale),
          includes_local_policy: booleanValue(
            requireValue("includes_local_policy"),
            locale,
          ),
          confirm_privilege_expansion: booleanValue(
            requireValue("confirm_privilege_expansion"),
            locale,
          ),
        },
      };
    case "get_policy_history":
      return { command: "get_policy_history" };
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

const MEMBER_STATUSES = new Set([
  "pending",
  "approved",
  "active",
  "degraded",
  "suspended",
  "exited",
  "removed",
  "expired",
]);
const MEMBER_GROUPS = new Set(["owner", "administrator", "member"]);
const MEMBER_PRESENCE_STATES = new Set(["online", "offline", "unknown"]);
const MEMBER_OBSERVATION_SOURCES = new Set([
  "live_session",
  "disconnect_observed",
  "not_observed",
]);

function optionalNullableInteger(value: unknown, message: string): void {
  if (value !== undefined && value !== null) nonNegativeInteger(value, message);
}

function memberRemovalPreview(value: unknown): void {
  if (!record(value)) fail("member removal preview invalid");
  stringValue(
    value.network_id,
    "member removal preview network id invalid",
    true,
  );
  stringValue(
    value.target_node_id,
    "member removal preview target node id invalid",
    true,
  );
  const role = stringValue(
    value.role,
    "member removal preview role invalid",
    true,
  );
  if (!MEMBERSHIP_ROLES.has(role)) fail("member removal preview role invalid");
  const status = stringValue(
    value.status,
    "member removal preview status invalid",
    true,
  );
  if (!MEMBER_STATUSES.has(status))
    fail("member removal preview status invalid");
  const lifecycleState = stringValue(
    value.lifecycle_state,
    "member removal preview lifecycle state invalid",
    true,
  );
  if (!MEMBER_LIFECYCLE_STATUSES.has(lifecycleState))
    fail("member removal preview lifecycle state invalid");
  nonNegativeInteger(value.revision, "member removal preview revision invalid");
  nonNegativeInteger(
    value.access_epoch,
    "member removal preview access epoch invalid",
  );
  if (!record(value.presence)) fail("member removal preview presence invalid");
  stringValue(
    value.presence.state,
    "member removal preview presence state invalid",
    true,
  );
  for (const flag of [
    "can_remove",
    "network_access_revoked",
    "file_access_revoked",
    "incomplete_transfers_resume_blocked",
    "credentials_revoked",
    "old_access_epoch_invalidated",
    "local_files_deleted",
    "safe_undo_requires_reapproval",
    "safe_undo_restores_old_credentials",
  ]) {
    booleanValueRaw(value[flag], `member removal preview ${flag} invalid`);
  }
  if (value.local_files_deleted !== false)
    fail("member removal preview must never delete local files");
  if (value.safe_undo_restores_old_credentials !== false)
    fail("member removal preview must never restore old credentials");
  const impactKeys = arrayValue(
    value.impact_keys,
    "member removal preview impact keys invalid",
  ).map((key) => stringValue(key, "member removal impact key invalid", true));
  unique(impactKeys, "member removal impact key");
  if (impactKeys.length === 0)
    fail("member removal preview must disclose its impact");
  for (const key of impactKeys) {
    if (!MEMBER_REMOVAL_IMPACT_KEYS.has(key))
      fail(`unknown member removal impact key: ${key}`);
  }
  if (value.active_capabilities !== undefined) {
    const capabilities = arrayValue(
      value.active_capabilities,
      "member removal preview capabilities invalid",
    ).map((capability) => {
      const kind = stringValue(
        capability,
        "member removal preview capability invalid",
        true,
      );
      if (!CAPABILITY_KINDS.has(kind as CapabilityKind))
        fail("member removal preview capability invalid");
      return kind;
    });
    unique(capabilities, "member removal preview capability");
  }
  if (value.denial_key !== undefined && value.denial_key !== null) {
    stringValue(
      value.denial_key,
      "member removal preview denial key invalid",
      true,
    );
  }
  if (value.display_name !== undefined && value.display_name !== null) {
    stringValue(
      value.display_name,
      "member removal preview display name invalid",
      true,
    );
  }
}

function memberLifecycleChange(value: unknown): void {
  if (!record(value)) fail("member lifecycle change invalid");
  stringValue(value.network_id, "member lifecycle network id invalid", true);
  stringValue(
    value.target_node_id,
    "member lifecycle target node id invalid",
    true,
  );
  const action = stringValue(
    value.action,
    "member lifecycle action invalid",
    true,
  );
  if (!MEMBER_LIFECYCLE_ACTIONS.has(action))
    fail("member lifecycle action invalid");
  const status = stringValue(
    value.status,
    "member lifecycle status invalid",
    true,
  );
  if (!MEMBER_LIFECYCLE_STATUSES.has(status))
    fail("member lifecycle status invalid");
  nonNegativeInteger(value.revision, "member lifecycle revision invalid");
  if (Number(value.revision) < 1) fail("member lifecycle revision invalid");
  nonNegativeInteger(
    value.access_epoch,
    "member lifecycle access epoch invalid",
  );
  if (Number(value.access_epoch) < 1)
    fail("member lifecycle access epoch invalid");
  stringValue(
    value.operation_id,
    "member lifecycle operation id invalid",
    true,
  );
  stringValue(value.audit_id, "member lifecycle audit id invalid", true);
  for (const flag of [
    "idempotent",
    "access_revoked",
    "requires_reauthentication",
    "requires_reapproval",
  ]) {
    booleanValueRaw(value[flag], `member lifecycle ${flag} invalid`);
  }
}

function memberDirectory(value: unknown): NetworkMemberDirectory {
  if (!record(value)) fail("member directory invalid");
  stringValue(value.network_id, "member directory network id invalid", true);
  nonNegativeInteger(
    value.generated_at_unix_ms,
    "member directory generation time invalid",
  );
  const viewerRole = stringValue(
    value.viewer_role,
    "member directory viewer role invalid",
    true,
  );
  if (!MEMBERSHIP_ROLES.has(viewerRole))
    fail("member directory viewer role invalid");
  const canManage = booleanValueRaw(
    value.can_manage_members,
    "member directory manage flag invalid",
  );
  const members = arrayValue(value.members, "member directory rows invalid");
  const seenNodeIds: string[] = [];
  for (const [index, entry] of members.entries()) {
    if (!record(entry)) fail(`member ${index} invalid`);
    const nodeId = stringValue(
      entry.node_id,
      `member ${index} node id invalid`,
      true,
    );
    seenNodeIds.push(nodeId);
    if (entry.display_name !== undefined && entry.display_name !== null) {
      stringValue(
        entry.display_name,
        `member ${index} display name invalid`,
        true,
      );
    }
    if (
      entry.identity_fingerprint !== undefined &&
      entry.identity_fingerprint !== null
    ) {
      stringValue(
        entry.identity_fingerprint,
        `member ${index} fingerprint invalid`,
        true,
      );
    }
    const role = stringValue(entry.role, `member ${index} role invalid`, true);
    if (!MEMBERSHIP_ROLES.has(role)) fail(`member ${index} role invalid`);
    const groups = arrayValue(
      entry.groups,
      `member ${index} groups invalid`,
    ).map((group) => {
      const value = stringValue(group, `member ${index} group invalid`, true);
      if (!MEMBER_GROUPS.has(value)) fail(`member ${index} group invalid`);
      return value;
    });
    unique(groups, `member ${index} group`);
    if (entry.capabilities !== undefined && entry.capabilities !== null) {
      const capabilities = arrayValue(
        entry.capabilities,
        `member ${index} capabilities invalid`,
      ).map((capability) => {
        const value = stringValue(
          capability,
          `member ${index} capability invalid`,
          true,
        );
        if (!CAPABILITY_KINDS.has(value as CapabilityKind))
          fail(`member ${index} capability invalid`);
        return value;
      });
      unique(capabilities, `member ${index} capability`);
    }
    const status = stringValue(
      entry.status,
      `member ${index} status invalid`,
      true,
    );
    if (!MEMBER_STATUSES.has(status)) fail(`member ${index} status invalid`);
    optionalNullableInteger(
      entry.joined_at_unix_ms,
      `member ${index} join time invalid`,
    );
    const lifecycleState = stringValue(
      entry.lifecycle_state,
      `member ${index} lifecycle state invalid`,
      true,
    );
    if (!MEMBER_LIFECYCLE_STATUSES.has(lifecycleState))
      fail(`member ${index} lifecycle state invalid`);
    const memberRevision = nonNegativeInteger(
      entry.revision,
      `member ${index} revision invalid`,
    );
    if (memberRevision < 1) fail(`member ${index} revision invalid`);
    const accessEpoch = nonNegativeInteger(
      entry.access_epoch,
      `member ${index} access epoch invalid`,
    );
    if (accessEpoch < 1) fail(`member ${index} access epoch invalid`);
    if (entry.available_actions !== undefined) {
      const actions = arrayValue(
        entry.available_actions,
        `member ${index} available actions invalid`,
      ).map((action) => {
        const value = stringValue(
          action,
          `member ${index} available action invalid`,
          true,
        );
        if (!MEMBER_LIFECYCLE_ACTIONS.has(value))
          fail(`member ${index} available action invalid`);
        return value;
      });
      unique(actions, `member ${index} available action`);
      if (!canManage)
        fail(`member ${index} exposes actions without governance`);
    }

    if (!record(entry.presence)) fail(`member ${index} presence invalid`);
    const presenceState = stringValue(
      entry.presence.state,
      `member ${index} presence state invalid`,
      true,
    );
    const source = stringValue(
      entry.presence.source,
      `member ${index} presence source invalid`,
      true,
    );
    if (!MEMBER_PRESENCE_STATES.has(presenceState))
      fail(`member ${index} presence state invalid`);
    if (!MEMBER_OBSERVATION_SOURCES.has(source))
      fail(`member ${index} presence source invalid`);
    optionalNullableInteger(
      entry.presence.observed_at_unix_ms,
      `member ${index} presence time invalid`,
    );
    const observedAt = entry.presence.observed_at_unix_ms;
    if (presenceState === "unknown") {
      if (
        source !== "not_observed" ||
        (observedAt !== undefined && observedAt !== null)
      )
        fail(`member ${index} unknown presence is inconsistent`);
    } else if (presenceState === "online") {
      if (
        source !== "live_session" ||
        observedAt === undefined ||
        observedAt === null
      )
        fail(`member ${index} online presence is inconsistent`);
    } else if (
      source !== "disconnect_observed" ||
      observedAt === undefined ||
      observedAt === null
    ) {
      fail(`member ${index} offline presence is inconsistent`);
    }
  }
  unique(seenNodeIds, "member node id");
  return value as NetworkMemberDirectory;
}

export function validateCommandOutcome(value: unknown): NodeCommandOutcome {
  if (!record(value) || !OUTCOME_STATUSES.has(String(value.status)))
    fail("unknown command outcome");
  const outcome = value as NodeCommandOutcome;
  if (outcome.status === "snapshot") validateSnapshot(outcome.data);
  if (outcome.status === "network_members") memberDirectory(outcome.data);
  if (outcome.status === "member_removal_preview")
    memberRemovalPreview(outcome.data);
  if (outcome.status === "member_lifecycle_changed")
    memberLifecycleChange(outcome.data);
  if (outcome.status === "configuration") configurationDocument(outcome.data);
  if (outcome.status === "configuration_preview")
    configurationPreview(outcome.data);
  if (outcome.status === "configuration_applied")
    configurationApplied(outcome.data);
  if (outcome.status === "configuration_history")
    configurationHistory(outcome.data);
  if (outcome.status === "policy_test") validatePolicyTestResult(outcome.data);
  if (outcome.status === "policy") policyCurrent(outcome.data);
  if (outcome.status === "policy_preview") policyPreview(outcome.data);
  if (outcome.status === "policy_applied") policyApplied(outcome.data);
  if (outcome.status === "policy_history") policyHistory(outcome.data);
  if (outcome.status === "policy_rolled_back") policyRolledBack(outcome.data);
  if (outcome.status === "network_profile_preview")
    networkProfilePreview(outcome.data);
  if (outcome.status === "network_profile_updated")
    networkProfileApplyResult(outcome.data);
  if (outcome.status === "identity_rotated") {
    if (!record(outcome.data)) fail("identity rotation result invalid");
    stringValue(outcome.data.old_node_id, "old node id invalid", true);
    stringValue(outcome.data.new_node_id, "new node id invalid", true);
    stringValue(outcome.data.old_fingerprint, "old fingerprint invalid", true);
    stringValue(outcome.data.new_fingerprint, "new fingerprint invalid", true);
    nonNegativeInteger(
      outcome.data.created_at_unix_ms,
      "identity creation time invalid",
    );
    nonNegativeInteger(
      outcome.data.affected_configured_network_count,
      "affected network count invalid",
    );
    booleanValueRaw(
      outcome.data.owner_initialized,
      "owner initialized flag invalid",
    );
    booleanValueRaw(
      outcome.data.owner_sessions_invalidated,
      "owner session invalidation flag invalid",
    );
  }
  if (outcome.status === "rejected" && !record(outcome.data))
    fail("rejected outcome is missing problem");
  if (outcome.status === "accepted" && !record(outcome.data))
    fail("accepted outcome is missing operation");
  return outcome;
}

export function validateOperationSnapshot(
  value: unknown,
): NodeOperationSnapshot {
  if (
    !record(value) ||
    typeof value.operation_id !== "string" ||
    !value.operation_id
  )
    fail("operation snapshot is missing its id");
  if (!OPERATION_STATES.has(String(value.state)))
    fail("operation snapshot has an unknown state");
  if (value.result !== undefined && value.result !== null) {
    if (!record(value.result)) fail("operation result is invalid");
    validateCommandOutcome(value.result.outcome);
  }
  return value as NodeOperationSnapshot;
}

export function operationStateIsTerminal(state: string): boolean {
  return OPERATION_TERMINAL_STATES.has(state);
}

export function validateCommandResponse(
  response: unknown,
  requestId: number,
): NodeCommandOutcome {
  if (!record(response) || response.schema_version !== HTTP_SCHEMA_VERSION)
    fail("command HTTP schema mismatch");
  if (!record(response.result) || response.result.request_id !== requestId)
    fail("command request id mismatch");
  return validateCommandOutcome(response.result.outcome);
}

export function validateAcceptedCommandResponse(
  response: NodeApiCommandResponse,
  requestId: number,
): NodeOperationSnapshot {
  if (!record(response) || response.schema_version !== HTTP_SCHEMA_VERSION)
    fail("command HTTP schema mismatch");
  if (response.request_id !== requestId) fail("command request id mismatch");
  if (response.result !== undefined && response.result !== null)
    fail("accepted command unexpectedly returned a synchronous result");
  if (!response.operation) fail("accepted command is missing its operation");
  const operation = validateOperationSnapshot(response.operation);
  if (operation.operation_id !== response.operation.operation_id)
    fail("accepted command operation id mismatch");
  return operation;
}

export function validateOperationResponse(
  response: NodeApiOperationResponse,
  operationId: string,
): NodeOperationSnapshot {
  if (!record(response) || response.schema_version !== HTTP_SCHEMA_VERSION)
    fail("operation HTTP schema mismatch");
  if (!record(response.operation))
    fail("operation HTTP response is missing its operation");
  const operation = validateOperationSnapshot(response.operation);
  if (operation.operation_id !== operationId)
    fail("operation HTTP response id mismatch");
  return operation;
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
  if (outcome.status === "policy") {
    return i18n().tr(locale, "outcome.policy_loaded", {
      revision: i18n().formatInteger(locale, outcome.data.revision),
    });
  }
  if (outcome.status === "policy_preview") {
    return i18n().tr(
      locale,
      outcome.data.revision_conflict
        ? "outcome.policy_preview_conflict"
        : "outcome.policy_previewed",
      {
        revision: i18n().formatInteger(locale, outcome.data.current_revision),
        changes: i18n().formatInteger(locale, outcome.data.diffs.length),
        objects: i18n().formatInteger(
          locale,
          outcome.data.affected_objects.length,
        ),
      },
    );
  }
  if (outcome.status === "policy_applied") {
    return i18n().tr(
      locale,
      outcome.data.no_op
        ? "outcome.policy_unchanged"
        : "outcome.policy_applied",
      {
        previous: i18n().formatInteger(locale, outcome.data.previous_revision),
        revision: i18n().formatInteger(locale, outcome.data.revision),
      },
    );
  }
  if (outcome.status === "policy_history") {
    return i18n().tr(locale, "outcome.policy_history", {
      count: i18n().formatInteger(locale, outcome.data.entries.length),
      revision: i18n().formatInteger(locale, outcome.data.current_revision),
    });
  }
  if (outcome.status === "policy_rolled_back") {
    return i18n().tr(locale, "outcome.policy_rolled_back", {
      target: i18n().formatInteger(locale, outcome.data.target_revision),
      revision: i18n().formatInteger(locale, outcome.data.revision),
    });
  }
  if (outcome.status === "policy_test") {
    return i18n().tr(locale, "outcome.policy_test", {
      decision: label(
        locale,
        outcome.data.decision.allowed
          ? "policy.result.allowed"
          : "policy.result.denied",
      ),
      reason: label(
        locale,
        `policy.choice.reason.${outcome.data.decision.reason}`,
      ),
    });
  }
  if (outcome.status === "network_created") {
    return i18n().tr(locale, "outcome.network_created", {
      network: outcome.data.network_name,
      network_id: outcome.data.network_id,
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
      next_steps: outcome.data.next_steps
        .map((step) => label(locale, `next_step.${step}`))
        .join(label(locale, "common.list_separator")),
    });
  }
  if (outcome.status === "network_profile_preview") {
    return i18n().tr(locale, "outcome.network_profile_previewed", {
      changes: i18n().formatInteger(locale, outcome.data.diff.length),
      warnings: i18n().formatInteger(locale, outcome.data.warnings.length),
    });
  }
  if (outcome.status === "network_profile_updated") {
    return i18n().tr(locale, "outcome.network_profile_updated", {
      previous: i18n().formatInteger(locale, outcome.data.previous_revision),
      revision: i18n().formatInteger(locale, outcome.data.revision),
      warnings: i18n().formatInteger(locale, outcome.data.warnings.length),
    });
  }
  if (outcome.status === "network_join_preview") {
    return i18n().tr(locale, "outcome.network_join_previewed", {
      network: outcome.data.network_name,
      members: i18n().formatInteger(locale, outcome.data.member_count),
    });
  }
  if (outcome.status === "network_members") {
    return i18n().tr(locale, "outcome.network_members_loaded", {
      count: i18n().formatInteger(locale, outcome.data.members.length),
      presence: i18n().formatInteger(
        locale,
        outcome.data.members.filter(
          (member) => member.presence.state === "online",
        ).length,
      ),
    });
  }
  if (outcome.status === "member_removal_preview") {
    return i18n().tr(locale, "outcome.member_removal_previewed", {
      member: outcome.data.display_name ?? outcome.data.target_node_id,
      state: label(
        locale,
        `member_directory.lifecycle_state.${outcome.data.lifecycle_state}`,
      ),
    });
  }
  if (outcome.status === "member_lifecycle_changed") {
    return i18n().tr(locale, "outcome.member_lifecycle_changed", {
      action: label(locale, `choice.member_lifecycle.${outcome.data.action}`),
      member: outcome.data.target_node_id,
      revision: i18n().formatInteger(locale, outcome.data.revision),
    });
  }
  if (outcome.status === "network_joined") {
    return i18n().tr(locale, "outcome.network_joined", {
      network: outcome.data.membership.network_name,
      state: label(
        locale,
        outcome.data.idempotent
          ? "outcome.membership_unchanged"
          : "outcome.membership_created",
      ),
    });
  }
  if (outcome.status === "network_left") {
    return i18n().tr(locale, "outcome.network_left", {
      network: outcome.data.network_name,
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
  if (outcome.status === "identity_rotated") {
    return i18n().tr(locale, "outcome.identity_rotated", {
      old_node_id: outcome.data.old_node_id,
      new_node_id: outcome.data.new_node_id,
      old_fingerprint: outcome.data.old_fingerprint,
      new_fingerprint: outcome.data.new_fingerprint,
      networks: i18n().formatInteger(
        locale,
        outcome.data.affected_configured_network_count,
      ),
      sessions: label(
        locale,
        outcome.data.owner_sessions_invalidated
          ? "identity.rotation.sessions_invalidated"
          : "identity.rotation.sessions_preserved",
      ),
    });
  }
  return label(locale, "outcome.completed");
}

export function allActions(manifest: NodeUiManifest): NodeUiAction[] {
  return manifest.pages.flatMap((page: NodeUiPage) => page.actions);
}
