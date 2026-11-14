import * as Dialog from "@radix-ui/react-dialog";
import {
  AlertTriangle,
  Braces,
  CheckCircle2,
  CircleSlash2,
  Clock3,
  FileJson,
  Gavel,
  GitBranch,
  GitCompare,
  History,
  ListChecks,
  LoaderCircle,
  Play,
  Plus,
  RefreshCw,
  RotateCcw,
  ShieldAlert,
  ShieldCheck,
  Trash2,
  Upload,
  X,
} from "lucide-react";
import {
  useEffect,
  useId,
  useRef,
  useState,
  type ChangeEvent,
  type JSX,
} from "react";
import type {
  NetworkAcl,
  NetworkAclGrant,
  NetworkRole,
  NodeCommandOutcome,
  NodeContributionVeto,
  NodeLocalAccessPolicy,
  NodeNetworkScope,
  NodeSnapshot,
  NodeUiAction,
  PermissionCondition,
  PermissionEffect,
  PermissionIntent,
  PermissionRule,
  PolicyAction,
  PolicyAffectedObject,
  PolicyApplyResult,
  PolicyCurrent,
  PolicyHistorySnapshot,
  PolicyPreview,
  PolicyRiskSummary,
  PolicyRollbackResult,
  PolicyRuleExplanation,
  PolicySnapshot,
  PolicyTestInput,
  PolicyTestResult,
  RelayLimitPolicy,
  ResourceKind,
  SubjectSelector,
} from "../../../contracts/typescript/node-contract-v1";
import {
  blockReasonText,
  executionBlockReason,
  label,
  outcomeSummary,
  problemText,
  type ActionValues,
} from "../contract/invariants";
import {
  parsePolicyTestInputJson,
  PolicyJsonError,
} from "../contract/policy-test";
import { tr } from "../i18n";
import { pageStateDefinition, type PageStateKind } from "../state-matrix";

export const POLICY_ACTION_IDS = new Set([
  "test_policy",
  "get_policy",
  "preview_policy",
  "preview_policy_rollback",
  "apply_policy",
  "rollback_policy",
  "get_policy_history",
]);

const NODE_ID = /^node_[A-Za-z0-9_-]{43}$/;
const NETWORK_ID = /^net_[A-Za-z0-9_-]{43}$/;

const RESOURCE_KINDS: ResourceKind[] = [
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
];
const POLICY_ACTIONS: PolicyAction[] = [
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
];
const NETWORK_ROLES: NetworkRole[] = [
  "owner",
  "administrator",
  "member",
  "temporary_coordinator",
];
const PERMISSION_INTENTS: PermissionIntent[] = [
  "read_only",
  "mutating",
  "governance_write",
];
const CONTRIBUTION_KINDS = ["relay", "exit", "file"] as const;

export type PolicyRuleDraft = {
  ruleId: string;
  subjectKind: SubjectSelector["kind"];
  subjectValue: string;
  resources: string;
  actions: string;
  effect: PermissionEffect;
  conditions: string;
};

export type PolicyGrantDraft = {
  grantId: string;
  subjectKind: SubjectSelector["kind"];
  subjectValue: string;
  resources: string;
  actions: string;
  issuedAt: string;
  expiresAt: string;
  revokedAt: string;
  revokedBy: string;
  requiresRecentReauthentication: boolean;
};

export type PolicyVetoDraft = {
  vetoId: string;
  contributions: string;
};

export type PolicyRelayLimitDraft = {
  maxConnections: string;
  maxConcurrentStreams: string;
  maxBandwidthKbps: string;
  maxBytes: string;
  maxCpuPercent: string;
  maxMemoryBytes: string;
  maxSessionDurationSecs: string;
  quotaResetAtUnixMs: string;
};

export type PolicyStructuredDraft = {
  networkId: string;
  actor: string;
  role: NetworkRole;
  groups: string;
  membershipActive: boolean;
  recentReauthentication: boolean;
  emergencyCoordinator: boolean;
  nowUnixMs: string;
  action: PolicyAction;
  resource: ResourceKind;
  resourceNetworkId: string;
  objectId: string;
  intent: PermissionIntent;
  includeLocalPolicy: boolean;
  localRevision: string;
  localIssuedAt: string;
  localExpiresAt: string;
  networkScopeKind: "all" | "only";
  networkIds: string;
  localRules: PolicyRuleDraft[];
  vetoes: PolicyVetoDraft[];
  localRelayLimits: PolicyRelayLimitDraft;
  includeNetworkAcl: boolean;
  aclRevision: string;
  aclIssuedAt: string;
  aclExpiresAt: string;
  aclRules: PolicyRuleDraft[];
  grants: PolicyGrantDraft[];
  aclRelayLimits: PolicyRelayLimitDraft;
};

export type PolicyDraftMemo = {
  structured: PolicyStructuredDraft;
  editor: string;
  mode: "structured" | "advanced";
  result: PolicyTestResult | null;
  current: PolicyCurrent | null;
  history: PolicyHistorySnapshot | null;
  preview: PolicyPreview | null;
  applied: PolicyApplyResult | PolicyRollbackResult | null;
  importName: string;
  activeIssuePath: string;
};

type PolicyWorkflowProps = {
  actions: NodeUiAction[];
  snapshot: NodeSnapshot;
  selectedNetwork: string;
  locale: Locale;
  pendingAction: string | null;
  interactionBlocked?: PageStateKind | null;
  draft?: PolicyDraftMemo | null;
  onDraftChange?(draft: PolicyDraftMemo): void;
  onSubmit(
    action: NodeUiAction,
    values: ActionValues,
  ): Promise<NodeCommandOutcome>;
};

type EditorMode = PolicyDraftMemo["mode"];
type BusyState = "load" | "history" | "preview" | "apply" | "rollback" | null;

function generatedId(prefix: string, index: number): string {
  const seed = `${Date.now().toString(36)}${index.toString(36)}${Math.random()
    .toString(36)
    .slice(2)}`.replace(/[^A-Za-z0-9_-]/g, "");
  return `${prefix}${seed.slice(0, 43).padEnd(43, "A")}`;
}

function emptyRule(index: number, effect: PermissionEffect): PolicyRuleDraft {
  return {
    ruleId: generatedId("srule_", index),
    subjectKind: "any_member",
    subjectValue: "",
    resources: "network",
    actions: "read",
    effect,
    conditions: "",
  };
}

function emptyGrant(index: number, now: number): PolicyGrantDraft {
  return {
    grantId: generatedId("srule_", index),
    subjectKind: "any_member",
    subjectValue: "",
    resources: "network",
    actions: "read",
    issuedAt: String(now),
    expiresAt: String(now + 86_400_000),
    revokedAt: "",
    revokedBy: "",
    requiresRecentReauthentication: false,
  };
}

function emptyVeto(index: number): PolicyVetoDraft {
  return {
    vetoId: generatedId("srule_", index),
    contributions: "relay",
  };
}

function emptyRelayLimits(): PolicyRelayLimitDraft {
  return {
    maxConnections: "",
    maxConcurrentStreams: "",
    maxBandwidthKbps: "",
    maxBytes: "",
    maxCpuPercent: "",
    maxMemoryBytes: "",
    maxSessionDurationSecs: "",
    quotaResetAtUnixMs: "",
  };
}

function validId(value: string | null | undefined, pattern: RegExp): string {
  return value && pattern.test(value) ? value : "";
}

function networkIdForSnapshot(
  snapshot: NodeSnapshot,
  selectedNetwork: string,
): string {
  const membership = snapshot.memberships?.find(
    (entry) => entry.network_name === selectedNetwork,
  );
  if (membership && NETWORK_ID.test(membership.network_id)) {
    return membership.network_id;
  }
  const profile = snapshot.network_profiles?.find(
    (entry) => entry.name === selectedNetwork,
  );
  return profile && NETWORK_ID.test(profile.network_id)
    ? profile.network_id
    : "";
}

function defaultStructuredDraft(
  snapshot: NodeSnapshot,
  selectedNetwork: string,
): PolicyStructuredDraft {
  const membership = snapshot.memberships?.find(
    (entry) => entry.network_name === selectedNetwork,
  );
  const now = Date.now();
  const actor =
    validId(membership?.node_id, NODE_ID) ||
    validId(snapshot.identity?.node_id, NODE_ID);
  const role =
    membership?.role === "owner" ||
    membership?.role === "administrator" ||
    membership?.role === "member"
      ? membership.role
      : "member";
  const networkId = networkIdForSnapshot(snapshot, selectedNetwork);
  return {
    networkId,
    actor,
    role,
    groups: "",
    membershipActive: true,
    recentReauthentication: true,
    emergencyCoordinator: false,
    nowUnixMs: String(now),
    action: "read",
    resource: "network",
    resourceNetworkId: networkId,
    objectId: "",
    intent: "read_only",
    includeLocalPolicy: false,
    localRevision: String(membership?.local_policy_revision ?? 1),
    localIssuedAt: String(now),
    localExpiresAt: String(now + 86_400_000),
    networkScopeKind: "all",
    networkIds: networkId,
    localRules: [],
    vetoes: [],
    localRelayLimits: emptyRelayLimits(),
    includeNetworkAcl: false,
    aclRevision: "1",
    aclIssuedAt: String(now),
    aclExpiresAt: String(now + 86_400_000),
    aclRules: [],
    grants: [],
    aclRelayLimits: emptyRelayLimits(),
  };
}

function restoredStructuredDraft(
  draft: PolicyStructuredDraft | undefined,
  fallback: PolicyStructuredDraft,
): PolicyStructuredDraft {
  if (!draft) return fallback;
  const localRelayLimits = draft.localRelayLimits;
  const aclRelayLimits = draft.aclRelayLimits;
  return {
    ...fallback,
    ...draft,
    localRelayLimits: {
      ...fallback.localRelayLimits,
      ...(localRelayLimits ?? {}),
    },
    aclRelayLimits: {
      ...fallback.aclRelayLimits,
      ...(aclRelayLimits ?? {}),
    },
  };
}

function csv(value: string): string[] {
  return value
    .split(",")
    .map((entry) => entry.trim())
    .filter(Boolean);
}

function subjectValue(draft: PolicyRuleDraft | PolicyGrantDraft): string {
  return draft.subjectValue.trim();
}

function subjectFromDraft(
  draft: PolicyRuleDraft | PolicyGrantDraft,
): SubjectSelector {
  if (draft.subjectKind === "any_member") return { kind: "any_member" };
  return {
    kind: draft.subjectKind,
    value: subjectValue(draft),
  } as SubjectSelector;
}

function conditionFromToken(token: string): PermissionCondition {
  const separator = token.indexOf(":");
  if (separator < 0) return { kind: token } as PermissionCondition;
  return {
    kind: token.slice(0, separator),
    value: token.slice(separator + 1),
  } as PermissionCondition;
}

function ruleFromDraft(draft: PolicyRuleDraft): PermissionRule {
  return {
    rule_id: draft.ruleId.trim(),
    subjects: [subjectFromDraft(draft)],
    resources: csv(draft.resources) as ResourceKind[],
    actions: csv(draft.actions) as PolicyAction[],
    effect: draft.effect,
    conditions: csv(draft.conditions).map(conditionFromToken),
  };
}

function grantFromDraft(draft: PolicyGrantDraft): NetworkAclGrant {
  return {
    grant_id: draft.grantId.trim(),
    subjects: [subjectFromDraft(draft)],
    resources: csv(draft.resources) as ResourceKind[],
    actions: csv(draft.actions) as PolicyAction[],
    issued_at_unix_ms: Number(draft.issuedAt),
    expires_at_unix_ms: Number(draft.expiresAt),
    ...(draft.revokedAt.trim()
      ? { revoked_at_unix_ms: Number(draft.revokedAt) }
      : {}),
    ...(draft.revokedBy.trim() ? { revoked_by: draft.revokedBy.trim() } : {}),
    ...(draft.requiresRecentReauthentication
      ? { requires_recent_reauthentication: true }
      : {}),
  };
}

function scopeFromDraft(draft: PolicyStructuredDraft): NodeNetworkScope {
  return draft.networkScopeKind === "all"
    ? { kind: "all" }
    : { kind: "only", network_ids: csv(draft.networkIds) as never[] };
}

function optionalLimit(value: string): number | undefined {
  const trimmed = value.trim();
  return trimmed ? Number(trimmed) : undefined;
}

function relayLimitsFromDraft(
  draft: PolicyRelayLimitDraft,
): RelayLimitPolicy {
  const maxConnections = optionalLimit(draft.maxConnections);
  const maxConcurrentStreams = optionalLimit(draft.maxConcurrentStreams);
  const maxBandwidthKbps = optionalLimit(draft.maxBandwidthKbps);
  const maxBytes = optionalLimit(draft.maxBytes);
  const maxCpuPercent = optionalLimit(draft.maxCpuPercent);
  const maxMemoryBytes = optionalLimit(draft.maxMemoryBytes);
  const maxSessionDurationSecs = optionalLimit(draft.maxSessionDurationSecs);
  const quotaResetAtUnixMs = optionalLimit(draft.quotaResetAtUnixMs);
  return {
    ...(maxConnections !== undefined ? { max_connections: maxConnections } : {}),
    ...(maxConcurrentStreams !== undefined
      ? { max_concurrent_streams: maxConcurrentStreams }
      : {}),
    ...(maxBandwidthKbps !== undefined
      ? { max_bandwidth_kbps: maxBandwidthKbps }
      : {}),
    ...(maxBytes !== undefined ? { max_bytes: maxBytes } : {}),
    ...(maxCpuPercent !== undefined ? { max_cpu_percent: maxCpuPercent } : {}),
    ...(maxMemoryBytes !== undefined
      ? { max_memory_bytes: maxMemoryBytes }
      : {}),
    ...(maxSessionDurationSecs !== undefined
      ? { max_session_duration_secs: maxSessionDurationSecs }
      : {}),
    ...(quotaResetAtUnixMs !== undefined
      ? { quota_reset_at_unix_ms: quotaResetAtUnixMs }
      : {}),
  };
}

function optionalLimitString(value: number | null | undefined): string {
  return value === null || value === undefined ? "" : String(value);
}

function relayLimitsFromPolicy(
  limits: RelayLimitPolicy | undefined,
): PolicyRelayLimitDraft {
  return {
    maxConnections: optionalLimitString(limits?.max_connections),
    maxConcurrentStreams: optionalLimitString(limits?.max_concurrent_streams),
    maxBandwidthKbps: optionalLimitString(limits?.max_bandwidth_kbps),
    maxBytes: optionalLimitString(limits?.max_bytes),
    maxCpuPercent: optionalLimitString(limits?.max_cpu_percent),
    maxMemoryBytes: optionalLimitString(limits?.max_memory_bytes),
    maxSessionDurationSecs: optionalLimitString(
      limits?.max_session_duration_secs,
    ),
    quotaResetAtUnixMs: optionalLimitString(limits?.quota_reset_at_unix_ms),
  };
}

export function structuredPolicyInput(
  draft: PolicyStructuredDraft,
): PolicyTestInput {
  const localPolicy: NodeLocalAccessPolicy | undefined =
    draft.includeLocalPolicy
      ? {
          node_id: draft.actor.trim(),
          revision: Number(draft.localRevision),
          issued_at_unix_ms: Number(draft.localIssuedAt),
          expires_at_unix_ms: Number(draft.localExpiresAt),
          network_scope: scopeFromDraft(draft),
          rules: draft.localRules.map(ruleFromDraft),
          contribution_vetoes: draft.vetoes.map((veto) => ({
            veto_id: veto.vetoId.trim(),
            network_id: draft.networkId.trim(),
            contributions: csv(
              veto.contributions,
            ) as NodeContributionVeto["contributions"],
          })),
          relay_limits: relayLimitsFromDraft(draft.localRelayLimits),
        }
      : undefined;
  const networkAcl: NetworkAcl | undefined = draft.includeNetworkAcl
    ? {
        network_id: draft.networkId.trim(),
        revision: Number(draft.aclRevision),
        issued_at_unix_ms: Number(draft.aclIssuedAt),
        expires_at_unix_ms: Number(draft.aclExpiresAt),
        rules: draft.aclRules.map(ruleFromDraft),
        grants: draft.grants.map(grantFromDraft),
        relay_limits: relayLimitsFromDraft(draft.aclRelayLimits),
      }
    : undefined;
  return {
    network_id: draft.networkId.trim(),
    actor: draft.actor.trim(),
    role: draft.role,
    groups: csv(draft.groups) as never[],
    membership_active: draft.membershipActive,
    recent_reauthentication: draft.recentReauthentication,
    emergency_coordinator: draft.emergencyCoordinator,
    request: {
      action: draft.action,
      resource: draft.resource,
      resource_network_id: draft.resourceNetworkId.trim(),
      ...(draft.objectId.trim() ? { object_id: draft.objectId.trim() } : {}),
      intent: draft.intent,
    },
    ...(localPolicy ? { local_policy: localPolicy } : {}),
    ...(networkAcl ? { network_acl: networkAcl } : {}),
    now_unix_ms: Number(draft.nowUnixMs),
  };
}

function encodedSubject(subject: SubjectSelector | undefined): string {
  if (!subject) return "any_member";
  if (subject.kind === "any_member") return "any_member";
  return `${subject.kind}:${subject.value}`;
}

function encodedCondition(condition: PermissionCondition): string {
  if (condition.kind === "resource_in_network") {
    return `${condition.kind}:${condition.value}`;
  }
  return condition.kind;
}

function ruleDraftFromRule(rule: PermissionRule): PolicyRuleDraft {
  const subject = rule.subjects[0];
  return {
    ruleId: rule.rule_id,
    subjectKind: subject?.kind ?? "any_member",
    subjectValue: subject && "value" in subject ? String(subject.value) : "",
    resources: rule.resources.join(","),
    actions: rule.actions.join(","),
    effect: rule.effect,
    conditions: (rule.conditions ?? []).map(encodedCondition).join(","),
  };
}

function grantDraftFromGrant(grant: NetworkAclGrant): PolicyGrantDraft {
  const subject = grant.subjects[0];
  return {
    grantId: grant.grant_id,
    subjectKind: subject?.kind ?? "any_member",
    subjectValue: subject && "value" in subject ? String(subject.value) : "",
    resources: grant.resources.join(","),
    actions: grant.actions.join(","),
    issuedAt: String(grant.issued_at_unix_ms),
    expiresAt: String(grant.expires_at_unix_ms),
    revokedAt:
      grant.revoked_at_unix_ms === undefined ||
      grant.revoked_at_unix_ms === null
        ? ""
        : String(grant.revoked_at_unix_ms),
    revokedBy: grant.revoked_by ?? "",
    requiresRecentReauthentication:
      grant.requires_recent_reauthentication ?? false,
  };
}

export function structuredPolicySnapshot(
  draft: PolicyStructuredDraft,
): PolicySnapshot {
  const input = structuredPolicyInput(draft);
  return {
    ...(input.local_policy ? { local_policy: input.local_policy } : {}),
    ...(input.network_acl ? { network_acls: [input.network_acl] } : {}),
  };
}

export function structuredDraftFromPolicyInput(
  input: PolicyTestInput,
  fallback: PolicyStructuredDraft,
): PolicyStructuredDraft {
  const localPolicy = input.local_policy;
  const networkAcl = input.network_acl;
  return {
    ...fallback,
    networkId: input.network_id,
    actor: input.actor,
    role: input.role,
    groups: input.groups?.join(",") ?? "",
    membershipActive: input.membership_active,
    recentReauthentication: input.recent_reauthentication,
    emergencyCoordinator: input.emergency_coordinator,
    nowUnixMs: String(input.now_unix_ms),
    action: input.request.action,
    resource: input.request.resource,
    resourceNetworkId: input.request.resource_network_id,
    objectId: input.request.object_id ?? "",
    intent: input.request.intent,
    includeLocalPolicy: Boolean(localPolicy),
    localRevision: String(localPolicy?.revision ?? fallback.localRevision),
    localIssuedAt: String(
      localPolicy?.issued_at_unix_ms ?? fallback.localIssuedAt,
    ),
    localExpiresAt: String(
      localPolicy?.expires_at_unix_ms ?? fallback.localExpiresAt,
    ),
    networkScopeKind:
      localPolicy?.network_scope.kind ?? fallback.networkScopeKind,
    networkIds:
      localPolicy?.network_scope.kind === "only"
        ? localPolicy.network_scope.network_ids.join(",")
        : fallback.networkIds,
    localRules: localPolicy?.rules?.map(ruleDraftFromRule) ?? [],
    vetoes:
      localPolicy?.contribution_vetoes?.map((veto) => ({
        vetoId: veto.veto_id,
        contributions: veto.contributions.join(","),
      })) ?? [],
    localRelayLimits: relayLimitsFromPolicy(localPolicy?.relay_limits),
    includeNetworkAcl: Boolean(networkAcl),
    aclRevision: String(networkAcl?.revision ?? fallback.aclRevision),
    aclIssuedAt: String(networkAcl?.issued_at_unix_ms ?? fallback.aclIssuedAt),
    aclExpiresAt: String(
      networkAcl?.expires_at_unix_ms ?? fallback.aclExpiresAt,
    ),
    aclRules: networkAcl?.rules?.map(ruleDraftFromRule) ?? [],
    grants: networkAcl?.grants?.map(grantDraftFromGrant) ?? [],
    aclRelayLimits: relayLimitsFromPolicy(networkAcl?.relay_limits),
  };
}

function prettyPolicy(input: PolicyTestInput): string {
  return JSON.stringify(input, null, 2);
}

function rejectedMessage(
  outcome: NodeCommandOutcome,
  locale: Locale,
): string | null {
  return outcome.status === "rejected"
    ? problemText(outcome.data, locale)
    : null;
}

function errorMessage(error: unknown, locale: Locale): string {
  if (error instanceof PolicyJsonError) {
    return tr(locale, "validation.policy_json_path", {
      path: error.path,
      message: error.message,
    });
  }
  return error instanceof Error ? error.message : String(error);
}

function jsonPointerSelection(
  source: string,
  pointer: string,
): { start: number; end: number } {
  if (!pointer.startsWith("/") || pointer === "/") {
    return { start: 0, end: 0 };
  }
  const segments = pointer
    .slice(1)
    .split("/")
    .map((segment) => segment.replaceAll("~1", "/").replaceAll("~0", "~"));
  let start = 0;
  let end = 0;
  for (const segment of segments) {
    const token = JSON.stringify(segment);
    const found = source.indexOf(token, start);
    if (found < 0) continue;
    start = found;
    end = found + token.length;
  }
  return { start, end };
}

function choice(locale: Locale, namespace: string, value: string): string {
  return label(locale, `${namespace}.${value}`);
}

function formatTimestamp(locale: Locale, unixMs: number): string {
  return (
    window.FuckIPV6I18n?.formatTimestamp(locale, unixMs) ??
    new Date(unixMs).toLocaleString(locale)
  );
}

function valueList(
  locale: Locale,
  namespace: string,
  values: readonly string[] | undefined,
): string {
  return values && values.length
    ? values.map((value) => choice(locale, namespace, value)).join(", ")
    : label(locale, "policy.result.none");
}

function grantList(
  locale: Locale,
  values: readonly string[] | undefined,
): string {
  return values && values.length
    ? values.join(", ")
    : label(locale, "policy.result.none");
}

function timestampText(locale: Locale, value: string): string {
  const timestamp = new Date(value);
  return Number.isNaN(timestamp.getTime())
    ? value
    : timestamp.toLocaleString(locale);
}

function riskLevelText(locale: Locale, risk: PolicyRiskSummary): string {
  return label(locale, `policy.risk.level.${risk.level}`);
}

function riskReasonText(locale: Locale, reason: string): string {
  return label(locale, `policy.risk.reason.${reason}`);
}

function diffChangeText(locale: Locale, change: string): string {
  return label(locale, `policy.diff.change.${change}`);
}

function affectedObjectText(
  locale: Locale,
  object: PolicyAffectedObject,
): string {
  const objectLabel = label(locale, `policy.object.kind.${object.kind}`);
  const identifiers = [object.network_id, object.object_id].filter(
    (value): value is string => Boolean(value),
  );
  return identifiers.length
    ? `${objectLabel} · ${identifiers.join(" · ")}`
    : objectLabel;
}

function PolicyDangerConfirmationFacts({
  locale,
  preview,
  rollback,
}: {
  locale: Locale;
  preview: PolicyPreview;
  rollback: boolean;
}): JSX.Element {
  const separator = label(locale, "common.list_separator");
  const networks = preview.network_ids.length
    ? preview.network_ids.join(separator)
    : label(locale, "common.none");
  return (
    <ol className="danger-confirm-facts">
      <li data-confirm-fact="scope">
        <strong>{label(locale, "confirm.policy.scope")}</strong>
        <code>{networks}</code>
        <span>
          {label(locale, "policy.preview.includes_local")}:{" "}
          {preview.includes_local_policy
            ? label(locale, "policy.yes")
            : label(locale, "policy.no")}
        </span>
        <span>
          {tr(locale, "confirm.policy.affected_objects", {
            count: preview.affected_objects.length,
          })}
        </span>
      </li>
      <li data-confirm-fact="revision">
        {rollback
          ? tr(locale, "confirm.policy.target_revision", {
              revision: preview.rollback_target_revision ?? 0,
            })
          : tr(locale, "confirm.policy.expected_revision", {
              revision: preview.expected_revision,
            })}
      </li>
      <li data-confirm-fact="duration">
        {label(locale, "confirm.policy.duration")}
      </li>
      <li data-confirm-fact="undo">{label(locale, "confirm.policy.undo")}</li>
      <li data-confirm-fact="failure">
        {label(locale, "confirm.policy.failure")}
      </li>
      <li data-confirm-fact="audit">
        {label(locale, "confirm.policy.audit")}
      </li>
    </ol>
  );
}

function policyJson(value: unknown): string {
  return JSON.stringify(value, null, 2);
}

function structuredSnapshotDraft(
  snapshot: PolicySnapshot,
  fallback: PolicyStructuredDraft,
): PolicyStructuredDraft {
  const acl =
    snapshot.network_acls?.find(
      (entry) => entry.network_id === fallback.networkId,
    ) ?? snapshot.network_acls?.[0];
  const input = structuredPolicyInput(fallback);
  const next = structuredDraftFromPolicyInput(
    {
      ...input,
      local_policy: snapshot.local_policy ?? null,
      network_acl: acl ?? null,
    },
    fallback,
  );
  return {
    ...next,
    includeLocalPolicy: Boolean(snapshot.local_policy),
    includeNetworkAcl: Boolean(acl),
    networkId: acl?.network_id ?? next.networkId,
    resourceNetworkId:
      acl?.network_id ?? next.resourceNetworkId ?? next.networkId,
  };
}

type FieldProps = {
  id: string;
  labelText: string;
  children: JSX.Element;
  help?: string;
};

function Field({ id, labelText, children, help }: FieldProps): JSX.Element {
  return (
    <div className="policy-field">
      <label htmlFor={id}>{labelText}</label>
      {children}
      {help ? <small>{help}</small> : null}
    </div>
  );
}

const RELAY_LIMIT_FIELDS: Array<{
  draft: keyof PolicyRelayLimitDraft;
  field: keyof RelayLimitPolicy;
}> = [
  { draft: "maxConnections", field: "max_connections" },
  { draft: "maxConcurrentStreams", field: "max_concurrent_streams" },
  { draft: "maxBandwidthKbps", field: "max_bandwidth_kbps" },
  { draft: "maxBytes", field: "max_bytes" },
  { draft: "maxCpuPercent", field: "max_cpu_percent" },
  { draft: "maxMemoryBytes", field: "max_memory_bytes" },
  { draft: "maxSessionDurationSecs", field: "max_session_duration_secs" },
  { draft: "quotaResetAtUnixMs", field: "quota_reset_at_unix_ms" },
];

function RelayLimitsEditor({
  idPrefix,
  limits,
  locale,
  onChange,
}: {
  idPrefix: string;
  limits: PolicyRelayLimitDraft;
  locale: Locale;
  onChange(limits: PolicyRelayLimitDraft): void;
}): JSX.Element {
  return (
    <div
      className="policy-context-grid policy-metadata-grid"
      data-testid={idPrefix}
    >
      {RELAY_LIMIT_FIELDS.map(({ draft, field }) => {
        const id = `${idPrefix}-${field.replaceAll("_", "-")}`;
        return (
          <Field
            key={field}
            id={id}
            labelText={label(locale, `relay.limits.${field}`)}
          >
            <input
              id={id}
              inputMode="numeric"
              placeholder={label(locale, "relay.limit.none")}
              value={limits[draft]}
              onChange={(event) =>
                onChange({ ...limits, [draft]: event.target.value })
              }
            />
          </Field>
        );
      })}
    </div>
  );
}

function RuleEditor({
  rule,
  index,
  locale,
  source,
  onChange,
  onRemove,
}: {
  rule: PolicyRuleDraft;
  index: number;
  locale: Locale;
  source: "local" | "acl";
  onChange(rule: PolicyRuleDraft): void;
  onRemove(): void;
}): JSX.Element {
  const prefix = `${source}-rule-${index}`;
  return (
    <fieldset className="policy-rule-card">
      <legend>
        <GitBranch size={14} />
        {tr(locale, "policy.rule.title", { index: index + 1 })}
        <span>
          {label(
            locale,
            source === "local"
              ? "policy.choice.source.local_policy_rule"
              : "policy.choice.source.network_acl_rule",
          )}
        </span>
      </legend>
      <button
        className="policy-remove-button"
        type="button"
        aria-label={`${label(locale, "policy.remove")} ${index + 1}`}
        onClick={onRemove}
      >
        <Trash2 size={14} />
      </button>
      <div className="policy-rule-grid">
        <Field
          id={`${prefix}-id`}
          labelText={label(locale, "policy.field.rule_id")}
        >
          <input
            id={`${prefix}-id`}
            value={rule.ruleId}
            spellCheck={false}
            onChange={(event) =>
              onChange({ ...rule, ruleId: event.target.value })
            }
          />
        </Field>
        <Field
          id={`${prefix}-effect`}
          labelText={label(locale, "policy.field.effect")}
        >
          <select
            id={`${prefix}-effect`}
            value={rule.effect}
            onChange={(event) =>
              onChange({
                ...rule,
                effect: event.target.value as PermissionEffect,
              })
            }
          >
            {(["allow", "deny"] as const).map((effect) => (
              <option key={effect} value={effect}>
                {choice(locale, "policy.choice.effect", effect)}
              </option>
            ))}
          </select>
        </Field>
        <Field
          id={`${prefix}-subject-kind`}
          labelText={label(locale, "policy.field.subject_kind")}
        >
          <select
            id={`${prefix}-subject-kind`}
            value={rule.subjectKind}
            onChange={(event) =>
              onChange({
                ...rule,
                subjectKind: event.target.value as SubjectSelector["kind"],
              })
            }
          >
            {(["any_member", "role", "node", "group"] as const).map((kind) => (
              <option key={kind} value={kind}>
                {choice(locale, "policy.choice.subject", kind)}
              </option>
            ))}
          </select>
        </Field>
        <Field
          id={`${prefix}-subject-value`}
          labelText={label(locale, "policy.field.subject_value")}
        >
          <input
            id={`${prefix}-subject-value`}
            value={rule.subjectValue}
            disabled={rule.subjectKind === "any_member"}
            placeholder={
              rule.subjectKind === "role"
                ? "member"
                : rule.subjectKind === "node"
                  ? "node_..."
                  : rule.subjectKind === "group"
                    ? "group_..."
                    : "—"
            }
            onChange={(event) =>
              onChange({ ...rule, subjectValue: event.target.value })
            }
          />
        </Field>
        <Field
          id={`${prefix}-resources`}
          labelText={label(locale, "policy.field.resources")}
        >
          <input
            id={`${prefix}-resources`}
            value={rule.resources}
            placeholder={label(locale, "policy.placeholder.csv")}
            onChange={(event) =>
              onChange({ ...rule, resources: event.target.value })
            }
          />
        </Field>
        <Field
          id={`${prefix}-actions`}
          labelText={label(locale, "policy.field.actions")}
        >
          <input
            id={`${prefix}-actions`}
            value={rule.actions}
            placeholder={label(locale, "policy.placeholder.actions")}
            onChange={(event) =>
              onChange({ ...rule, actions: event.target.value })
            }
          />
        </Field>
        <Field
          id={`${prefix}-conditions`}
          labelText={label(locale, "policy.field.conditions")}
        >
          <input
            id={`${prefix}-conditions`}
            value={rule.conditions}
            placeholder="membership_active,resource_in_network:net_..."
            onChange={(event) =>
              onChange({ ...rule, conditions: event.target.value })
            }
          />
        </Field>
      </div>
    </fieldset>
  );
}

function GrantEditor({
  grant,
  index,
  locale,
  onChange,
  onRemove,
}: {
  grant: PolicyGrantDraft;
  index: number;
  locale: Locale;
  onChange(grant: PolicyGrantDraft): void;
  onRemove(): void;
}): JSX.Element {
  const prefix = `grant-${index}`;
  return (
    <fieldset className="policy-rule-card policy-grant-card">
      <legend>
        <Gavel size={14} />
        {tr(locale, "policy.grant.title", { index: index + 1 })}
      </legend>
      <button
        className="policy-remove-button"
        type="button"
        aria-label={`${label(locale, "policy.remove")} grant ${index + 1}`}
        onClick={onRemove}
      >
        <Trash2 size={14} />
      </button>
      <div className="policy-rule-grid">
        <Field
          id={`${prefix}-id`}
          labelText={label(locale, "policy.field.grant_id")}
        >
          <input
            id={`${prefix}-id`}
            value={grant.grantId}
            spellCheck={false}
            onChange={(event) =>
              onChange({ ...grant, grantId: event.target.value })
            }
          />
        </Field>
        <Field
          id={`${prefix}-subject-kind`}
          labelText={label(locale, "policy.field.subject_kind")}
        >
          <select
            id={`${prefix}-subject-kind`}
            value={grant.subjectKind}
            onChange={(event) =>
              onChange({
                ...grant,
                subjectKind: event.target.value as SubjectSelector["kind"],
              })
            }
          >
            {(["any_member", "role", "node", "group"] as const).map((kind) => (
              <option key={kind} value={kind}>
                {choice(locale, "policy.choice.subject", kind)}
              </option>
            ))}
          </select>
        </Field>
        <Field
          id={`${prefix}-subject-value`}
          labelText={label(locale, "policy.field.subject_value")}
        >
          <input
            id={`${prefix}-subject-value`}
            value={grant.subjectValue}
            disabled={grant.subjectKind === "any_member"}
            onChange={(event) =>
              onChange({ ...grant, subjectValue: event.target.value })
            }
          />
        </Field>
        <Field
          id={`${prefix}-resources`}
          labelText={label(locale, "policy.field.resources")}
        >
          <input
            id={`${prefix}-resources`}
            value={grant.resources}
            onChange={(event) =>
              onChange({ ...grant, resources: event.target.value })
            }
          />
        </Field>
        <Field
          id={`${prefix}-actions`}
          labelText={label(locale, "policy.field.actions")}
        >
          <input
            id={`${prefix}-actions`}
            value={grant.actions}
            onChange={(event) =>
              onChange({ ...grant, actions: event.target.value })
            }
          />
        </Field>
        <Field
          id={`${prefix}-issued`}
          labelText={label(locale, "policy.field.issued_at_unix_ms")}
        >
          <input
            id={`${prefix}-issued`}
            inputMode="numeric"
            value={grant.issuedAt}
            onChange={(event) =>
              onChange({ ...grant, issuedAt: event.target.value })
            }
          />
        </Field>
        <Field
          id={`${prefix}-expires`}
          labelText={label(locale, "policy.field.expires_at_unix_ms")}
        >
          <input
            id={`${prefix}-expires`}
            inputMode="numeric"
            value={grant.expiresAt}
            onChange={(event) =>
              onChange({ ...grant, expiresAt: event.target.value })
            }
          />
        </Field>
        <Field
          id={`${prefix}-revoked`}
          labelText={label(locale, "policy.field.revoked_at_unix_ms")}
        >
          <input
            id={`${prefix}-revoked`}
            inputMode="numeric"
            value={grant.revokedAt}
            onChange={(event) =>
              onChange({ ...grant, revokedAt: event.target.value })
            }
          />
        </Field>
        <Field
          id={`${prefix}-revoked-by`}
          labelText={label(locale, "policy.field.revoked_by")}
        >
          <input
            id={`${prefix}-revoked-by`}
            value={grant.revokedBy}
            placeholder="node_..."
            onChange={(event) =>
              onChange({ ...grant, revokedBy: event.target.value })
            }
          />
        </Field>
      </div>
      <label className="policy-check">
        <input
          type="checkbox"
          checked={grant.requiresRecentReauthentication}
          onChange={(event) =>
            onChange({
              ...grant,
              requiresRecentReauthentication: event.target.checked,
            })
          }
        />
        <span>
          {label(locale, "policy.field.requires_recent_authentication")}
        </span>
      </label>
    </fieldset>
  );
}

function VetoEditor({
  veto,
  index,
  locale,
  onChange,
  onRemove,
}: {
  veto: PolicyVetoDraft;
  index: number;
  locale: Locale;
  onChange(veto: PolicyVetoDraft): void;
  onRemove(): void;
}): JSX.Element {
  const prefix = `veto-${index}`;
  return (
    <fieldset className="policy-rule-card policy-veto-card">
      <legend>
        <CircleSlash2 size={14} />
        {tr(locale, "policy.veto.title", { index: index + 1 })}
      </legend>
      <button
        className="policy-remove-button"
        type="button"
        aria-label={`${label(locale, "policy.remove")} veto ${index + 1}`}
        onClick={onRemove}
      >
        <Trash2 size={14} />
      </button>
      <div className="policy-rule-grid policy-veto-grid">
        <Field
          id={`${prefix}-id`}
          labelText={label(locale, "policy.field.veto_id")}
        >
          <input
            id={`${prefix}-id`}
            value={veto.vetoId}
            spellCheck={false}
            onChange={(event) =>
              onChange({ ...veto, vetoId: event.target.value })
            }
          />
        </Field>
        <Field
          id={`${prefix}-contributions`}
          labelText={label(locale, "policy.field.contributions")}
        >
          <input
            id={`${prefix}-contributions`}
            value={veto.contributions}
            placeholder={CONTRIBUTION_KINDS.join(",")}
            onChange={(event) =>
              onChange({ ...veto, contributions: event.target.value })
            }
          />
        </Field>
      </div>
    </fieldset>
  );
}

function ExplanationCard({
  explanation,
  locale,
  onLocate,
}: {
  explanation: PolicyRuleExplanation;
  locale: Locale;
  onLocate(path: string): void;
}): JSX.Element {
  return (
    <article
      className={`policy-explanation${explanation.applied ? " applied" : ""}`}
    >
      <div className="policy-explanation-heading">
        <div>
          <strong>
            {choice(locale, "policy.choice.source", explanation.source)}
          </strong>
          <span>
            {choice(locale, "policy.choice.precedence", explanation.precedence)}
          </span>
        </div>
        <div className="policy-explanation-state">
          <span className={explanation.matched ? "matched" : ""}>
            {label(
              locale,
              explanation.matched
                ? "policy.explain.matched"
                : "policy.explain.not_matched",
            )}
          </span>
          <span className={explanation.applied ? "applied" : ""}>
            {label(
              locale,
              explanation.applied
                ? "policy.explain.applied"
                : "policy.explain.not_applied",
            )}
          </span>
        </div>
      </div>
      <dl className="policy-explanation-meta">
        <div>
          <dt>{label(locale, "policy.explain.path")}</dt>
          <dd>
            <button
              type="button"
              onClick={() => onLocate(explanation.source_path)}
            >
              <code>{explanation.source_path}</code>
            </button>
          </dd>
        </div>
        <div>
          <dt>{label(locale, "policy.explain.rule")}</dt>
          <dd>
            <code>{explanation.rule_id ?? "—"}</code>
          </dd>
        </div>
        <div>
          <dt>{label(locale, "policy.explain.effect")}</dt>
          <dd>
            {explanation.effect
              ? choice(locale, "policy.choice.effect", explanation.effect)
              : "—"}
          </dd>
        </div>
        <div>
          <dt>{label(locale, "policy.explain.subjects")}</dt>
          <dd>
            {valueList(
              locale,
              "policy.choice.subject",
              explanation.subjects?.map(encodedSubject),
            )}
          </dd>
        </div>
        <div>
          <dt>{label(locale, "policy.explain.resources")}</dt>
          <dd>
            {valueList(locale, "policy.choice.resource", explanation.resources)}
          </dd>
        </div>
        <div>
          <dt>{label(locale, "policy.explain.actions")}</dt>
          <dd>
            {valueList(locale, "policy.choice.action", explanation.actions)}
          </dd>
        </div>
        <div>
          <dt>{label(locale, "policy.explain.conditions")}</dt>
          <dd>
            {explanation.conditions?.length
              ? explanation.conditions
                  .map((condition) =>
                    choice(locale, "policy.choice.condition", condition.kind),
                  )
                  .join(", ")
              : label(locale, "policy.result.none")}
          </dd>
        </div>
      </dl>
    </article>
  );
}

export function PolicyWorkflow({
  actions,
  snapshot,
  selectedNetwork,
  locale,
  pendingAction,
  interactionBlocked = null,
  draft,
  onDraftChange,
  onSubmit,
}: PolicyWorkflowProps): JSX.Element {
  const action = actions.find((candidate) => candidate.id === "test_policy");
  const getAction = actions.find((candidate) => candidate.id === "get_policy");
  const previewAction = actions.find(
    (candidate) => candidate.id === "preview_policy",
  );
  const previewRollbackAction = actions.find(
    (candidate) => candidate.id === "preview_policy_rollback",
  );
  const applyAction = actions.find(
    (candidate) => candidate.id === "apply_policy",
  );
  const rollbackAction = actions.find(
    (candidate) => candidate.id === "rollback_policy",
  );
  const historyAction = actions.find(
    (candidate) => candidate.id === "get_policy_history",
  );
  const titleId = useId();
  const editorId = useId();
  const importId = useId();
  const previewReasonId = useId();
  const runReasonId = useId();
  const applyReasonId = useId();
  const rollbackReasonId = useId();
  const applyDialogId = useId();
  const rollbackDialogId = useId();
  const historyReasonId = useId();
  const editorRef = useRef<HTMLTextAreaElement>(null);
  const fileInputRef = useRef<HTMLInputElement>(null);
  const initialLoadStarted = useRef(false);
  const [structured, setStructured] = useState<PolicyStructuredDraft>(
    restoredStructuredDraft(
      draft?.structured,
      defaultStructuredDraft(snapshot, selectedNetwork),
    ),
  );
  const [editor, setEditor] = useState(
    draft?.editor ?? prettyPolicy(structuredPolicyInput(structured)),
  );
  const [mode, setMode] = useState<EditorMode>(draft?.mode ?? "structured");
  const [result, setResult] = useState<PolicyTestResult | null>(
    draft?.result ?? null,
  );
  const [current, setCurrent] = useState<PolicyCurrent | null>(
    draft?.current ?? null,
  );
  const [history, setHistory] = useState<PolicyHistorySnapshot | null>(
    draft?.history ?? null,
  );
  const [preview, setPreview] = useState<PolicyPreview | null>(
    draft?.preview ?? null,
  );
  const [applied, setApplied] = useState<
    PolicyApplyResult | PolicyRollbackResult | null
  >(draft?.applied ?? null);
  const [importName, setImportName] = useState(draft?.importName ?? "");
  const [activeIssuePath, setActiveIssuePath] = useState(
    draft?.activeIssuePath ?? "",
  );
  const [jsonError, setJsonError] = useState("");
  const [submitError, setSubmitError] = useState("");
  const [transactionError, setTransactionError] = useState("");
  const [historyError, setHistoryError] = useState("");
  const [notice, setNotice] = useState("");
  const [running, setRunning] = useState(false);
  const [busy, setBusy] = useState<BusyState>(
    draft?.current ? null : getAction ? "load" : null,
  );
  const [confirmPrivilegeExpansion, setConfirmPrivilegeExpansion] =
    useState(false);
  const [confirmingApply, setConfirmingApply] = useState(false);
  const [confirmingRollback, setConfirmingRollback] = useState(false);

  const pageBlocked = interactionBlocked
    ? pageStateDefinition(interactionBlocked)
    : null;
  const testBlocked = action
    ? executionBlockReason(action, snapshot, selectedNetwork)
    : "backend_unavailable";
  const previewBlocked = previewAction
    ? executionBlockReason(previewAction, snapshot, selectedNetwork)
    : "backend_unavailable";
  const applyBlocked = applyAction
    ? executionBlockReason(applyAction, snapshot, selectedNetwork)
    : "backend_unavailable";
  const previewRollbackBlocked = previewRollbackAction
    ? executionBlockReason(previewRollbackAction, snapshot, selectedNetwork)
    : "backend_unavailable";
  const rollbackBlocked = rollbackAction
    ? executionBlockReason(rollbackAction, snapshot, selectedNetwork)
    : "backend_unavailable";
  const historyBlocked = historyAction
    ? executionBlockReason(historyAction, snapshot, selectedNetwork)
    : "backend_unavailable";
  const pending = Boolean(pendingAction) || running || busy !== null;
  const pendingReason = pending ? label(locale, "common.control_pending") : "";
  const pageReason = pageBlocked ? label(locale, pageBlocked.titleKey) : "";
  const unavailableReason = label(locale, "blocked.backend_unavailable");
  const firstReason = (...reasons: Array<string | false | null>): string =>
    reasons.find((reason): reason is string => Boolean(reason)) ?? "";
  const readOnlyPermissionReason = (
    blocked: ReturnType<typeof executionBlockReason>,
    available: boolean,
  ): string => {
    if (!available || blocked !== "permission_denied") return "";
    return [
      label(locale, "settings.policy_access.read_only"),
      blockReasonText(blocked, locale),
      tr(locale, "settings.policy_access.required_permission", {
        permission: label(locale, "choice.permission.manage_networks"),
      }),
      label(locale, "settings.policy_access.request_path"),
      tr(locale, "settings.policy_access.command", {
        command: "apply_policy",
      }),
    ]
      .filter(Boolean)
      .join(label(locale, "common.list_separator"));
  };
  const runReason = firstReason(
    pendingReason,
    pageReason,
    blockReasonText(testBlocked, locale),
    !action && unavailableReason,
  );
  const previewReason = firstReason(
    pendingReason,
    pageReason,
    blockReasonText(previewBlocked, locale),
    !previewAction && unavailableReason,
    !current && label(locale, "policy.transaction.load_required"),
    Boolean(jsonError) && label(locale, "policy.transaction.invalid_editor"),
  );
  const stalePreview = Boolean(
    preview &&
    current &&
    !preview.revision_conflict &&
    preview.expected_revision !== current.revision,
  );
  const missingPrivilegeConfirmation = Boolean(
    preview?.risk.requires_explicit_confirmation && !confirmPrivilegeExpansion,
  );
  const rollbackPreview =
    preview?.rollback_target_revision !== null &&
    preview?.rollback_target_revision !== undefined;
  const canApply = Boolean(
    preview &&
    !rollbackPreview &&
    preview.can_apply &&
    preview.has_changes &&
    !preview.revision_conflict &&
    !stalePreview &&
    !missingPrivilegeConfirmation &&
    !applyBlocked &&
    !pageBlocked &&
    !pending,
  );
  const canRollback = Boolean(
    preview &&
    rollbackPreview &&
    preview.can_apply &&
    preview.has_changes &&
    !preview.revision_conflict &&
    !stalePreview &&
    !missingPrivilegeConfirmation &&
    !rollbackBlocked &&
    !pageBlocked &&
    !pending,
  );
  const applyAccessReason = readOnlyPermissionReason(
    applyBlocked,
    Boolean(applyAction),
  );
  const rollbackAccessReason = readOnlyPermissionReason(
    rollbackBlocked,
    Boolean(rollbackAction),
  );
  const applyReason = firstReason(
    pendingReason,
    pageReason,
    applyAccessReason || blockReasonText(applyBlocked, locale),
    !applyAction && unavailableReason,
    !preview && label(locale, "policy.transaction.preview_required"),
    Boolean(preview?.revision_conflict) &&
      label(locale, "policy.transaction.revision_conflict"),
    stalePreview && label(locale, "policy.transaction.stale_preview"),
    Boolean(preview && !preview.has_changes) &&
      label(locale, "policy.transaction.no_changes"),
    Boolean(preview && !preview.can_apply) &&
      label(locale, "policy.transaction.blocked"),
    missingPrivilegeConfirmation &&
      label(locale, "policy.transaction.confirm_required"),
  );
  const rollbackReason = firstReason(
    pendingReason,
    pageReason,
    rollbackAccessReason || blockReasonText(rollbackBlocked, locale),
    !rollbackAction && unavailableReason,
    !preview && label(locale, "policy.transaction.preview_required"),
    !rollbackPreview &&
      label(locale, "policy.transaction.rollback_preview_required"),
    Boolean(preview?.revision_conflict) &&
      label(locale, "policy.transaction.revision_conflict"),
    stalePreview && label(locale, "policy.transaction.stale_preview"),
    Boolean(preview && !preview.has_changes) &&
      label(locale, "policy.transaction.no_changes"),
    Boolean(preview && !preview.can_apply) &&
      label(locale, "policy.transaction.blocked"),
    missingPrivilegeConfirmation &&
      label(locale, "policy.transaction.confirm_required"),
  );
  const historyRefreshReason = firstReason(
    pendingReason,
    pageReason,
    blockReasonText(historyBlocked, locale),
    !historyAction && unavailableReason,
  );
  const rollbackCompleted = Boolean(applied && "target_revision" in applied);
  const activePolicyStates: Array<{
    id: string;
    label: string;
    tone: "warning" | "active" | "danger" | "success";
  }> = [];
  if (preview?.revision_conflict) {
    activePolicyStates.push({
      id: "conflict",
      label: label(locale, "policy.state.conflict"),
      tone: "warning",
    });
  }
  if (stalePreview) {
    activePolicyStates.push({
      id: "stale",
      label: label(locale, "policy.state.stale"),
      tone: "warning",
    });
  }
  if (busy === "apply") {
    activePolicyStates.push({
      id: "applying",
      label: label(locale, "policy.state.applying"),
      tone: "active",
    });
  }
  if (busy === "rollback") {
    activePolicyStates.push({
      id: "rolling_back",
      label: label(locale, "policy.state.rolling_back"),
      tone: "active",
    });
  }
  if (transactionError) {
    activePolicyStates.push({
      id: "failed",
      label: label(locale, "policy.state.failed"),
      tone: "danger",
    });
  }
  if (applied) {
    activePolicyStates.push({
      id: rollbackCompleted ? "rolled_back" : "applied",
      label: label(
        locale,
        rollbackCompleted ? "policy.state.rolled_back" : "policy.state.applied",
      ),
      tone: "success",
    });
  }

  useEffect(() => {
    onDraftChange?.({
      structured,
      editor,
      mode,
      result,
      current,
      history,
      preview,
      applied,
      importName,
      activeIssuePath,
    });
  }, [
    activeIssuePath,
    applied,
    current,
    editor,
    history,
    importName,
    mode,
    onDraftChange,
    preview,
    result,
    structured,
  ]);

  async function request(
    requestAction: NodeUiAction | undefined,
    values: ActionValues,
  ): Promise<NodeCommandOutcome> {
    if (!requestAction)
      throw new Error(label(locale, "common.operation_failed"));
    const outcome = await onSubmit(requestAction, values);
    const rejected = rejectedMessage(outcome, locale);
    if (rejected) throw new Error(rejected);
    return outcome;
  }

  async function refreshHistory(): Promise<void> {
    if (!historyAction || historyBlocked) return;
    try {
      const outcome = await request(historyAction, {});
      if (outcome.status !== "policy_history") {
        throw new Error(label(locale, "common.operation_failed"));
      }
      setHistory(outcome.data);
      setHistoryError("");
    } catch (historyFailure) {
      setHistoryError(
        historyFailure instanceof Error
          ? historyFailure.message
          : String(historyFailure),
      );
    }
  }

  async function loadPolicy(): Promise<void> {
    if (!getAction) return;
    setBusy("load");
    setTransactionError("");
    setHistoryError("");
    setNotice("");
    try {
      const outcome = await request(getAction, {});
      if (outcome.status !== "policy") {
        throw new Error(label(locale, "common.operation_failed"));
      }
      setCurrent(outcome.data);
      setPreview(null);
      setApplied(null);
      setConfirmPrivilegeExpansion(false);
      setNotice(outcomeSummary(outcome, locale));
      await refreshHistory();
    } catch (loadFailure) {
      setTransactionError(
        loadFailure instanceof Error
          ? loadFailure.message
          : String(loadFailure),
      );
    } finally {
      setBusy(null);
    }
  }

  useEffect(() => {
    if (!getAction || initialLoadStarted.current) return;
    initialLoadStarted.current = true;
    void loadPolicy();
    // The authoritative revision is loaded once when the settings page mounts.
    // AppShell recreates submit callbacks, so they must not restart this flow.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [getAction]);

  function invalidateTransactionPreview(): void {
    setPreview(null);
    setApplied(null);
    setConfirmPrivilegeExpansion(false);
    setTransactionError("");
  }

  function clearTransientState(): void {
    setResult(null);
    setJsonError("");
    setSubmitError("");
    setNotice("");
    setActiveIssuePath("");
    invalidateTransactionPreview();
  }

  function updateStructured(next: PolicyStructuredDraft): void {
    setStructured(next);
    if (mode === "structured") {
      setEditor(prettyPolicy(structuredPolicyInput(next)));
    }
    setResult(null);
    setJsonError("");
    setSubmitError("");
    setNotice("");
    setActiveIssuePath("");
    invalidateTransactionPreview();
  }

  function updateRule(
    source: "local" | "acl",
    index: number,
    next: PolicyRuleDraft,
  ): void {
    const key = source === "local" ? "localRules" : "aclRules";
    updateStructured({
      ...structured,
      [key]: structured[key].map((rule, ruleIndex) =>
        ruleIndex === index ? next : rule,
      ),
    });
  }

  function removeRule(source: "local" | "acl", index: number): void {
    const key = source === "local" ? "localRules" : "aclRules";
    updateStructured({
      ...structured,
      [key]: structured[key].filter((_, ruleIndex) => ruleIndex !== index),
    });
  }

  function addRule(source: "local" | "acl"): void {
    const key = source === "local" ? "localRules" : "aclRules";
    updateStructured({
      ...structured,
      [key]: [
        ...structured[key],
        emptyRule(
          structured[key].length,
          source === "local" ? "deny" : "allow",
        ),
      ],
    });
  }

  function selectMode(nextMode: EditorMode): void {
    if (nextMode === mode) return;
    invalidateTransactionPreview();
    if (nextMode === "advanced") {
      setEditor(prettyPolicy(structuredPolicyInput(structured)));
      setMode("advanced");
      setJsonError("");
      setActiveIssuePath("");
      return;
    }
    try {
      const parsed = parsePolicyTestInputJson(editor);
      setStructured(structuredDraftFromPolicyInput(parsed, structured));
      setMode("structured");
      setJsonError("");
      setActiveIssuePath("");
    } catch (error) {
      setMode("advanced");
      setJsonError(errorMessage(error, locale));
      if (error instanceof PolicyJsonError) setActiveIssuePath(error.path);
    }
  }

  function changeAdvanced(value: string): void {
    setEditor(value);
    setResult(null);
    setSubmitError("");
    setNotice("");
    setImportName("");
    invalidateTransactionPreview();
    try {
      const parsed = parsePolicyTestInputJson(value);
      setStructured(structuredDraftFromPolicyInput(parsed, structured));
      setJsonError("");
      setActiveIssuePath("");
    } catch (error) {
      setJsonError(errorMessage(error, locale));
      setActiveIssuePath(error instanceof PolicyJsonError ? error.path : "");
    }
  }

  async function importFile(
    event: ChangeEvent<HTMLInputElement>,
  ): Promise<void> {
    const file = event.target.files?.[0];
    if (!file) return;
    const source = await file.text();
    setMode("advanced");
    setEditor(source);
    setImportName(file.name);
    clearTransientState();
    try {
      const parsed = parsePolicyTestInputJson(source);
      setStructured(structuredDraftFromPolicyInput(parsed, structured));
      setNotice(tr(locale, "policy.imported_from", { file: file.name }));
    } catch (error) {
      setJsonError(errorMessage(error, locale));
      if (error instanceof PolicyJsonError) setActiveIssuePath(error.path);
    } finally {
      event.target.value = "";
    }
  }

  function focusPath(path: string): void {
    setMode("advanced");
    setActiveIssuePath(path);
    setTimeout(() => {
      const textarea = editorRef.current;
      if (!textarea) return;
      const selection = jsonPointerSelection(textarea.value, path);
      textarea.focus();
      textarea.setSelectionRange(selection.start, selection.end);
    }, 0);
  }

  async function runTest(): Promise<void> {
    if (!action || testBlocked || pageBlocked || pending) return;
    setResult(null);
    setJsonError("");
    setSubmitError("");
    setNotice("");
    setActiveIssuePath("");
    let parsed: PolicyTestInput;
    try {
      parsed = parsePolicyTestInputJson(editor);
      setJsonError("");
    } catch (error) {
      setMode("advanced");
      setJsonError(errorMessage(error, locale));
      if (error instanceof PolicyJsonError) setActiveIssuePath(error.path);
      return;
    }
    setRunning(true);
    try {
      const outcome = await onSubmit(action, {
        policy_json: JSON.stringify(parsed),
      });
      const rejected = rejectedMessage(outcome, locale);
      if (rejected) throw new Error(rejected);
      if (outcome.status !== "policy_test") {
        throw new Error(label(locale, "common.operation_failed"));
      }
      setResult(outcome.data);
    } catch (error) {
      setSubmitError(error instanceof Error ? error.message : String(error));
    } finally {
      setRunning(false);
    }
  }

  async function previewDraft(): Promise<void> {
    if (!previewAction || !current || previewBlocked || pageBlocked || pending)
      return;
    setBusy("preview");
    setTransactionError("");
    setNotice("");
    setApplied(null);
    setConfirmPrivilegeExpansion(false);
    try {
      const candidate = structuredPolicySnapshot(structured);
      const outcome = await request(previewAction, {
        expected_revision: String(current.revision),
        candidate_json: policyJson(candidate),
      });
      if (outcome.status !== "policy_preview") {
        throw new Error(label(locale, "common.operation_failed"));
      }
      setPreview(outcome.data);
      setNotice(outcomeSummary(outcome, locale));
    } catch (previewFailure) {
      setPreview(null);
      setTransactionError(
        previewFailure instanceof Error
          ? previewFailure.message
          : String(previewFailure),
      );
    } finally {
      setBusy(null);
    }
  }

  async function applyPreview(): Promise<void> {
    if (!applyAction || !preview || !canApply || rollbackPreview) return;
    setConfirmingApply(false);
    setBusy("apply");
    setTransactionError("");
    setNotice("");
    try {
      const outcome = await request(applyAction, {
        expected_revision: String(preview.expected_revision),
        candidate_json: policyJson(preview.candidate),
        network_ids: preview.network_ids.join(","),
        includes_local_policy: String(preview.includes_local_policy),
        confirm_privilege_expansion: String(confirmPrivilegeExpansion),
      });
      if (outcome.status !== "policy_applied") {
        throw new Error(label(locale, "common.operation_failed"));
      }
      setApplied(outcome.data);
      setCurrent({
        revision: outcome.data.revision,
        snapshot: outcome.data.snapshot,
      });
      setPreview(null);
      setConfirmPrivilegeExpansion(false);
      setNotice(outcomeSummary(outcome, locale));
      await refreshHistory();
    } catch (applyFailure) {
      setTransactionError(
        applyFailure instanceof Error
          ? applyFailure.message
          : String(applyFailure),
      );
    } finally {
      setBusy(null);
    }
  }

  async function previewRollback(targetRevision: number): Promise<void> {
    if (
      !previewRollbackAction ||
      !current ||
      previewRollbackBlocked ||
      pageBlocked ||
      pending
    )
      return;
    setBusy("preview");
    setTransactionError("");
    setNotice("");
    setApplied(null);
    setConfirmPrivilegeExpansion(false);
    try {
      const outcome = await request(previewRollbackAction, {
        expected_revision: String(current.revision),
        target_revision: String(targetRevision),
      });
      if (outcome.status !== "policy_preview") {
        throw new Error(label(locale, "common.operation_failed"));
      }
      setPreview(outcome.data);
      setNotice(outcomeSummary(outcome, locale));
    } catch (rollbackPreviewFailure) {
      setPreview(null);
      setTransactionError(
        rollbackPreviewFailure instanceof Error
          ? rollbackPreviewFailure.message
          : String(rollbackPreviewFailure),
      );
    } finally {
      setBusy(null);
    }
  }

  async function rollbackPreviewed(): Promise<void> {
    if (
      !rollbackAction ||
      !preview ||
      preview.rollback_target_revision === null ||
      preview.rollback_target_revision === undefined ||
      !canRollback
    )
      return;
    setConfirmingRollback(false);
    setBusy("rollback");
    setTransactionError("");
    setNotice("");
    try {
      const outcome = await request(rollbackAction, {
        expected_revision: String(preview.expected_revision),
        target_revision: String(preview.rollback_target_revision),
        network_ids: preview.network_ids.join(","),
        includes_local_policy: String(preview.includes_local_policy),
        confirm_privilege_expansion: String(confirmPrivilegeExpansion),
      });
      if (outcome.status !== "policy_rolled_back") {
        throw new Error(label(locale, "common.operation_failed"));
      }
      setApplied(outcome.data);
      setCurrent({
        revision: outcome.data.revision,
        snapshot: outcome.data.snapshot,
      });
      setPreview(null);
      setConfirmPrivilegeExpansion(false);
      setNotice(outcomeSummary(outcome, locale));
      await refreshHistory();
    } catch (rollbackFailure) {
      setTransactionError(
        rollbackFailure instanceof Error
          ? rollbackFailure.message
          : String(rollbackFailure),
      );
    } finally {
      setBusy(null);
    }
  }

  function loadCurrentIntoEditor(): void {
    if (!current) return;
    const next = structuredSnapshotDraft(current.snapshot, structured);
    setStructured(next);
    setEditor(prettyPolicy(structuredPolicyInput(next)));
    setMode("structured");
    setResult(null);
    setJsonError("");
    setSubmitError("");
    setNotice(label(locale, "policy.transaction.loaded_into_editor"));
    setActiveIssuePath("");
    invalidateTransactionPreview();
  }

  function reset(): void {
    const next = defaultStructuredDraft(snapshot, selectedNetwork);
    setStructured(next);
    setEditor(prettyPolicy(structuredPolicyInput(next)));
    setMode("structured");
    setResult(null);
    setImportName("");
    setActiveIssuePath("");
    setJsonError("");
    setSubmitError("");
    setNotice("");
    invalidateTransactionPreview();
  }

  const resultDecision = result?.decision;
  const runDisabled = Boolean(runReason);
  const editorErrorId = `${editorId}-error`;

  return (
    <section className="policy-workspace" aria-labelledby={titleId}>
      <header className="policy-heading">
        <div className="policy-heading-mark" aria-hidden="true">
          <ShieldCheck size={22} />
        </div>
        <div className="policy-heading-copy">
          <span>{label(locale, "policy.eyebrow")}</span>
          <h2 id={titleId}>{label(locale, "policy.title")}</h2>
          <p>{label(locale, "policy.description")}</p>
        </div>
        <div className="policy-heading-actions">
          <button className="secondary-button" type="button" onClick={reset}>
            <RefreshCw size={15} />
            {label(locale, "policy.reset")}
          </button>
        </div>
      </header>

      <div className="policy-side-effect-note" role="note">
        <ShieldAlert size={18} />
        <div>
          <strong>{label(locale, "policy.not_applied")}</strong>
          <span>{label(locale, "policy.deny_priority.no_recommendation")}</span>
        </div>
      </div>

      <section
        className="policy-transaction"
        aria-labelledby={`${titleId}-transaction`}
      >
        <div className="policy-transaction-heading">
          <div>
            <span>{label(locale, "policy.transaction.eyebrow")}</span>
            <h3 id={`${titleId}-transaction`}>
              {label(locale, "policy.transaction.title")}
            </h3>
            <p>{label(locale, "policy.transaction.description")}</p>
          </div>
          <div className="policy-heading-actions">
            <button
              className="secondary-button"
              type="button"
              disabled={Boolean(
                firstReason(
                  pendingReason,
                  pageReason,
                  blockReasonText(
                    getAction
                      ? executionBlockReason(
                          getAction,
                          snapshot,
                          selectedNetwork,
                        )
                      : "backend_unavailable",
                    locale,
                  ),
                  !getAction && unavailableReason,
                ),
              )}
              onClick={() => void loadPolicy()}
            >
              {busy === "load" ? (
                <LoaderCircle className="spin" size={15} />
              ) : (
                <RefreshCw size={15} />
              )}
              {label(locale, "policy.transaction.reload")}
            </button>
          </div>
        </div>

        <div className="policy-transaction-note" role="note">
          <ShieldAlert size={17} />
          <span>{label(locale, "policy.transaction.complete_snapshot")}</span>
        </div>

        <div
          className="policy-state-track"
          aria-label={label(locale, "policy.state.title")}
          aria-live="polite"
          aria-atomic="false"
        >
          {activePolicyStates.length ? (
            activePolicyStates.map((state) => (
              <span
                className={`status-pill policy-state policy-state-${state.tone}`}
                data-policy-state={state.id}
                key={state.id}
              >
                {state.tone === "active" ? (
                  <LoaderCircle className="spin" size={13} />
                ) : null}
                {state.label}
              </span>
            ))
          ) : (
            <span className="policy-state-idle">
              {label(locale, "policy.state.idle")}
            </span>
          )}
        </div>

        {transactionError ? (
          <div className="inline-notice danger" role="alert">
            <AlertTriangle size={16} />
            <span>{transactionError}</span>
          </div>
        ) : null}
        {notice ? (
          <div className="inline-notice success" role="status">
            <CheckCircle2 size={16} />
            <span>{notice}</span>
          </div>
        ) : null}

        <div className="policy-transaction-grid">
          <section
            className="policy-transaction-card"
            aria-labelledby={`${titleId}-current`}
          >
            <div className="policy-subsection-heading">
              <h4 id={`${titleId}-current`}>
                <ShieldCheck size={16} />
                {label(locale, "policy.current.title")}
              </h4>
              {current ? (
                <span className="status-pill neutral">
                  {tr(locale, "policy.current.revision", {
                    revision: current.revision,
                  })}
                </span>
              ) : null}
            </div>
            {busy === "load" && !current ? (
              <div className="policy-transaction-empty" role="status">
                <LoaderCircle className="spin" size={20} />
                <p>{label(locale, "policy.current.loading")}</p>
              </div>
            ) : !current ? (
              <div className="policy-transaction-empty">
                <ShieldAlert size={20} />
                <p>{label(locale, "policy.current.empty")}</p>
              </div>
            ) : (
              <>
                <dl className="policy-transaction-facts">
                  <div>
                    <dt>{label(locale, "policy.current.local_policy")}</dt>
                    <dd>
                      {current.snapshot.local_policy
                        ? tr(locale, "policy.current.rules_count", {
                            count:
                              current.snapshot.local_policy.rules?.length ?? 0,
                          })
                        : label(locale, "policy.current.none")}
                    </dd>
                  </div>
                  <div>
                    <dt>{label(locale, "policy.current.network_acls")}</dt>
                    <dd>
                      {current.snapshot.network_acls?.length
                        ? current.snapshot.network_acls
                            .map((entry) =>
                              tr(locale, "policy.current.acl_revision", {
                                network: entry.network_id,
                                revision: entry.revision,
                              }),
                            )
                            .join(" · ")
                        : label(locale, "policy.current.none")}
                    </dd>
                  </div>
                </dl>
                <button
                  className="secondary-button compact"
                  type="button"
                  onClick={loadCurrentIntoEditor}
                >
                  <FileJson size={14} />
                  {label(locale, "policy.current.load_into_editor")}
                </button>
              </>
            )}
          </section>

          <section
            className="policy-transaction-card"
            aria-labelledby={`${titleId}-preview`}
          >
            <div className="policy-subsection-heading">
              <h4 id={`${titleId}-preview`}>
                <GitCompare size={16} />
                {label(locale, "policy.preview.title")}
              </h4>
              <button
                className="primary-button compact"
                type="button"
                disabled={Boolean(previewReason)}
                aria-describedby={
                  previewReason ? previewReasonId : undefined
                }
                onClick={() => void previewDraft()}
              >
                {busy === "preview" ? (
                  <LoaderCircle className="spin" size={15} />
                ) : (
                  <GitCompare size={15} />
                )}
                {label(locale, "policy.preview")}
              </button>
            </div>
            {previewReason ? (
              <small
                id={previewReasonId}
                className="control-reason"
                role="status"
              >
                {previewReason}
              </small>
            ) : null}
            {!preview ? (
              <div className="policy-transaction-empty">
                <GitCompare size={20} />
                <p>{label(locale, "policy.preview.empty")}</p>
              </div>
            ) : (
              <div className="policy-preview-body">
                {preview.revision_conflict ? (
                  <div className="inline-notice warning" role="alert">
                    <ShieldAlert size={16} />
                    <div>
                      <strong>
                        {label(locale, "policy.transaction.revision_conflict")}
                      </strong>
                      <p>
                        {tr(
                          locale,
                          "policy.transaction.revision_conflict_detail",
                          {
                            expected: preview.expected_revision,
                            current: preview.current_revision,
                          },
                        )}
                      </p>
                    </div>
                  </div>
                ) : null}
                {stalePreview ? (
                  <div className="inline-notice warning" role="alert">
                    <AlertTriangle size={16} />
                    <span>
                      {label(locale, "policy.transaction.stale_preview")}
                    </span>
                  </div>
                ) : null}
                <div className={`policy-risk risk-${preview.risk.level}`}>
                  <div className="policy-risk-heading">
                    <strong>{label(locale, "policy.risk.title")}</strong>
                    <span className="status-pill neutral">
                      {riskLevelText(locale, preview.risk)}
                    </span>
                  </div>
                  {preview.risk.reasons.length ? (
                    <ul>
                      {preview.risk.reasons.map((reason) => (
                        <li key={reason}>{riskReasonText(locale, reason)}</li>
                      ))}
                    </ul>
                  ) : (
                    <p>{label(locale, "policy.risk.none")}</p>
                  )}
                  <div className="policy-risk-flags">
                    <span>
                      {label(locale, "policy.risk.privilege_expansion")}:{" "}
                      {preview.risk.privilege_expansion
                        ? label(locale, "policy.yes")
                        : label(locale, "policy.no")}
                    </span>
                    <span>
                      {label(locale, "policy.risk.recent_authentication")}:{" "}
                      {preview.risk.requires_recent_reauthentication
                        ? label(locale, "policy.required")
                        : label(locale, "policy.not_required")}
                    </span>
                  </div>
                  {preview.risk.requires_explicit_confirmation ? (
                    <label className="policy-check policy-risk-confirm">
                      <input
                        type="checkbox"
                        checked={confirmPrivilegeExpansion}
                        onChange={(event) =>
                          setConfirmPrivilegeExpansion(event.target.checked)
                        }
                      />
                      <span>
                        {label(
                          locale,
                          "policy.transaction.confirm_privilege_expansion",
                        )}
                      </span>
                    </label>
                  ) : null}
                </div>

                {!preview.has_changes ? (
                  <div className="inline-notice neutral" role="status">
                    <CircleSlash2 size={16} />
                    <span>
                      {label(locale, "policy.transaction.no_changes")}
                    </span>
                  </div>
                ) : null}

                <div className="policy-transaction-summary">
                  <span>
                    {tr(locale, "policy.preview.diff_count", {
                      count: preview.diffs.length,
                    })}
                  </span>
                  <span>
                    {label(locale, "policy.preview.network_scope")}:{" "}
                    {preview.network_ids.length
                      ? preview.network_ids.join(", ")
                      : label(locale, "policy.current.none")}
                  </span>
                  <span>
                    {label(locale, "policy.preview.includes_local")}:{" "}
                    {preview.includes_local_policy
                      ? label(locale, "policy.yes")
                      : label(locale, "policy.no")}
                  </span>
                </div>

                <section className="policy-affected">
                  <h5>{label(locale, "policy.preview.affected_objects")}</h5>
                  {preview.affected_objects.length ? (
                    <ul>
                      {preview.affected_objects.map((object, index) => (
                        <li
                          key={`${object.kind}:${object.network_id ?? ""}:${object.object_id ?? ""}:${index}`}
                        >
                          {affectedObjectText(locale, object)}
                        </li>
                      ))}
                    </ul>
                  ) : (
                    <p>{label(locale, "policy.current.none")}</p>
                  )}
                </section>

                <section className="policy-transaction-diffs">
                  <h5>{label(locale, "policy.preview.diffs")}</h5>
                  {preview.diffs.length === 0 ? (
                    <p>{label(locale, "policy.transaction.no_changes")}</p>
                  ) : (
                    <ol>
                      {preview.diffs.map((diff, index) => (
                        <li key={`${diff.path}:${diff.change}:${index}`}>
                          <div className="policy-diff-heading">
                            <code>{diff.path}</code>
                            <span>{diffChangeText(locale, diff.change)}</span>
                          </div>
                          <div className="policy-diff-values">
                            <div>
                              <span>{label(locale, "policy.diff.before")}</span>
                              <pre>{policyJson(diff.before)}</pre>
                            </div>
                            <div>
                              <span>{label(locale, "policy.diff.after")}</span>
                              <pre>{policyJson(diff.after)}</pre>
                            </div>
                          </div>
                        </li>
                      ))}
                    </ol>
                  )}
                </section>

                <details className="policy-candidate-json">
                  <summary>{label(locale, "policy.preview.candidate")}</summary>
                  <pre>{policyJson(preview.candidate)}</pre>
                </details>

                <div className="policy-transaction-actions">
                  {rollbackPreview ? (
                    <button
                      id={`${titleId}-execute-rollback`}
                      className="danger-button"
                      type="button"
                      disabled={Boolean(rollbackReason)}
                      aria-describedby={
                        rollbackReason ? rollbackReasonId : undefined
                      }
                      onClick={() => setConfirmingRollback(true)}
                    >
                      {busy === "rollback" ? (
                        <LoaderCircle className="spin" size={15} />
                      ) : (
                        <RotateCcw size={15} />
                      )}
                      {busy === "rollback"
                        ? label(locale, "policy.state.rolling_back")
                        : label(locale, "policy.rollback.execute")}
                    </button>
                  ) : (
                    <button
                      id={`${titleId}-execute-apply`}
                      className="primary-button"
                      type="button"
                      disabled={Boolean(applyReason)}
                      aria-describedby={
                        applyReason ? applyReasonId : undefined
                      }
                      onClick={() => setConfirmingApply(true)}
                    >
                      {busy === "apply" ? (
                        <LoaderCircle className="spin" size={15} />
                      ) : (
                        <ShieldCheck size={15} />
                      )}
                      {busy === "apply"
                        ? label(locale, "policy.state.applying")
                        : label(locale, "policy.apply")}
                    </button>
                  )}
                  {rollbackPreview ? (
                    rollbackReason ? (
                      <small
                        id={rollbackReasonId}
                        className="control-reason"
                        role="status"
                      >
                        {rollbackReason}
                      </small>
                    ) : null
                  ) : applyReason ? (
                    <small
                      id={applyReasonId}
                      className="control-reason"
                      role="status"
                    >
                      {applyReason}
                    </small>
                  ) : null}
                </div>
              </div>
            )}
          </section>

          <section
            className="policy-transaction-card"
            aria-labelledby={`${titleId}-history`}
          >
            <div className="policy-subsection-heading">
              <h4 id={`${titleId}-history`}>
                <History size={16} />
                {label(locale, "policy.history.title")}
              </h4>
              <button
                className="secondary-button compact"
                type="button"
                disabled={Boolean(historyRefreshReason)}
                onClick={() => void refreshHistory()}
              >
                {busy === "history" ? (
                  <LoaderCircle className="spin" size={15} />
                ) : (
                  <RefreshCw size={15} />
                )}
                {label(locale, "policy.history.refresh")}
              </button>
            </div>
            {historyError ? (
              <div className="inline-notice danger" role="alert">
                <AlertTriangle size={16} />
                <span>{historyError}</span>
              </div>
            ) : !history ? (
              <div className="policy-transaction-empty" role="status">
                <History size={20} />
                <p>{label(locale, "policy.history.empty")}</p>
              </div>
            ) : history.entries.length === 0 ? (
              <div className="policy-transaction-empty">
                <History size={20} />
                <p>{label(locale, "policy.history.empty")}</p>
              </div>
            ) : (
              <ol className="policy-history-list">
                {history.entries.map((entry, index) => {
                  const entryReasonId = `${historyReasonId}-${index}`;
                  const entryRollbackReason = firstReason(
                    pendingReason,
                    pageReason,
                    readOnlyPermissionReason(
                      previewRollbackBlocked,
                      Boolean(previewRollbackAction),
                    ) || blockReasonText(previewRollbackBlocked, locale),
                    !previewRollbackAction && unavailableReason,
                    !current &&
                      label(locale, "policy.transaction.load_required"),
                    Boolean(current && entry.revision === current.revision) &&
                      label(locale, "policy.history.current"),
                  );
                  return (
                    <li key={entry.transaction_id}>
                      <div className="policy-history-summary">
                        <div>
                          <strong>
                            {tr(locale, "policy.history.revision", {
                              revision: entry.revision,
                            })}
                          </strong>
                          <span>
                            {label(
                              locale,
                              `policy.history.action.${entry.action}`,
                            )}
                            {entry.target_revision !== null &&
                            entry.target_revision !== undefined
                              ? ` · ${tr(locale, "policy.history.target", {
                                  revision: entry.target_revision,
                                })}`
                              : ""}
                          </span>
                        </div>
                        <span
                          className={`status-pill risk-${entry.risk.level}`}
                        >
                          {riskLevelText(locale, entry.risk)}
                        </span>
                      </div>
                      <div className="policy-history-meta">
                        <span>
                          <Clock3 size={13} />
                          {timestampText(locale, entry.occurred_at)}
                        </span>
                        <span>{entry.source || entry.actor}</span>
                      </div>
                      <div className="policy-history-control">
                        <button
                          id={`${historyReasonId}-${index}-button`}
                          className="secondary-button compact"
                          type="button"
                          disabled={Boolean(entryRollbackReason)}
                          aria-describedby={
                            entryRollbackReason ? entryReasonId : undefined
                          }
                          onClick={() => void previewRollback(entry.revision)}
                        >
                          <RotateCcw size={14} />
                          {label(locale, "policy.rollback.action")}
                        </button>
                        {entryRollbackReason ? (
                          <small
                            id={entryReasonId}
                            className="control-reason"
                            role="status"
                          >
                            {entryRollbackReason}
                          </small>
                        ) : null}
                      </div>
                      <details>
                        <summary>
                          {label(locale, "policy.history.details")}
                        </summary>
                        <dl className="policy-history-facts">
                          <div>
                            <dt>
                              {label(locale, "policy.history.transaction_id")}
                            </dt>
                            <dd>
                              <code>{entry.transaction_id}</code>
                            </dd>
                          </div>
                          <div>
                            <dt>
                              {label(
                                locale,
                                "policy.history.previous_revision",
                              )}
                            </dt>
                            <dd>{entry.previous_revision}</dd>
                          </div>
                          <div>
                            <dt>{label(locale, "policy.history.actor")}</dt>
                            <dd>{entry.actor}</dd>
                          </div>
                          <div>
                            <dt>
                              {label(locale, "policy.preview.affected_objects")}
                            </dt>
                            <dd>
                              {entry.affected_objects.length
                                ? entry.affected_objects
                                    .map((object) =>
                                      affectedObjectText(locale, object),
                                    )
                                    .join(" · ")
                                : label(locale, "policy.current.none")}
                            </dd>
                          </div>
                        </dl>
                        {entry.risk.reasons.length ? (
                          <ul className="policy-history-reasons">
                            {entry.risk.reasons.map((reason) => (
                              <li key={reason}>
                                {riskReasonText(locale, reason)}
                              </li>
                            ))}
                          </ul>
                        ) : null}
                        <ol className="policy-history-diffs">
                          {entry.diffs.map((diff, index) => (
                            <li key={`${diff.path}:${diff.change}:${index}`}>
                              <code>{diff.path}</code>
                              <span>{diffChangeText(locale, diff.change)}</span>
                            </li>
                          ))}
                        </ol>
                      </details>
                    </li>
                  );
                })}
              </ol>
            )}
          </section>
        </div>

        {applied ? (
          <section className="policy-applied" aria-live="polite">
            <div className="policy-subsection-heading">
              <h4>
                <CheckCircle2 size={16} />
                {label(
                  locale,
                  rollbackCompleted
                    ? "policy.rolled_back.title"
                    : "policy.applied.title",
                )}
              </h4>
              <span className="status-pill success">
                {tr(locale, "policy.applied.revision", {
                  previous: applied.previous_revision,
                  revision: applied.revision,
                })}
              </span>
            </div>
            {"audit" in applied && applied.audit ? (
              <dl className="policy-history-facts">
                <div>
                  <dt>{label(locale, "policy.applied.transaction")}</dt>
                  <dd>
                    <code>{applied.audit.transaction_id}</code>
                  </dd>
                </div>
                <div>
                  <dt>{label(locale, "policy.applied.action")}</dt>
                  <dd>
                    {label(
                      locale,
                      `policy.history.action.${applied.audit.action}`,
                    )}
                  </dd>
                </div>
                <div>
                  <dt>{label(locale, "policy.applied.affected_objects")}</dt>
                  <dd>
                    {applied.affected_objects.length
                      ? applied.affected_objects
                          .map((object) => affectedObjectText(locale, object))
                          .join(" · ")
                      : label(locale, "policy.current.none")}
                  </dd>
                </div>
              </dl>
            ) : (
              <p>{label(locale, "policy.applied.no_changes")}</p>
            )}
            {applied.diffs.length ? (
              <details>
                <summary>{label(locale, "policy.applied.details")}</summary>
                <ol className="policy-history-diffs">
                  {applied.diffs.map((diff, index) => (
                    <li key={`${diff.path}:${diff.change}:${index}`}>
                      <code>{diff.path}</code>
                      <span>{diffChangeText(locale, diff.change)}</span>
                    </li>
                  ))}
                </ol>
              </details>
            ) : null}
          </section>
        ) : null}
      </section>

      <div className="policy-workspace-grid">
        <form
          className="policy-editor-card"
          onSubmit={(event) => {
            event.preventDefault();
            void runTest();
          }}
        >
          <div className="policy-section-heading">
            <div>
              <span>{label(locale, "policy.editor.eyebrow")}</span>
              <h3>{label(locale, "policy.editor.title")}</h3>
            </div>
            <div
              className="policy-mode-switch"
              aria-label={label(locale, "policy.mode.label")}
            >
              <button
                type="button"
                className={mode === "structured" ? "active" : ""}
                aria-pressed={mode === "structured"}
                onClick={() => selectMode("structured")}
              >
                {label(locale, "policy.mode.structured")}
              </button>
              <button
                type="button"
                className={mode === "advanced" ? "active" : ""}
                aria-pressed={mode === "advanced"}
                onClick={() => selectMode("advanced")}
              >
                <Braces size={14} />
                {label(locale, "policy.mode.advanced")}
              </button>
            </div>
          </div>

          {mode === "structured" ? (
            <div className="policy-structured">
              <p className="policy-editor-help">
                {label(locale, "policy.editor.help")}
              </p>
              <section
                className="policy-form-section"
                aria-labelledby={`${titleId}-context`}
              >
                <h4 id={`${titleId}-context`}>
                  <ListChecks size={16} />
                  {label(locale, "policy.section.context")}
                </h4>
                <div className="policy-context-grid">
                  <Field
                    id="policy-network-id"
                    labelText={label(locale, "policy.field.network_id")}
                  >
                    <input
                      id="policy-network-id"
                      value={structured.networkId}
                      spellCheck={false}
                      placeholder="net_..."
                      onChange={(event) =>
                        updateStructured({
                          ...structured,
                          networkId: event.target.value,
                        })
                      }
                    />
                  </Field>
                  <Field
                    id="policy-actor"
                    labelText={label(locale, "policy.field.actor")}
                  >
                    <input
                      id="policy-actor"
                      value={structured.actor}
                      spellCheck={false}
                      placeholder="node_..."
                      onChange={(event) =>
                        updateStructured({
                          ...structured,
                          actor: event.target.value,
                        })
                      }
                    />
                  </Field>
                  <Field
                    id="policy-role"
                    labelText={label(locale, "policy.field.role")}
                  >
                    <select
                      id="policy-role"
                      value={structured.role}
                      onChange={(event) =>
                        updateStructured({
                          ...structured,
                          role: event.target.value as NetworkRole,
                          emergencyCoordinator:
                            event.target.value === "temporary_coordinator",
                        })
                      }
                    >
                      {NETWORK_ROLES.map((role) => (
                        <option key={role} value={role}>
                          {choice(locale, "policy.choice.role", role)}
                        </option>
                      ))}
                    </select>
                  </Field>
                  <Field
                    id="policy-groups"
                    labelText={label(locale, "policy.field.groups")}
                  >
                    <input
                      id="policy-groups"
                      value={structured.groups}
                      placeholder="group_..."
                      onChange={(event) =>
                        updateStructured({
                          ...structured,
                          groups: event.target.value,
                        })
                      }
                    />
                  </Field>
                  <Field
                    id="policy-now"
                    labelText={label(locale, "policy.field.now_unix_ms")}
                  >
                    <input
                      id="policy-now"
                      inputMode="numeric"
                      value={structured.nowUnixMs}
                      onChange={(event) =>
                        updateStructured({
                          ...structured,
                          nowUnixMs: event.target.value,
                        })
                      }
                    />
                  </Field>
                </div>
                <div className="policy-check-grid">
                  <label className="policy-check">
                    <input
                      type="checkbox"
                      checked={structured.membershipActive}
                      onChange={(event) =>
                        updateStructured({
                          ...structured,
                          membershipActive: event.target.checked,
                        })
                      }
                    />
                    <span>
                      {label(locale, "policy.field.membership_active")}
                    </span>
                  </label>
                  <label className="policy-check">
                    <input
                      type="checkbox"
                      checked={structured.recentReauthentication}
                      onChange={(event) =>
                        updateStructured({
                          ...structured,
                          recentReauthentication: event.target.checked,
                        })
                      }
                    />
                    <span>
                      {label(locale, "policy.field.recent_reauthentication")}
                    </span>
                  </label>
                  <label className="policy-check">
                    <input
                      type="checkbox"
                      checked={structured.emergencyCoordinator}
                      onChange={(event) =>
                        updateStructured({
                          ...structured,
                          emergencyCoordinator: event.target.checked,
                        })
                      }
                    />
                    <span>
                      {label(locale, "policy.field.emergency_coordinator")}
                    </span>
                  </label>
                </div>
              </section>

              <section
                className="policy-form-section"
                aria-labelledby={`${titleId}-request`}
              >
                <h4 id={`${titleId}-request`}>
                  <Gavel size={16} />
                  {label(locale, "policy.section.request")}
                </h4>
                <div className="policy-context-grid">
                  <Field
                    id="policy-action"
                    labelText={label(locale, "policy.field.action")}
                  >
                    <select
                      id="policy-action"
                      value={structured.action}
                      onChange={(event) =>
                        updateStructured({
                          ...structured,
                          action: event.target.value as PolicyAction,
                        })
                      }
                    >
                      {POLICY_ACTIONS.map((policyAction) => (
                        <option key={policyAction} value={policyAction}>
                          {choice(locale, "policy.choice.action", policyAction)}
                        </option>
                      ))}
                    </select>
                  </Field>
                  <Field
                    id="policy-resource"
                    labelText={label(locale, "policy.field.resource")}
                  >
                    <select
                      id="policy-resource"
                      value={structured.resource}
                      onChange={(event) =>
                        updateStructured({
                          ...structured,
                          resource: event.target.value as ResourceKind,
                        })
                      }
                    >
                      {RESOURCE_KINDS.map((resource) => (
                        <option key={resource} value={resource}>
                          {choice(locale, "policy.choice.resource", resource)}
                        </option>
                      ))}
                    </select>
                  </Field>
                  <Field
                    id="policy-resource-network"
                    labelText={label(
                      locale,
                      "policy.field.resource_network_id",
                    )}
                  >
                    <input
                      id="policy-resource-network"
                      value={structured.resourceNetworkId}
                      spellCheck={false}
                      onChange={(event) =>
                        updateStructured({
                          ...structured,
                          resourceNetworkId: event.target.value,
                        })
                      }
                    />
                  </Field>
                  <Field
                    id="policy-object-id"
                    labelText={label(locale, "policy.field.object_id")}
                  >
                    <input
                      id="policy-object-id"
                      value={structured.objectId}
                      onChange={(event) =>
                        updateStructured({
                          ...structured,
                          objectId: event.target.value,
                        })
                      }
                    />
                  </Field>
                  <Field
                    id="policy-intent"
                    labelText={label(locale, "policy.field.intent")}
                  >
                    <select
                      id="policy-intent"
                      value={structured.intent}
                      onChange={(event) =>
                        updateStructured({
                          ...structured,
                          intent: event.target.value as PermissionIntent,
                        })
                      }
                    >
                      {PERMISSION_INTENTS.map((intent) => (
                        <option key={intent} value={intent}>
                          {choice(locale, "policy.choice.intent", intent)}
                        </option>
                      ))}
                    </select>
                  </Field>
                </div>
              </section>

              <section
                className="policy-form-section"
                aria-labelledby={`${titleId}-local`}
              >
                <div className="policy-section-subheading">
                  <h4 id={`${titleId}-local`}>
                    <ShieldAlert size={16} />
                    {label(locale, "policy.section.local")}
                  </h4>
                  <label className="policy-check compact">
                    <input
                      type="checkbox"
                      checked={structured.includeLocalPolicy}
                      onChange={(event) =>
                        updateStructured({
                          ...structured,
                          includeLocalPolicy: event.target.checked,
                        })
                      }
                    />
                    <span>{label(locale, "policy.include_local")}</span>
                  </label>
                </div>
                {structured.includeLocalPolicy ? (
                  <>
                    <div className="policy-context-grid policy-metadata-grid">
                      <Field
                        id="policy-local-revision"
                        labelText={label(locale, "policy.field.local_revision")}
                      >
                        <input
                          id="policy-local-revision"
                          inputMode="numeric"
                          value={structured.localRevision}
                          onChange={(event) =>
                            updateStructured({
                              ...structured,
                              localRevision: event.target.value,
                            })
                          }
                        />
                      </Field>
                      <Field
                        id="policy-local-issued"
                        labelText={label(
                          locale,
                          "policy.field.issued_at_unix_ms",
                        )}
                      >
                        <input
                          id="policy-local-issued"
                          inputMode="numeric"
                          value={structured.localIssuedAt}
                          onChange={(event) =>
                            updateStructured({
                              ...structured,
                              localIssuedAt: event.target.value,
                            })
                          }
                        />
                      </Field>
                      <Field
                        id="policy-local-expires"
                        labelText={label(
                          locale,
                          "policy.field.expires_at_unix_ms",
                        )}
                      >
                        <input
                          id="policy-local-expires"
                          inputMode="numeric"
                          value={structured.localExpiresAt}
                          onChange={(event) =>
                            updateStructured({
                              ...structured,
                              localExpiresAt: event.target.value,
                            })
                          }
                        />
                      </Field>
                      <Field
                        id="policy-local-scope"
                        labelText={label(locale, "policy.field.network_scope")}
                      >
                        <select
                          id="policy-local-scope"
                          value={structured.networkScopeKind}
                          onChange={(event) =>
                            updateStructured({
                              ...structured,
                              networkScopeKind: event.target.value as
                                "all" | "only",
                            })
                          }
                        >
                          <option value="all">
                            {label(locale, "policy.scope.all")}
                          </option>
                          <option value="only">
                            {label(locale, "policy.scope.only")}
                          </option>
                        </select>
                      </Field>
                      {structured.networkScopeKind === "only" ? (
                        <Field
                          id="policy-local-network-ids"
                          labelText={label(locale, "policy.field.network_ids")}
                        >
                          <input
                            id="policy-local-network-ids"
                            value={structured.networkIds}
                            onChange={(event) =>
                              updateStructured({
                                ...structured,
                                networkIds: event.target.value,
                              })
                            }
                          />
                        </Field>
                      ) : null}
                    </div>
                    <div className="policy-list-heading">
                      <h5>{label(locale, "policy.section.relay_limits")}</h5>
                    </div>
                    <p className="policy-editor-help">
                      {label(locale, "policy.help.relay_limits_local")}
                    </p>
                    <RelayLimitsEditor
                      idPrefix="policy-local-relay-limit"
                      limits={structured.localRelayLimits}
                      locale={locale}
                      onChange={(localRelayLimits) =>
                        updateStructured({ ...structured, localRelayLimits })
                      }
                    />
                    <div className="policy-list-heading">
                      <h5>{label(locale, "policy.section.rules")}</h5>
                      <button
                        className="secondary-button compact"
                        type="button"
                        onClick={() => addRule("local")}
                      >
                        <Plus size={14} />
                        {label(locale, "policy.add.rule")}
                      </button>
                    </div>
                    <div className="policy-rule-list">
                      {structured.localRules.map((rule, index) => (
                        <RuleEditor
                          key={`${rule.ruleId}-${index}`}
                          rule={rule}
                          index={index}
                          locale={locale}
                          source="local"
                          onChange={(next) => updateRule("local", index, next)}
                          onRemove={() => removeRule("local", index)}
                        />
                      ))}
                    </div>
                    <div className="policy-list-heading">
                      <h5>{label(locale, "policy.section.vetoes")}</h5>
                      <button
                        className="secondary-button compact"
                        type="button"
                        onClick={() =>
                          updateStructured({
                            ...structured,
                            vetoes: [
                              ...structured.vetoes,
                              emptyVeto(structured.vetoes.length),
                            ],
                          })
                        }
                      >
                        <Plus size={14} />
                        {label(locale, "policy.add.veto")}
                      </button>
                    </div>
                    <div className="policy-rule-list">
                      {structured.vetoes.map((veto, index) => (
                        <VetoEditor
                          key={`${veto.vetoId}-${index}`}
                          veto={veto}
                          index={index}
                          locale={locale}
                          onChange={(next) =>
                            updateStructured({
                              ...structured,
                              vetoes: structured.vetoes.map(
                                (entry, vetoIndex) =>
                                  vetoIndex === index ? next : entry,
                              ),
                            })
                          }
                          onRemove={() =>
                            updateStructured({
                              ...structured,
                              vetoes: structured.vetoes.filter(
                                (_, vetoIndex) => vetoIndex !== index,
                              ),
                            })
                          }
                        />
                      ))}
                    </div>
                  </>
                ) : null}
              </section>

              <section
                className="policy-form-section"
                aria-labelledby={`${titleId}-acl`}
              >
                <div className="policy-section-subheading">
                  <h4 id={`${titleId}-acl`}>
                    <GitBranch size={16} />
                    {label(locale, "policy.section.acl")}
                  </h4>
                  <label className="policy-check compact">
                    <input
                      type="checkbox"
                      checked={structured.includeNetworkAcl}
                      onChange={(event) =>
                        updateStructured({
                          ...structured,
                          includeNetworkAcl: event.target.checked,
                        })
                      }
                    />
                    <span>{label(locale, "policy.include_acl")}</span>
                  </label>
                </div>
                {structured.includeNetworkAcl ? (
                  <>
                    <div className="policy-context-grid policy-metadata-grid">
                      <Field
                        id="policy-acl-revision"
                        labelText={label(locale, "policy.field.acl_revision")}
                      >
                        <input
                          id="policy-acl-revision"
                          inputMode="numeric"
                          value={structured.aclRevision}
                          onChange={(event) =>
                            updateStructured({
                              ...structured,
                              aclRevision: event.target.value,
                            })
                          }
                        />
                      </Field>
                      <Field
                        id="policy-acl-issued"
                        labelText={label(
                          locale,
                          "policy.field.issued_at_unix_ms",
                        )}
                      >
                        <input
                          id="policy-acl-issued"
                          inputMode="numeric"
                          value={structured.aclIssuedAt}
                          onChange={(event) =>
                            updateStructured({
                              ...structured,
                              aclIssuedAt: event.target.value,
                            })
                          }
                        />
                      </Field>
                      <Field
                        id="policy-acl-expires"
                        labelText={label(
                          locale,
                          "policy.field.expires_at_unix_ms",
                        )}
                      >
                        <input
                          id="policy-acl-expires"
                          inputMode="numeric"
                          value={structured.aclExpiresAt}
                          onChange={(event) =>
                            updateStructured({
                              ...structured,
                              aclExpiresAt: event.target.value,
                            })
                          }
                        />
                      </Field>
                    </div>
                    <div className="policy-list-heading">
                      <h5>{label(locale, "policy.section.relay_limits")}</h5>
                    </div>
                    <p className="policy-editor-help">
                      {label(locale, "policy.help.relay_limits_acl")}
                    </p>
                    <RelayLimitsEditor
                      idPrefix="policy-acl-relay-limit"
                      limits={structured.aclRelayLimits}
                      locale={locale}
                      onChange={(aclRelayLimits) =>
                        updateStructured({ ...structured, aclRelayLimits })
                      }
                    />
                    <div className="policy-list-heading">
                      <h5>{label(locale, "policy.section.rules")}</h5>
                      <button
                        className="secondary-button compact"
                        type="button"
                        onClick={() => addRule("acl")}
                      >
                        <Plus size={14} />
                        {label(locale, "policy.add.rule")}
                      </button>
                    </div>
                    <div className="policy-rule-list">
                      {structured.aclRules.map((rule, index) => (
                        <RuleEditor
                          key={`${rule.ruleId}-${index}`}
                          rule={rule}
                          index={index}
                          locale={locale}
                          source="acl"
                          onChange={(next) => updateRule("acl", index, next)}
                          onRemove={() => removeRule("acl", index)}
                        />
                      ))}
                    </div>
                    <div className="policy-list-heading">
                      <h5>{label(locale, "policy.section.grants")}</h5>
                      <button
                        className="secondary-button compact"
                        type="button"
                        onClick={() =>
                          updateStructured({
                            ...structured,
                            grants: [
                              ...structured.grants,
                              emptyGrant(
                                structured.grants.length,
                                Number(structured.nowUnixMs),
                              ),
                            ],
                          })
                        }
                      >
                        <Plus size={14} />
                        {label(locale, "policy.add.grant")}
                      </button>
                    </div>
                    <div className="policy-rule-list">
                      {structured.grants.map((grant, index) => (
                        <GrantEditor
                          key={`${grant.grantId}-${index}`}
                          grant={grant}
                          index={index}
                          locale={locale}
                          onChange={(next) =>
                            updateStructured({
                              ...structured,
                              grants: structured.grants.map(
                                (entry, grantIndex) =>
                                  grantIndex === index ? next : entry,
                              ),
                            })
                          }
                          onRemove={() =>
                            updateStructured({
                              ...structured,
                              grants: structured.grants.filter(
                                (_, grantIndex) => grantIndex !== index,
                              ),
                            })
                          }
                        />
                      ))}
                    </div>
                  </>
                ) : null}
              </section>
            </div>
          ) : (
            <div className="policy-advanced">
              <p className="policy-editor-help">
                {label(locale, "policy.editor.advanced_help")}
              </p>
              {importName ? (
                <p className="policy-import-name">
                  {tr(locale, "policy.imported_from", { file: importName })}
                </p>
              ) : null}
              <textarea
                ref={editorRef}
                id={editorId}
                className={`policy-editor${jsonError ? " invalid" : ""}`}
                value={editor}
                spellCheck={false}
                aria-invalid={Boolean(jsonError)}
                aria-describedby={jsonError ? editorErrorId : undefined}
                onChange={(event) => changeAdvanced(event.target.value)}
              />
              {jsonError ? (
                <p
                  id={editorErrorId}
                  className="policy-json-error"
                  role="alert"
                >
                  <AlertTriangle size={15} />
                  <span>{jsonError}</span>
                  {activeIssuePath ? <code>{activeIssuePath}</code> : null}
                </p>
              ) : null}
            </div>
          )}

          <div className="policy-submit-bar">
            <div>
              {notice ? <span className="policy-notice">{notice}</span> : null}
              {submitError ? (
                <span className="policy-submit-error" role="alert">
                  <AlertTriangle size={15} />
                  {submitError}
                </span>
              ) : null}
              {runReason ? (
                <small
                  id={runReasonId}
                  className="control-reason"
                  role="status"
                >
                  {runReason}
                </small>
              ) : null}
            </div>
            <div className="policy-submit-actions">
              <input
                ref={fileInputRef}
                id={importId}
                className="visually-hidden"
                type="file"
                accept="application/json,.json"
                onChange={(event) => void importFile(event)}
              />
              <button
                className="secondary-button"
                type="button"
                onClick={() => fileInputRef.current?.click()}
              >
                <Upload size={15} />
                {label(locale, "policy.import")}
              </button>
              <button
                className="primary-button"
                type="submit"
                disabled={runDisabled}
                aria-describedby={runReason ? runReasonId : undefined}
              >
                {running ? (
                  <LoaderCircle className="spin" size={16} />
                ) : (
                  <Play size={16} />
                )}
                {running
                  ? label(locale, "policy.running")
                  : label(locale, "policy.run")}
              </button>
            </div>
          </div>
        </form>

        <aside className="policy-result-card" aria-live="polite">
          <div className="policy-section-heading">
            <div>
              <span>{label(locale, "policy.result.eyebrow")}</span>
              <h3>{label(locale, "policy.result.title")}</h3>
            </div>
            {resultDecision ? (
              <span
                className={`policy-decision-pill ${resultDecision.allowed ? "allowed" : "denied"}`}
              >
                {resultDecision.allowed ? (
                  <CheckCircle2 size={15} />
                ) : (
                  <ShieldAlert size={15} />
                )}
                {label(
                  locale,
                  resultDecision.allowed
                    ? "policy.result.allowed"
                    : "policy.result.denied",
                )}
              </span>
            ) : null}
          </div>

          {!resultDecision ? (
            <div className="policy-result-empty">
              <FileJson size={26} />
              <p>{label(locale, "policy.result.empty")}</p>
            </div>
          ) : (
            <div className="policy-result-content">
              <div
                className={`policy-decision ${resultDecision.allowed ? "allowed" : "denied"}`}
              >
                <div className="policy-decision-title">
                  {resultDecision.allowed ? (
                    <CheckCircle2 size={22} />
                  ) : (
                    <ShieldAlert size={22} />
                  )}
                  <strong>
                    {label(
                      locale,
                      resultDecision.allowed
                        ? "policy.result.allowed"
                        : "policy.result.denied",
                    )}
                  </strong>
                  <span>
                    {choice(
                      locale,
                      "policy.choice.reason",
                      resultDecision.reason,
                    )}
                  </span>
                </div>
                <dl className="policy-decision-grid">
                  <div>
                    <dt>{label(locale, "policy.result.code")}</dt>
                    <dd>
                      <code>{resultDecision.code ?? "—"}</code>
                    </dd>
                  </div>
                  <div>
                    <dt>{label(locale, "policy.result.layer")}</dt>
                    <dd>
                      {resultDecision.layer
                        ? choice(
                            locale,
                            "policy.choice.layer",
                            resultDecision.layer,
                          )
                        : "—"}
                    </dd>
                  </div>
                  <div>
                    <dt>{label(locale, "policy.result.rule")}</dt>
                    <dd>
                      <code>{resultDecision.rule_id ?? "—"}</code>
                    </dd>
                  </div>
                  <div>
                    <dt>{label(locale, "policy.result.evaluated_at")}</dt>
                    <dd>
                      {formatTimestamp(
                        locale,
                        resultDecision.evaluated_at_unix_ms,
                      )}
                    </dd>
                  </div>
                  <div>
                    <dt>{label(locale, "policy.result.local_revision")}</dt>
                    <dd>{resultDecision.local_policy_revision}</dd>
                  </div>
                  <div>
                    <dt>{label(locale, "policy.result.acl_revision")}</dt>
                    <dd>{resultDecision.network_acl_revision}</dd>
                  </div>
                  <div>
                    <dt>{label(locale, "policy.result.reauth_satisfied")}</dt>
                    <dd>
                      {label(
                        locale,
                        resultDecision.reauthentication_satisfied
                          ? "policy.result.reauth_satisfied"
                          : "policy.result.reauth_required",
                      )}
                    </dd>
                  </div>
                </dl>
                <div className="policy-grant-row">
                  <span>
                    <strong>
                      {label(locale, "policy.result.expired_grants")}
                    </strong>
                    <code>
                      {grantList(locale, resultDecision.expired_grant_ids)}
                    </code>
                  </span>
                  <span>
                    <strong>
                      {label(locale, "policy.result.revoked_grants")}
                    </strong>
                    <code>
                      {grantList(locale, resultDecision.revoked_grant_ids)}
                    </code>
                  </span>
                  <span>
                    <strong>
                      {label(locale, "policy.result.reauth_grants")}
                    </strong>
                    <code>
                      {grantList(
                        locale,
                        resultDecision.reauthentication_required_grant_ids,
                      )}
                    </code>
                  </span>
                </div>
              </div>

              <section className="policy-impact-notes">
                <div className="policy-subsection-heading">
                  <h4>{label(locale, "policy.deny_priority.title")}</h4>
                  <span>{label(locale, "policy.precedence.description")}</span>
                </div>
                <ul>
                  <li>{label(locale, "policy.deny_priority.local_veto")}</li>
                  <li>
                    {label(locale, "policy.deny_priority.expired_revoked")}
                  </li>
                  <li>{label(locale, "policy.deny_priority.temporary")}</li>
                  <li>{label(locale, "policy.deny_priority.network_scope")}</li>
                  <li>
                    {label(locale, "policy.deny_priority.no_recommendation")}
                  </li>
                </ul>
              </section>

              <section className="policy-validation">
                <div className="policy-subsection-heading">
                  <h4>
                    {tr(locale, "policy.validation.title", {
                      count: result.validation_issues.length,
                    })}
                  </h4>
                </div>
                {result.validation_issues.length === 0 ? (
                  <p className="policy-clean-state">
                    <CheckCircle2 size={15} />
                    {label(locale, "policy.validation.empty")}
                  </p>
                ) : (
                  <ul>
                    {result.validation_issues.map((issue, index) => (
                      <li
                        key={`${issue.path}:${issue.code}:${index}`}
                        className={
                          activeIssuePath === issue.path ? "active" : ""
                        }
                      >
                        <button
                          type="button"
                          onClick={() => focusPath(issue.path)}
                        >
                          <span className="policy-validation-source">
                            {choice(
                              locale,
                              "policy.validation.source",
                              issue.source,
                            )}
                          </span>
                          <code>{issue.path || "/"}</code>
                          <strong>{issue.message}</strong>
                          <small>
                            {issue.code}
                            {issue.related_rule_id
                              ? ` · ${issue.related_rule_id}`
                              : ""}
                          </small>
                          <span className="policy-validation-focus">
                            {label(locale, "policy.validation.focus")}
                          </span>
                        </button>
                      </li>
                    ))}
                  </ul>
                )}
              </section>

              <section className="policy-precedence">
                <div className="policy-subsection-heading">
                  <h4>{label(locale, "policy.precedence.title")}</h4>
                  <span>{label(locale, "policy.precedence.description")}</span>
                </div>
                <ol>
                  {result.precedence.map((entry) => (
                    <li
                      key={entry.precedence}
                      className={entry.triggered ? "triggered" : ""}
                    >
                      <span className="policy-precedence-index">
                        {result.precedence.findIndex(
                          (item) => item.precedence === entry.precedence,
                        ) + 1}
                      </span>
                      <span>
                        {choice(
                          locale,
                          "policy.choice.precedence",
                          entry.precedence,
                        )}
                      </span>
                      {entry.source ? (
                        <small>
                          {choice(locale, "policy.choice.source", entry.source)}
                        </small>
                      ) : null}
                      {entry.rule_id ? <code>{entry.rule_id}</code> : null}
                    </li>
                  ))}
                </ol>
              </section>

              <section className="policy-explanations">
                <div className="policy-subsection-heading">
                  <h4>{label(locale, "policy.explain.title")}</h4>
                  <span>{result.explanations.length}</span>
                </div>
                {result.explanations.length === 0 ? (
                  <p className="policy-clean-state">
                    {label(locale, "policy.explain.empty")}
                  </p>
                ) : (
                  <div className="policy-explanation-list">
                    {result.explanations.map((explanation, index) => (
                      <ExplanationCard
                        key={`${explanation.source_path}:${explanation.rule_id ?? index}`}
                        explanation={explanation}
                        locale={locale}
                        onLocate={focusPath}
                      />
                    ))}
                  </div>
                )}
              </section>
            </div>
          )}
        </aside>
      </div>

      <Dialog.Root open={confirmingApply} onOpenChange={setConfirmingApply}>
        <Dialog.Portal>
          <Dialog.Overlay className="dialog-overlay" />
          <Dialog.Content className="dialog-content confirm-dialog danger-confirm-dialog">
            <div className="dialog-heading">
              <div className="dialog-icon danger">
                <ShieldCheck size={20} />
              </div>
              <div>
                <Dialog.Title>
                  {label(locale, "confirm.apply_policy.title")}
                </Dialog.Title>
                <Dialog.Description>
                  {label(locale, "confirm.apply_policy.consequence")}
                </Dialog.Description>
              </div>
            </div>
            {preview ? (
              <PolicyDangerConfirmationFacts
                locale={locale}
                preview={preview}
                rollback={false}
              />
            ) : null}
            <div className="dialog-actions">
              <Dialog.Close asChild>
                <button className="secondary-button" type="button">
                  {label(locale, "common.cancel")}
                </button>
              </Dialog.Close>
              <div className="configuration-control dialog-control">
                <button
                  id={`${applyDialogId}-confirm`}
                  className="danger-button"
                  type="button"
                  disabled={!canApply || Boolean(applyReason)}
                  aria-describedby={
                    applyReason ? `${applyDialogId}-reason` : undefined
                  }
                  onClick={() => void applyPreview()}
                >
                  {label(locale, "confirm.apply_policy.action")}
                </button>
                {applyReason ? (
                  <small
                    id={`${applyDialogId}-reason`}
                    className="control-reason"
                    role="status"
                  >
                    {applyReason}
                  </small>
                ) : null}
              </div>
            </div>
            <Dialog.Close asChild>
              <button
                className="icon-button dialog-close"
                type="button"
                aria-label={label(locale, "common.close")}
              >
                <X size={17} />
              </button>
            </Dialog.Close>
          </Dialog.Content>
        </Dialog.Portal>
      </Dialog.Root>

      <Dialog.Root
        open={confirmingRollback}
        onOpenChange={setConfirmingRollback}
      >
        <Dialog.Portal>
          <Dialog.Overlay className="dialog-overlay" />
          <Dialog.Content className="dialog-content confirm-dialog danger-confirm-dialog">
            <div className="dialog-heading">
              <div className="dialog-icon danger">
                <RotateCcw size={20} />
              </div>
              <div>
                <Dialog.Title>
                  {label(locale, "confirm.rollback_policy.title")}
                </Dialog.Title>
                <Dialog.Description>
                  {label(locale, "confirm.rollback_policy.consequence")}
                </Dialog.Description>
              </div>
            </div>
            {preview ? (
              <PolicyDangerConfirmationFacts
                locale={locale}
                preview={preview}
                rollback
              />
            ) : null}
            <div className="dialog-actions">
              <Dialog.Close asChild>
                <button className="secondary-button" type="button">
                  {label(locale, "common.cancel")}
                </button>
              </Dialog.Close>
              <div className="configuration-control dialog-control">
                <button
                  id={`${rollbackDialogId}-confirm`}
                  className="danger-button"
                  type="button"
                  disabled={!canRollback || Boolean(rollbackReason)}
                  aria-describedby={
                    rollbackReason ? `${rollbackDialogId}-reason` : undefined
                  }
                  onClick={() => void rollbackPreviewed()}
                >
                  {label(locale, "confirm.rollback_policy.action")}
                </button>
                {rollbackReason ? (
                  <small
                    id={`${rollbackDialogId}-reason`}
                    className="control-reason"
                    role="status"
                  >
                    {rollbackReason}
                  </small>
                ) : null}
              </div>
            </div>
            <Dialog.Close asChild>
              <button
                className="icon-button dialog-close"
                type="button"
                aria-label={label(locale, "common.close")}
              >
                <X size={17} />
              </button>
            </Dialog.Close>
          </Dialog.Content>
        </Dialog.Portal>
      </Dialog.Root>
    </section>
  );
}
