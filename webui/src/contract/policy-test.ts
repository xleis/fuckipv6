import type {
  EffectiveAccess,
  EffectiveAccessReason,
  EffectivePolicyLayer,
  NetworkAcl,
  NetworkAclGrant,
  NetworkRole,
  NodeContributionVeto,
  NodeLocalAccessPolicy,
  NodeNetworkScope,
  PermissionCondition,
  PermissionEffect,
  PermissionIntent,
  PermissionRule,
  PolicyAction,
  PolicyPrecedence,
  PolicyPrecedenceExplanation,
  PolicyRuleExplanation,
  PolicyRuleSource,
  PolicySnapshot,
  PolicyTestInput,
  PolicyTestRequest,
  PolicyTestResult,
  PolicyValidationIssue,
  PolicyValidationSource,
  ResourceKind,
  SubjectSelector,
} from "../../../contracts/typescript/node-contract-v1";

const NODE_ID = /^node_[A-Za-z0-9_-]{43}$/;
const NETWORK_ID = /^net_[A-Za-z0-9_-]{43}$/;
const GROUP_ID = /^group_[A-Za-z0-9_-]{43}$/;
const SECURITY_RULE_ID = /^srule_[A-Za-z0-9_-]{43}$/;

const NETWORK_ROLES = new Set<NetworkRole>([
  "owner",
  "administrator",
  "member",
  "temporary_coordinator",
]);
const POLICY_ACTIONS = new Set<PolicyAction>([
  "read",
  "create",
  "update",
  "delete",
  "approve",
  "reject",
  "remove",
  "enable",
  "disable",
  "use",
  "recover",
  "export",
  "test",
]);
const RESOURCE_KINDS = new Set<ResourceKind>([
  "network",
  "ownership",
  "membership",
  "group",
  "security_policy",
  "traffic_policy",
  "capability",
  "exit_grant",
  "file_share",
  "audit",
  "coordinator_term",
  "keys",
]);
const PERMISSION_INTENTS = new Set<PermissionIntent>([
  "read_only",
  "mutating",
]);
const PERMISSION_EFFECTS = new Set<PermissionEffect>(["allow", "deny"]);
const CONTRIBUTION_KINDS = new Set(["relay", "exit", "file"]);
const SUBJECT_KINDS = new Set(["any_member", "role", "node", "group"]);
const CONDITION_KINDS = new Set([
  "membership_active",
  "recent_reauthentication",
  "resource_in_network",
  "non_emergency_coordinator",
]);
const ACCESS_REASONS = new Set<EffectiveAccessReason>([
  "allowed",
  "cross_network_denied",
  "network_scope_restricted",
  "local_contribution_veto",
  "authorization_expired",
  "authorization_revoked",
  "recent_reauthentication_required",
  "explicit_deny",
  "role_ceiling_exceeded",
  "temporary_coordinator_read_only",
  "default_denied",
  "invalid_policy",
]);
const POLICY_LAYERS = new Set<EffectivePolicyLayer>([
  "local",
  "network",
  "request",
]);
const RULE_SOURCES = new Set<PolicyRuleSource>([
  "role_baseline",
  "local_policy_rule",
  "local_contribution_veto",
  "local_network_scope",
  "network_acl_rule",
  "network_acl_grant",
  "request",
  "access_context",
]);
const VALIDATION_SOURCES = new Set<PolicyValidationSource>([
  "local_policy",
  "network_acl",
  "test_input",
]);
const PRECEDENCE_VALUES = new Set<PolicyPrecedence>([
  "cross_network_denied",
  "invalid_or_expired_policy",
  "network_scope_restricted",
  "local_contribution_veto",
  "explicit_deny",
  "temporary_coordinator_read_only",
  "role_ceiling_exceeded",
  "authorization_revoked",
  "authorization_expired",
  "recent_reauthentication_required",
  "default_denied",
  "allow",
]);

const ROOT_FIELDS = new Set([
  "network_id",
  "actor",
  "role",
  "groups",
  "membership_active",
  "recent_reauthentication",
  "emergency_coordinator",
  "request",
  "local_policy",
  "network_acl",
  "now_unix_ms",
]);
const SNAPSHOT_FIELDS = new Set(["local_policy", "network_acls"]);
const REQUEST_FIELDS = new Set([
  "action",
  "resource",
  "resource_network_id",
  "object_id",
  "intent",
]);
const LOCAL_POLICY_FIELDS = new Set([
  "node_id",
  "revision",
  "issued_at_unix_ms",
  "expires_at_unix_ms",
  "network_scope",
  "rules",
  "contribution_vetoes",
]);
const NETWORK_SCOPE_FIELDS = new Set(["kind", "network_ids"]);
const RULE_FIELDS = new Set([
  "rule_id",
  "subjects",
  "resources",
  "actions",
  "effect",
  "conditions",
]);
const SUBJECT_FIELDS = new Set(["kind", "value"]);
const CONDITION_FIELDS = new Set(["kind", "value"]);
const VETO_FIELDS = new Set(["veto_id", "network_id", "contributions"]);
const NETWORK_ACL_FIELDS = new Set([
  "network_id",
  "revision",
  "issued_at_unix_ms",
  "expires_at_unix_ms",
  "rules",
  "grants",
]);
const GRANT_FIELDS = new Set([
  "grant_id",
  "subjects",
  "resources",
  "actions",
  "issued_at_unix_ms",
  "expires_at_unix_ms",
  "revoked_at_unix_ms",
  "revoked_by",
  "requires_recent_reauthentication",
]);
const DECISION_FIELDS = new Set([
  "allowed",
  "code",
  "evaluated_at_unix_ms",
  "expired_grant_ids",
  "layer",
  "local_policy_revision",
  "matched_allow_rule_ids",
  "matched_deny_rule_ids",
  "network_acl_revision",
  "reason",
  "reauthentication_required_grant_ids",
  "reauthentication_satisfied",
  "revoked_grant_ids",
  "rule_id",
]);
const RESULT_FIELDS = new Set([
  "decision",
  "explanations",
  "precedence",
  "validation_issues",
]);
const EXPLANATION_FIELDS = new Set([
  "source",
  "source_path",
  "rule_id",
  "effect",
  "subjects",
  "resources",
  "actions",
  "conditions",
  "matched",
  "applied",
  "precedence",
]);
const PRECEDENCE_EXPLANATION_FIELDS = new Set([
  "precedence",
  "triggered",
  "source",
  "rule_id",
]);
const VALIDATION_ISSUE_FIELDS = new Set([
  "path",
  "code",
  "source",
  "related_rule_id",
  "message",
]);

type PlainObject = Record<string, unknown>;

export class PolicyJsonError extends Error {
  readonly path: string;

  constructor(path: string, message: string) {
    super(message);
    this.name = "PolicyJsonError";
    this.path = path || "/";
  }
}

function pointer(parent: string, key: string | number): string {
  return `${parent}/${String(key).replaceAll("~", "~0").replaceAll("/", "~1")}`;
}

function fail(path: string, message: string): never {
  throw new PolicyJsonError(path, message);
}

function objectAt(
  value: unknown,
  path: string,
  allowed: Set<string>,
  required: readonly string[],
): PlainObject {
  if (!record(value)) fail(path, "must be an object");
  for (const key of Object.keys(value)) {
    if (!allowed.has(key)) fail(pointer(path, key), `unknown field \`${key}\``);
  }
  for (const key of required) {
    if (!(key in value)) fail(pointer(path, key), `missing required field \`${key}\``);
  }
  return value;
}

function record(value: unknown): value is PlainObject {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function stringAt(
  value: unknown,
  path: string,
  nonEmpty = false,
): string {
  if (typeof value !== "string" || (nonEmpty && value.trim() === ""))
    fail(path, nonEmpty ? "must be a non-empty string" : "must be a string");
  return value;
}

function idAt(
  value: unknown,
  path: string,
  pattern: RegExp,
  description: string,
): string {
  const id = stringAt(value, path, true);
  if (!pattern.test(id)) fail(path, `must be a valid ${description}`);
  return id;
}

function boolAt(value: unknown, path: string): boolean {
  if (typeof value !== "boolean") fail(path, "must be a boolean");
  return value;
}

function uintAt(value: unknown, path: string): number {
  if (!Number.isSafeInteger(value) || Number(value) < 0)
    fail(path, "must be a non-negative safe integer");
  return Number(value);
}

function arrayAt(value: unknown, path: string): unknown[] {
  if (!Array.isArray(value)) fail(path, "must be an array");
  return value;
}

function enumAt<T extends string>(
  value: unknown,
  path: string,
  values: Set<T>,
  description: string,
): T {
  const candidate = stringAt(value, path, true) as T;
  if (!values.has(candidate)) fail(path, `unknown ${description} \`${candidate}\``);
  return candidate;
}

function optionalNullableId(
  value: unknown,
  path: string,
  pattern: RegExp,
  description: string,
): void {
  if (value === undefined || value === null) return;
  idAt(value, path, pattern, description);
}

/**
 * Parse with the same fail-closed shape rules as the Rust command adapter.
 * JSON.parse would otherwise silently keep the last duplicate object key.
 */
export function parsePolicyTestInputJson(source: string): PolicyTestInput {
  const duplicatePath = duplicateObjectKeyPath(source);
  if (duplicatePath !== null)
    fail(duplicatePath, "duplicate field is not allowed");

  let parsed: unknown;
  try {
    parsed = JSON.parse(source);
  } catch (error) {
    fail("/", error instanceof Error ? error.message : String(error));
  }
  return policyTestInputAt(parsed, "");
}

/**
 * Strictly parse the complete policy transaction document. Unknown fields,
 * duplicate keys, and trailing input fail closed before a preview is requested.
 */
export function parsePolicySnapshotJson(source: string): PolicySnapshot {
  const duplicatePath = duplicateObjectKeyPath(source);
  if (duplicatePath !== null)
    fail(duplicatePath, "duplicate field is not allowed");

  let parsed: unknown;
  try {
    parsed = JSON.parse(source);
  } catch (error) {
    fail("/", error instanceof Error ? error.message : String(error));
  }
  return validatePolicySnapshot(parsed);
}

export function validatePolicySnapshot(value: unknown): PolicySnapshot {
  const snapshot = objectAt(value, "", SNAPSHOT_FIELDS, []);
  if (snapshot.local_policy !== undefined && snapshot.local_policy !== null)
    localPolicyAt(snapshot.local_policy, "/local_policy");
  if (snapshot.network_acls !== undefined) {
    const networkIds = new Set<string>();
    arrayAt(snapshot.network_acls, "/network_acls").forEach((acl, index) => {
      const parsed = networkAclAt(acl, `/network_acls/${index}`);
      if (networkIds.has(parsed.network_id))
        fail(`/network_acls/${index}/network_id`, "duplicate network ACL");
      networkIds.add(parsed.network_id);
    });
  }
  return value as PolicySnapshot;
}

export function validatePolicyTestResult(value: unknown): PolicyTestResult {
  const result = objectAt(value, "", RESULT_FIELDS, [
    "decision",
    "explanations",
    "precedence",
    "validation_issues",
  ]);
  effectiveAccessAt(result.decision, "/decision");
  const explanations = arrayAt(result.explanations, "/explanations");
  explanations.forEach((entry, index) =>
    ruleExplanationAt(entry, `/explanations/${index}`),
  );
  const precedence = arrayAt(result.precedence, "/precedence");
  precedence.forEach((entry, index) =>
    precedenceExplanationAt(entry, `/precedence/${index}`),
  );
  const issues = arrayAt(result.validation_issues, "/validation_issues");
  issues.forEach((entry, index) =>
    validationIssueAt(entry, `/validation_issues/${index}`),
  );
  return value as PolicyTestResult;
}

function policyTestInputAt(value: unknown, path: string): PolicyTestInput {
  const root = objectAt(value, path, ROOT_FIELDS, [
    "network_id",
    "actor",
    "role",
    "membership_active",
    "recent_reauthentication",
    "emergency_coordinator",
    "request",
    "now_unix_ms",
  ]);
  idAt(root.network_id, pointer(path, "network_id"), NETWORK_ID, "network id");
  idAt(root.actor, pointer(path, "actor"), NODE_ID, "node id");
  enumAt(root.role, pointer(path, "role"), NETWORK_ROLES, "network role");
  if (root.groups !== undefined) {
    arrayAt(root.groups, pointer(path, "groups")).forEach((group, index) =>
      idAt(
        group,
        `${pointer(path, "groups")}/${index}`,
        GROUP_ID,
        "group id",
      ),
    );
  }
  boolAt(root.membership_active, pointer(path, "membership_active"));
  boolAt(
    root.recent_reauthentication,
    pointer(path, "recent_reauthentication"),
  );
  boolAt(root.emergency_coordinator, pointer(path, "emergency_coordinator"));
  policyRequestAt(root.request, pointer(path, "request"));
  if (root.local_policy !== undefined && root.local_policy !== null)
    localPolicyAt(root.local_policy, pointer(path, "local_policy"));
  if (root.network_acl !== undefined && root.network_acl !== null)
    networkAclAt(root.network_acl, pointer(path, "network_acl"));
  uintAt(root.now_unix_ms, pointer(path, "now_unix_ms"));
  return value as PolicyTestInput;
}

function policyRequestAt(value: unknown, path: string): PolicyTestRequest {
  const request = objectAt(value, path, REQUEST_FIELDS, [
    "action",
    "resource",
    "resource_network_id",
    "intent",
  ]);
  enumAt(
    request.action,
    pointer(path, "action"),
    POLICY_ACTIONS,
    "policy action",
  );
  enumAt(
    request.resource,
    pointer(path, "resource"),
    RESOURCE_KINDS,
    "resource kind",
  );
  idAt(
    request.resource_network_id,
    pointer(path, "resource_network_id"),
    NETWORK_ID,
    "network id",
  );
  if (request.object_id !== undefined && request.object_id !== null)
    stringAt(request.object_id, pointer(path, "object_id"));
  enumAt(
    request.intent,
    pointer(path, "intent"),
    PERMISSION_INTENTS,
    "permission intent",
  );
  return value as PolicyTestRequest;
}

function localPolicyAt(value: unknown, path: string): NodeLocalAccessPolicy {
  const policy = objectAt(value, path, LOCAL_POLICY_FIELDS, [
    "node_id",
    "revision",
    "issued_at_unix_ms",
    "expires_at_unix_ms",
    "network_scope",
  ]);
  idAt(policy.node_id, pointer(path, "node_id"), NODE_ID, "node id");
  uintAt(policy.revision, pointer(path, "revision"));
  uintAt(policy.issued_at_unix_ms, pointer(path, "issued_at_unix_ms"));
  uintAt(policy.expires_at_unix_ms, pointer(path, "expires_at_unix_ms"));
  networkScopeAt(policy.network_scope, pointer(path, "network_scope"));
  if (policy.rules !== undefined)
    arrayAt(policy.rules, pointer(path, "rules")).forEach((rule, index) =>
      permissionRuleAt(rule, `${pointer(path, "rules")}/${index}`),
    );
  if (policy.contribution_vetoes !== undefined)
    arrayAt(
      policy.contribution_vetoes,
      pointer(path, "contribution_vetoes"),
    ).forEach((veto, index) =>
      contributionVetoAt(
        veto,
        `${pointer(path, "contribution_vetoes")}/${index}`,
      ),
    );
  return value as NodeLocalAccessPolicy;
}

function networkScopeAt(value: unknown, path: string): NodeNetworkScope {
  if (!record(value)) fail(path, "must be an object");
  const kind = enumAt(
    value.kind,
    pointer(path, "kind"),
    new Set(["all", "only"] as const),
    "network scope kind",
  );
  objectAt(
    value,
    path,
    NETWORK_SCOPE_FIELDS,
    kind === "only" ? ["kind", "network_ids"] : ["kind"],
  );
  if (kind === "only") {
    arrayAt(value.network_ids, pointer(path, "network_ids")).forEach(
      (networkId, index) =>
        idAt(
          networkId,
          `${pointer(path, "network_ids")}/${index}`,
          NETWORK_ID,
          "network id",
        ),
    );
  }
  return value as NodeNetworkScope;
}

function permissionRuleAt(value: unknown, path: string): PermissionRule {
  const rule = objectAt(value, path, RULE_FIELDS, [
    "rule_id",
    "subjects",
    "resources",
    "actions",
    "effect",
  ]);
  idAt(
    rule.rule_id,
    pointer(path, "rule_id"),
    SECURITY_RULE_ID,
    "security rule id",
  );
  arrayAt(rule.subjects, pointer(path, "subjects")).forEach((subject, index) =>
    subjectAt(subject, `${pointer(path, "subjects")}/${index}`),
  );
  arrayAt(rule.resources, pointer(path, "resources")).forEach(
    (resource, index) =>
      enumAt(
        resource,
        `${pointer(path, "resources")}/${index}`,
        RESOURCE_KINDS,
        "resource kind",
      ),
  );
  arrayAt(rule.actions, pointer(path, "actions")).forEach((action, index) =>
    enumAt(
      action,
      `${pointer(path, "actions")}/${index}`,
      POLICY_ACTIONS,
      "policy action",
    ),
  );
  enumAt(
    rule.effect,
    pointer(path, "effect"),
    PERMISSION_EFFECTS,
    "permission effect",
  );
  if (rule.conditions !== undefined)
    arrayAt(rule.conditions, pointer(path, "conditions")).forEach(
      (condition, index) =>
        permissionConditionAt(
          condition,
          `${pointer(path, "conditions")}/${index}`,
        ),
    );
  return value as PermissionRule;
}

function subjectAt(value: unknown, path: string): SubjectSelector {
  if (!record(value)) fail(path, "must be an object");
  const kind = enumAt(
    value.kind,
    pointer(path, "kind"),
    SUBJECT_KINDS,
    "subject kind",
  );
  if (kind === "any_member") {
    objectAt(value, path, SUBJECT_FIELDS, ["kind"]);
    return value as SubjectSelector;
  }
  objectAt(value, path, SUBJECT_FIELDS, ["kind", "value"]);
  if (kind === "role")
    enumAt(
      value.value,
      pointer(path, "value"),
      NETWORK_ROLES,
      "network role",
    );
  else if (kind === "node")
    idAt(value.value, pointer(path, "value"), NODE_ID, "node id");
  else idAt(value.value, pointer(path, "value"), GROUP_ID, "group id");
  return value as SubjectSelector;
}

function permissionConditionAt(
  value: unknown,
  path: string,
): PermissionCondition {
  if (!record(value)) fail(path, "must be an object");
  const kind = enumAt(
    value.kind,
    pointer(path, "kind"),
    CONDITION_KINDS,
    "permission condition",
  );
  if (kind === "resource_in_network") {
    objectAt(value, path, CONDITION_FIELDS, ["kind", "value"]);
    idAt(value.value, pointer(path, "value"), NETWORK_ID, "network id");
  } else {
    objectAt(value, path, CONDITION_FIELDS, ["kind"]);
  }
  return value as PermissionCondition;
}

function contributionVetoAt(
  value: unknown,
  path: string,
): NodeContributionVeto {
  const veto = objectAt(value, path, VETO_FIELDS, [
    "veto_id",
    "network_id",
    "contributions",
  ]);
  idAt(
    veto.veto_id,
    pointer(path, "veto_id"),
    SECURITY_RULE_ID,
    "security rule id",
  );
  idAt(veto.network_id, pointer(path, "network_id"), NETWORK_ID, "network id");
  arrayAt(veto.contributions, pointer(path, "contributions")).forEach(
    (contribution, index) => {
      const kind = stringAt(contribution, `${pointer(path, "contributions")}/${index}`, true);
      if (!CONTRIBUTION_KINDS.has(kind))
        fail(
          `${pointer(path, "contributions")}/${index}`,
          `unknown contribution kind \`${kind}\``,
        );
    },
  );
  return value as NodeContributionVeto;
}

function networkAclAt(value: unknown, path: string): NetworkAcl {
  const acl = objectAt(value, path, NETWORK_ACL_FIELDS, [
    "network_id",
    "revision",
    "issued_at_unix_ms",
    "expires_at_unix_ms",
  ]);
  idAt(acl.network_id, pointer(path, "network_id"), NETWORK_ID, "network id");
  uintAt(acl.revision, pointer(path, "revision"));
  uintAt(acl.issued_at_unix_ms, pointer(path, "issued_at_unix_ms"));
  uintAt(acl.expires_at_unix_ms, pointer(path, "expires_at_unix_ms"));
  if (acl.rules !== undefined)
    arrayAt(acl.rules, pointer(path, "rules")).forEach((rule, index) =>
      permissionRuleAt(rule, `${pointer(path, "rules")}/${index}`),
    );
  if (acl.grants !== undefined)
    arrayAt(acl.grants, pointer(path, "grants")).forEach((grant, index) =>
      networkAclGrantAt(grant, `${pointer(path, "grants")}/${index}`),
    );
  return value as NetworkAcl;
}

function networkAclGrantAt(value: unknown, path: string): NetworkAclGrant {
  const grant = objectAt(value, path, GRANT_FIELDS, [
    "grant_id",
    "subjects",
    "resources",
    "actions",
    "issued_at_unix_ms",
    "expires_at_unix_ms",
  ]);
  idAt(
    grant.grant_id,
    pointer(path, "grant_id"),
    SECURITY_RULE_ID,
    "security rule id",
  );
  arrayAt(grant.subjects, pointer(path, "subjects")).forEach((subject, index) =>
    subjectAt(subject, `${pointer(path, "subjects")}/${index}`),
  );
  arrayAt(grant.resources, pointer(path, "resources")).forEach(
    (resource, index) =>
      enumAt(
        resource,
        `${pointer(path, "resources")}/${index}`,
        RESOURCE_KINDS,
        "resource kind",
      ),
  );
  arrayAt(grant.actions, pointer(path, "actions")).forEach((action, index) =>
    enumAt(
      action,
      `${pointer(path, "actions")}/${index}`,
      POLICY_ACTIONS,
      "policy action",
    ),
  );
  uintAt(grant.issued_at_unix_ms, pointer(path, "issued_at_unix_ms"));
  uintAt(grant.expires_at_unix_ms, pointer(path, "expires_at_unix_ms"));
  if (grant.revoked_at_unix_ms !== undefined && grant.revoked_at_unix_ms !== null)
    uintAt(grant.revoked_at_unix_ms, pointer(path, "revoked_at_unix_ms"));
  optionalNullableId(
    grant.revoked_by,
    pointer(path, "revoked_by"),
    NODE_ID,
    "node id",
  );
  if (grant.requires_recent_reauthentication !== undefined)
    boolAt(
      grant.requires_recent_reauthentication,
      pointer(path, "requires_recent_reauthentication"),
    );
  return value as NetworkAclGrant;
}

function effectiveAccessAt(value: unknown, path: string): EffectiveAccess {
  const decision = objectAt(value, path, DECISION_FIELDS, [
    "allowed",
    "evaluated_at_unix_ms",
    "expired_grant_ids",
    "local_policy_revision",
    "matched_allow_rule_ids",
    "matched_deny_rule_ids",
    "network_acl_revision",
    "reason",
    "reauthentication_required_grant_ids",
    "reauthentication_satisfied",
    "revoked_grant_ids",
  ]);
  boolAt(decision.allowed, pointer(path, "allowed"));
  if (decision.code !== undefined && decision.code !== null)
    stringAt(decision.code, pointer(path, "code"));
  uintAt(
    decision.evaluated_at_unix_ms,
    pointer(path, "evaluated_at_unix_ms"),
  );
  for (const field of [
    "expired_grant_ids",
    "matched_allow_rule_ids",
    "matched_deny_rule_ids",
    "reauthentication_required_grant_ids",
    "revoked_grant_ids",
  ] as const) {
    arrayAt(decision[field], pointer(path, field)).forEach((id, index) =>
      idAt(
        id,
        `${pointer(path, field)}/${index}`,
        SECURITY_RULE_ID,
        "security rule id",
      ),
    );
  }
  if (decision.layer !== undefined && decision.layer !== null)
    enumAt(
      decision.layer,
      pointer(path, "layer"),
      POLICY_LAYERS,
      "policy layer",
    );
  uintAt(
    decision.local_policy_revision,
    pointer(path, "local_policy_revision"),
  );
  uintAt(
    decision.network_acl_revision,
    pointer(path, "network_acl_revision"),
  );
  enumAt(
    decision.reason,
    pointer(path, "reason"),
    ACCESS_REASONS,
    "access reason",
  );
  boolAt(
    decision.reauthentication_satisfied,
    pointer(path, "reauthentication_satisfied"),
  );
  optionalNullableId(
    decision.rule_id,
    pointer(path, "rule_id"),
    SECURITY_RULE_ID,
    "security rule id",
  );
  return value as EffectiveAccess;
}

function ruleExplanationAt(
  value: unknown,
  path: string,
): PolicyRuleExplanation {
  const explanation = objectAt(value, path, EXPLANATION_FIELDS, [
    "source",
    "source_path",
    "matched",
    "applied",
    "precedence",
  ]);
  enumAt(
    explanation.source,
    pointer(path, "source"),
    RULE_SOURCES,
    "rule source",
  );
  stringAt(explanation.source_path, pointer(path, "source_path"));
  optionalNullableId(
    explanation.rule_id,
    pointer(path, "rule_id"),
    SECURITY_RULE_ID,
    "security rule id",
  );
  if (explanation.effect !== undefined && explanation.effect !== null)
    enumAt(
      explanation.effect,
      pointer(path, "effect"),
      PERMISSION_EFFECTS,
      "permission effect",
    );
  if (explanation.subjects !== undefined)
    arrayAt(explanation.subjects, pointer(path, "subjects")).forEach(
      (subject, index) =>
        subjectAt(subject, `${pointer(path, "subjects")}/${index}`),
    );
  if (explanation.resources !== undefined)
    arrayAt(explanation.resources, pointer(path, "resources")).forEach(
      (resource, index) =>
        enumAt(
          resource,
          `${pointer(path, "resources")}/${index}`,
          RESOURCE_KINDS,
          "resource kind",
        ),
    );
  if (explanation.actions !== undefined)
    arrayAt(explanation.actions, pointer(path, "actions")).forEach(
      (action, index) =>
        enumAt(
          action,
          `${pointer(path, "actions")}/${index}`,
          POLICY_ACTIONS,
          "policy action",
        ),
    );
  if (explanation.conditions !== undefined)
    arrayAt(explanation.conditions, pointer(path, "conditions")).forEach(
      (condition, index) =>
        permissionConditionAt(
          condition,
          `${pointer(path, "conditions")}/${index}`,
        ),
    );
  boolAt(explanation.matched, pointer(path, "matched"));
  boolAt(explanation.applied, pointer(path, "applied"));
  enumAt(
    explanation.precedence,
    pointer(path, "precedence"),
    PRECEDENCE_VALUES,
    "policy precedence",
  );
  return value as PolicyRuleExplanation;
}

function precedenceExplanationAt(
  value: unknown,
  path: string,
): PolicyPrecedenceExplanation {
  const explanation = objectAt(
    value,
    path,
    PRECEDENCE_EXPLANATION_FIELDS,
    ["precedence", "triggered"],
  );
  enumAt(
    explanation.precedence,
    pointer(path, "precedence"),
    PRECEDENCE_VALUES,
    "policy precedence",
  );
  boolAt(explanation.triggered, pointer(path, "triggered"));
  if (explanation.source !== undefined && explanation.source !== null)
    enumAt(
      explanation.source,
      pointer(path, "source"),
      RULE_SOURCES,
      "rule source",
    );
  optionalNullableId(
    explanation.rule_id,
    pointer(path, "rule_id"),
    SECURITY_RULE_ID,
    "security rule id",
  );
  return value as PolicyPrecedenceExplanation;
}

function validationIssueAt(
  value: unknown,
  path: string,
): PolicyValidationIssue {
  const issue = objectAt(value, path, VALIDATION_ISSUE_FIELDS, [
    "path",
    "code",
    "source",
    "message",
  ]);
  stringAt(issue.path, pointer(path, "path"));
  stringAt(issue.code, pointer(path, "code"), true);
  enumAt(
    issue.source,
    pointer(path, "source"),
    VALIDATION_SOURCES,
    "validation source",
  );
  optionalNullableId(
    issue.related_rule_id,
    pointer(path, "related_rule_id"),
    SECURITY_RULE_ID,
    "security rule id",
  );
  stringAt(issue.message, pointer(path, "message"), true);
  return value as PolicyValidationIssue;
}

function duplicateObjectKeyPath(source: string): string | null {
  let position = 0;

  const skipWhitespace = (): void => {
    while (position < source.length && /\s/u.test(source[position])) position += 1;
  };

  const stringToken = (): string | null => {
    skipWhitespace();
    const start = position;
    if (source[position] !== '"') return null;
    position += 1;
    let escaped = false;
    while (position < source.length) {
      const character = source[position];
      position += 1;
      if (escaped) escaped = false;
      else if (character === "\\") escaped = true;
      else if (character === '"') {
        try {
          return JSON.parse(source.slice(start, position)) as string;
        } catch {
          return null;
        }
      }
    }
    return null;
  };

  const value = (path: string): string | null | undefined => {
    skipWhitespace();
    const character = source[position];
    if (character === "{") return object(path);
    if (character === "[") return array(path);
    if (character === '"') return stringToken() === null ? undefined : null;
    if (
      character === "t" ||
      character === "f" ||
      character === "n" ||
      character === "-" ||
      /[0-9]/u.test(character ?? "")
    ) {
      while (position < source.length) {
        const current = source[position];
        if (/\s/u.test(current) || current === "," || current === "]" || current === "}")
          break;
        position += 1;
      }
      return null;
    }
    return undefined;
  };

  const object = (path: string): string | null | undefined => {
    const keys = new Set<string>();
    position += 1;
    skipWhitespace();
    if (source[position] === "}") {
      position += 1;
      return null;
    }
    while (position < source.length) {
      const key = stringToken();
      if (key === null) return undefined;
      const child = pointer(path, key);
      if (keys.has(key)) return child;
      keys.add(key);
      skipWhitespace();
      if (source[position] !== ":") return undefined;
      position += 1;
      const issue = value(child);
      if (issue !== null) return issue;
      skipWhitespace();
      if (source[position] === ",") {
        position += 1;
        continue;
      }
      if (source[position] === "}") {
        position += 1;
        return null;
      }
      return undefined;
    }
    return undefined;
  };

  const array = (path: string): string | null | undefined => {
    position += 1;
    skipWhitespace();
    if (source[position] === "]") {
      position += 1;
      return null;
    }
    let index = 0;
    while (position < source.length) {
      const issue = value(`${path}/${index}`);
      if (issue !== null) return issue;
      skipWhitespace();
      if (source[position] === ",") {
        position += 1;
        index += 1;
        continue;
      }
      if (source[position] === "]") {
        position += 1;
        return null;
      }
      return undefined;
    }
    return undefined;
  };

  skipWhitespace();
  const issue = value("");
  if (issue !== null) return issue ?? null;
  skipWhitespace();
  return position === source.length ? null : null;
}
