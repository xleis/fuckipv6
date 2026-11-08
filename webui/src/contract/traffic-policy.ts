import type {
  ApplicationIdentity,
  ConfirmedTrafficDirectFallback,
  NodeId,
  NetworkId,
  TrafficAction,
  TrafficDefaultPolicy,
  TrafficFailureKind,
  TrafficFailurePolicy,
  TrafficFlow,
  TrafficMatch,
  TrafficPolicy,
  TrafficPolicyDraft,
  TrafficPolicyTestInput,
  TrafficProtocol,
  TrafficRule,
  TrafficRuleId,
} from "../../../contracts/typescript/node-contract-v1";
import { duplicateObjectKeyPath } from "./policy-test";

const NODE_ID = /^node_[A-Za-z0-9_-]{43}$/;
const NETWORK_ID = /^net_[A-Za-z0-9_-]{43}$/;
const RULE_ID = /^rule_[A-Za-z0-9_-]{43}$/;
const CIDR = /^[0-9A-Fa-f:.]+(?:\/[0-9]{1,3})?$/;
const PACKAGE_ID = /^[A-Za-z0-9._-]{1,255}$/;
const PROTOCOLS = new Set<TrafficProtocol>(["tcp", "udp", "icmp"]);
const CONFIRMERS = new Set(["owner", "administrator"]);
const FAILURE_KINDS = new Set<TrafficFailureKind>([
  "unknown_application",
  "dns_resolution_timeout",
  "dns_pollution_detected",
  "exit_unavailable",
  "network_selection_unavailable",
  "connection_timeout",
  "partial_connection_failure",
]);
const DIRECT_FALLBACK_SAFE = new Set<TrafficFailureKind>([
  "unknown_application",
  "exit_unavailable",
  "network_selection_unavailable",
  "connection_timeout",
]);

type PlainObject = Record<string, unknown>;

export class TrafficPolicyJsonError extends Error {
  readonly path: string;

  constructor(path: string, message: string) {
    super(message);
    this.name = "TrafficPolicyJsonError";
    this.path = path || "/";
  }
}

function pointer(parent: string, key: string | number): string {
  return `${parent}/${String(key).replaceAll("~", "~0").replaceAll("/", "~1")}`;
}

function fail(path: string, message: string): never {
  throw new TrafficPolicyJsonError(path, message);
}

function isRecord(value: unknown): value is PlainObject {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function objectAt(
  value: unknown,
  path: string,
  allowed: readonly string[],
  required: readonly string[],
): PlainObject {
  if (!isRecord(value)) fail(path, "must be an object");
  const keys = new Set(allowed);
  for (const key of Object.keys(value)) {
    if (!keys.has(key)) fail(pointer(path, key), `unknown field \`${key}\``);
  }
  for (const key of required) {
    if (!(key in value)) fail(pointer(path, key), `missing required field \`${key}\``);
  }
  return value;
}

function stringAt(value: unknown, path: string, pattern?: RegExp): string {
  if (typeof value !== "string") fail(path, "must be a string");
  if (pattern && !pattern.test(value)) fail(path, "has an invalid format");
  return value;
}

function integerAt(
  value: unknown,
  path: string,
  minimum: number,
  maximum = Number.MAX_SAFE_INTEGER,
): number {
  if (
    typeof value !== "number" ||
    !Number.isSafeInteger(value) ||
    value < minimum ||
    value > maximum
  ) {
    fail(path, `must be an integer between ${minimum} and ${maximum}`);
  }
  return value;
}

function arrayAt(value: unknown, path: string): unknown[] {
  if (!Array.isArray(value)) fail(path, "must be an array");
  return value;
}

function uniqueStrings(values: string[], path: string): void {
  const seen = new Set<string>();
  for (let index = 0; index < values.length; index += 1) {
    const value = values[index];
    if (value.trim() === "") fail(pointer(path, index), "must not be empty");
    if (seen.has(value)) fail(pointer(path, index), "must be unique");
    seen.add(value);
  }
}

function parseJson(text: string, path: string): unknown {
  const duplicatePath = duplicateObjectKeyPath(text);
  if (duplicatePath !== null) fail(duplicatePath, "duplicate field is not allowed");
  try {
    return JSON.parse(text);
  } catch (error) {
    fail(path, error instanceof Error ? error.message : "invalid JSON");
  }
}

function parseAction(value: unknown, path: string, nodeId: NodeId): TrafficAction {
  if (!isRecord(value)) fail(path, "must be an object");
  const kind = value.kind;
  if (kind === "direct" || kind === "reject") {
    objectAt(value, path, ["kind"], ["kind"]);
    return { kind };
  }
  if (kind === "network") {
    const parsed = objectAt(value, path, ["kind", "value"], ["kind", "value"]);
    const networkId = stringAt(parsed.value, pointer(path, "value"), NETWORK_ID);
    return { kind, value: networkId as NetworkId };
  }
  if (kind === "exit") {
    const parsed = objectAt(value, path, ["kind", "value"], ["kind", "value"]);
    const exitId = stringAt(parsed.value, pointer(path, "value"), NODE_ID);
    if (exitId === nodeId) fail(pointer(path, "value"), "cannot route through this node itself");
    return { kind, value: exitId as NodeId };
  }
  fail(pointer(path, "kind"), "must be direct, network, exit or reject");
}

function parseApplication(value: unknown, path: string): ApplicationIdentity {
  if (!isRecord(value)) fail(path, "must be an object");
  if (value.kind === "unknown") {
    objectAt(value, path, ["kind"], ["kind"]);
    return { kind: "unknown" };
  }
  if (value.kind === "executable_path") {
    const parsed = objectAt(value, path, ["kind", "value"], ["kind", "value"]);
    const executable = stringAt(parsed.value, pointer(path, "value"));
    if (
      executable.length > 4096 ||
      executable.trim() !== executable ||
      /[\0\r\n]/.test(executable)
    ) {
      fail(pointer(path, "value"), "is not a valid executable path identity");
    }
    return { kind: "executable_path", value: executable };
  }
  if (value.kind === "package_id") {
    const parsed = objectAt(value, path, ["kind", "value"], ["kind", "value"]);
    return {
      kind: "package_id",
      value: stringAt(parsed.value, pointer(path, "value"), PACKAGE_ID),
    };
  }
  fail(pointer(path, "kind"), "must be executable_path, package_id or unknown");
}

function parseMatcher(value: unknown, path: string): TrafficMatch {
  const parsed = objectAt(
    value,
    path,
    [
      "applications",
      "destination_cidrs",
      "destination_ports",
      "domain_suffixes",
      "exact_domains",
      "protocols",
    ],
    [],
  );
  const exactDomains = arrayAt(parsed.exact_domains ?? [], pointer(path, "exact_domains")).map(
    (item, index) => stringAt(item, pointer(pointer(path, "exact_domains"), index)),
  );
  const suffixes = arrayAt(
    parsed.domain_suffixes ?? [],
    pointer(path, "domain_suffixes"),
  ).map((item, index) =>
    stringAt(item, pointer(pointer(path, "domain_suffixes"), index)),
  );
  uniqueStrings(exactDomains, pointer(path, "exact_domains"));
  uniqueStrings(suffixes, pointer(path, "domain_suffixes"));
  const cidrs = arrayAt(
    parsed.destination_cidrs ?? [],
    pointer(path, "destination_cidrs"),
  ).map((item, index) =>
    stringAt(item, pointer(pointer(path, "destination_cidrs"), index), CIDR),
  );
  uniqueStrings(cidrs, pointer(path, "destination_cidrs"));
  const ports = arrayAt(
    parsed.destination_ports ?? [],
    pointer(path, "destination_ports"),
  ).map((item, index) => {
    const itemPath = pointer(pointer(path, "destination_ports"), index);
    const range = objectAt(item, itemPath, ["start", "end"], ["start", "end"]);
    const start = integerAt(range.start, pointer(itemPath, "start"), 1, 65_535);
    const end = integerAt(range.end, pointer(itemPath, "end"), start, 65_535);
    return { start, end };
  });
  const protocols = arrayAt(parsed.protocols ?? [], pointer(path, "protocols")).map(
    (item, index) => {
      const protocol = stringAt(item, pointer(pointer(path, "protocols"), index));
      if (!PROTOCOLS.has(protocol as TrafficProtocol)) {
        fail(pointer(pointer(path, "protocols"), index), "has an unknown protocol");
      }
      return protocol as TrafficProtocol;
    },
  );
  uniqueStrings(protocols, pointer(path, "protocols"));
  const applications = arrayAt(
    parsed.applications ?? [],
    pointer(path, "applications"),
  ).map((item, index) =>
    parseApplication(item, pointer(pointer(path, "applications"), index)),
  );
  for (const application of applications) {
    if (application.kind === "unknown") {
      fail(pointer(path, "applications"), "unknown application cannot be a positive matcher");
    }
  }
  return {
    ...(exactDomains.length ? { exact_domains: exactDomains } : {}),
    ...(suffixes.length ? { domain_suffixes: suffixes } : {}),
    ...(cidrs.length ? { destination_cidrs: cidrs } : {}),
    ...(ports.length ? { destination_ports: ports } : {}),
    ...(protocols.length ? { protocols } : {}),
    ...(applications.length ? { applications } : {}),
  };
}

function parseRule(value: unknown, path: string, nodeId: NodeId): TrafficRule {
  const parsed = objectAt(
    value,
    path,
    ["action", "description", "enabled", "matcher", "priority", "rule_id"],
    ["action", "matcher", "priority", "rule_id"],
  );
  const ruleId = stringAt(parsed.rule_id, pointer(path, "rule_id"), RULE_ID);
  const description = parsed.description === undefined
    ? ""
    : stringAt(parsed.description, pointer(path, "description"));
  if (description.length > 256) fail(pointer(path, "description"), "must not exceed 256 characters");
  const enabled = parsed.enabled === undefined
    ? true
    : parsed.enabled;
  if (typeof enabled !== "boolean") fail(pointer(path, "enabled"), "must be a boolean");
  return {
    rule_id: ruleId as TrafficRuleId,
    priority: integerAt(parsed.priority, pointer(path, "priority"), 1, 4_294_967_295),
    enabled,
    description,
    matcher: parseMatcher(parsed.matcher, pointer(path, "matcher")),
    action: parseAction(parsed.action, pointer(path, "action"), nodeId),
  };
}

function parseDefaultPolicy(value: unknown, path: string, nodeId: NodeId): TrafficDefaultPolicy {
  const parsed = objectAt(
    value,
    path,
    ["action", "confirmed_at_unix_ms", "confirmed_by"],
    ["action", "confirmed_at_unix_ms", "confirmed_by"],
  );
  const confirmedBy = stringAt(parsed.confirmed_by, pointer(path, "confirmed_by"));
  if (!CONFIRMERS.has(confirmedBy)) fail(pointer(path, "confirmed_by"), "must be owner or administrator");
  return {
    action: parseAction(parsed.action, pointer(path, "action"), nodeId),
    confirmed_by: confirmedBy as TrafficDefaultPolicy["confirmed_by"],
    confirmed_at_unix_ms: integerAt(
      parsed.confirmed_at_unix_ms,
      pointer(path, "confirmed_at_unix_ms"),
      1,
    ),
  };
}

function parseFailurePolicy(value: unknown, path: string): TrafficFailurePolicy {
  const parsed = objectAt(value, path, ["direct_fallback"], []);
  if (parsed.direct_fallback === undefined || parsed.direct_fallback === null) {
    return parsed.direct_fallback === null ? { direct_fallback: null } : {};
  }
  const fallbackPath = pointer(path, "direct_fallback");
  const fallback = objectAt(
    parsed.direct_fallback,
    fallbackPath,
    ["allowed_failures", "confirmed_at_unix_ms", "confirmed_by"],
    ["allowed_failures", "confirmed_at_unix_ms", "confirmed_by"],
  );
  const allowed = arrayAt(fallback.allowed_failures, pointer(fallbackPath, "allowed_failures")).map(
    (item, index) => {
      const failure = stringAt(item, pointer(pointer(fallbackPath, "allowed_failures"), index));
      if (!FAILURE_KINDS.has(failure as TrafficFailureKind)) {
        fail(pointer(pointer(fallbackPath, "allowed_failures"), index), "has an unknown failure class");
      }
      if (!DIRECT_FALLBACK_SAFE.has(failure as TrafficFailureKind)) {
        fail(
          pointer(pointer(fallbackPath, "allowed_failures"), index),
          "must always fail closed and cannot be included in direct fallback",
        );
      }
      return failure as TrafficFailureKind;
    },
  );
  uniqueStrings(allowed, pointer(fallbackPath, "allowed_failures"));
  if (!allowed.length) fail(pointer(fallbackPath, "allowed_failures"), "must not be empty");
  const confirmedBy = stringAt(fallback.confirmed_by, pointer(fallbackPath, "confirmed_by"));
  if (!CONFIRMERS.has(confirmedBy)) {
    fail(pointer(fallbackPath, "confirmed_by"), "must be owner or administrator");
  }
  const result: ConfirmedTrafficDirectFallback = {
    allowed_failures: allowed,
    confirmed_by: confirmedBy as ConfirmedTrafficDirectFallback["confirmed_by"],
    confirmed_at_unix_ms: integerAt(
      fallback.confirmed_at_unix_ms,
      pointer(fallbackPath, "confirmed_at_unix_ms"),
      1,
    ),
  };
  return { direct_fallback: result };
}

function parseRules(
  value: unknown | undefined,
  path: string,
  nodeId: NodeId,
): TrafficRule[] {
  if (value === undefined) return [];
  const rules = arrayAt(value, path).map((item, index) =>
    parseRule(item, pointer(path, index), nodeId),
  );
  const seen = new Set<string>();
  for (let index = 0; index < rules.length; index += 1) {
    const rule = rules[index];
    if (seen.has(rule.rule_id)) fail(pointer(pointer(path, index), "rule_id"), "must be unique");
    seen.add(rule.rule_id);
    if (rule.priority !== index + 1) {
      fail(pointer(pointer(path, index), "priority"), "must match the visible rule order");
    }
  }
  return rules;
}

function parsePolicyDraftValue(value: unknown): TrafficPolicyDraft {
  const parsed = objectAt(
    value,
    "",
    ["default_policy", "failure_policy", "node_id", "rules"],
    ["default_policy", "node_id"],
  );
  const nodeId = stringAt(parsed.node_id, "/node_id", NODE_ID) as NodeId;
  return {
    node_id: nodeId,
    default_policy: parseDefaultPolicy(parsed.default_policy, "/default_policy", nodeId),
    ...(parsed.failure_policy === undefined
      ? {}
      : { failure_policy: parseFailurePolicy(parsed.failure_policy, "/failure_policy") }),
    rules: parseRules(parsed.rules, "/rules", nodeId),
  };
}

export function parseTrafficPolicyDraftJson(text: string): TrafficPolicyDraft {
  return parsePolicyDraftValue(parseJson(text, "/"));
}

export function parseTrafficPolicyJson(text: string): TrafficPolicy {
  const parsed = objectAt(
    parseJson(text, "/"),
    "",
    ["default_policy", "failure_policy", "node_id", "revision", "rules"],
    ["default_policy", "node_id", "revision"],
  );
  const revision = integerAt(parsed.revision, "/revision", 1);
  const draft = parsePolicyDraftValue({
    node_id: parsed.node_id,
    default_policy: parsed.default_policy,
    ...(parsed.failure_policy === undefined ? {} : { failure_policy: parsed.failure_policy }),
    ...(parsed.rules === undefined ? {} : { rules: parsed.rules }),
  });
  return { ...draft, revision };
}

function parseFlow(value: unknown, path: string): TrafficFlow {
  const parsed = objectAt(
    value,
    path,
    ["application", "destination_ip", "destination_port", "domain", "protocol"],
    ["application", "destination_ip", "destination_port", "protocol"],
  );
  const domain = parsed.domain === undefined || parsed.domain === null
    ? null
    : stringAt(parsed.domain, pointer(path, "domain"));
  const protocol = stringAt(parsed.protocol, pointer(path, "protocol"));
  if (!PROTOCOLS.has(protocol as TrafficProtocol)) {
    fail(pointer(path, "protocol"), "has an unknown protocol");
  }
  return {
    domain,
    destination_ip: stringAt(parsed.destination_ip, pointer(path, "destination_ip")),
    destination_port: integerAt(parsed.destination_port, pointer(path, "destination_port"), 1, 65_535),
    protocol: protocol as TrafficProtocol,
    application: parseApplication(parsed.application, pointer(path, "application")),
  };
}

function parseSource(value: unknown, path: string): TrafficPolicyTestInput["source"] {
  if (!isRecord(value)) fail(path, "must be an object");
  if (value.kind === "manual") {
    objectAt(value, path, ["kind"], ["kind"]);
    return { kind: "manual" };
  }
  if (value.kind === "activity") {
    const parsed = objectAt(value, path, ["activity_id", "kind"], ["activity_id", "kind"]);
    const activityId = stringAt(parsed.activity_id, pointer(path, "activity_id"));
    if (!activityId || activityId.length > 128 || activityId.trim() !== activityId || /\p{Cc}/u.test(activityId)) {
      fail(pointer(path, "activity_id"), "is invalid");
    }
    return { kind: "activity", activity_id: activityId };
  }
  if (value.kind === "diagnostic") {
    const parsed = objectAt(value, path, ["diagnostic_id", "kind"], ["diagnostic_id", "kind"]);
    const diagnosticId = stringAt(parsed.diagnostic_id, pointer(path, "diagnostic_id"));
    if (!diagnosticId || diagnosticId.length > 128 || diagnosticId.trim() !== diagnosticId || /\p{Cc}/u.test(diagnosticId)) {
      fail(pointer(path, "diagnostic_id"), "is invalid");
    }
    return { kind: "diagnostic", diagnostic_id: diagnosticId };
  }
  fail(pointer(path, "kind"), "must be manual, activity or diagnostic");
}

export function parseTrafficPolicyTestInputJson(text: string): TrafficPolicyTestInput {
  const parsed = objectAt(
    parseJson(text, "/"),
    "",
    ["fixed_exit", "fixed_network", "flow", "network_id", "source"],
    ["flow", "network_id", "source"],
  );
  const networkId = stringAt(parsed.network_id, "/network_id", NETWORK_ID) as NetworkId;
  const fixedExit = parsed.fixed_exit === undefined || parsed.fixed_exit === null
    ? null
    : stringAt(parsed.fixed_exit, "/fixed_exit", NODE_ID) as NodeId;
  const fixedNetwork = parsed.fixed_network === undefined || parsed.fixed_network === null
    ? null
    : stringAt(parsed.fixed_network, "/fixed_network", NETWORK_ID) as NetworkId;
  if (fixedExit && fixedNetwork) {
    fail("/fixed_network", "cannot set both fixed_exit and fixed_network");
  }
  return {
    network_id: networkId,
    flow: parseFlow(parsed.flow, "/flow"),
    source: parseSource(parsed.source, "/source"),
    ...(fixedExit ? { fixed_exit: fixedExit } : {}),
    ...(fixedNetwork ? { fixed_network: fixedNetwork } : {}),
  };
}
