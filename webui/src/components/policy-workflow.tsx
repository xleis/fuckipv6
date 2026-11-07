import {
  AlertTriangle,
  Braces,
  CheckCircle2,
  CircleSlash2,
  FileJson,
  Gavel,
  GitBranch,
  ListChecks,
  LoaderCircle,
  Play,
  Plus,
  RefreshCw,
  ShieldAlert,
  ShieldCheck,
  Trash2,
  Upload,
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
  PolicyRuleExplanation,
  PolicyTestInput,
  PolicyTestResult,
  ResourceKind,
  SubjectSelector,
} from "../../../contracts/typescript/node-contract-v1";
import {
  blockReasonText,
  executionBlockReason,
  label,
  problemText,
  type ActionValues,
} from "../contract/invariants";
import {
  parsePolicyTestInputJson,
  PolicyJsonError,
} from "../contract/policy-test";
import { tr } from "../i18n";
import { pageStateDefinition, type PageStateKind } from "../state-matrix";

export const POLICY_ACTION_IDS = new Set(["test_policy"]);

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
const PERMISSION_INTENTS: PermissionIntent[] = ["read_only", "mutating"];
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
  includeNetworkAcl: boolean;
  aclRevision: string;
  aclIssuedAt: string;
  aclExpiresAt: string;
  aclRules: PolicyRuleDraft[];
  grants: PolicyGrantDraft[];
};

export type PolicyDraftMemo = {
  structured: PolicyStructuredDraft;
  editor: string;
  mode: "structured" | "advanced";
  result: PolicyTestResult | null;
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

function generatedId(prefix: string, index: number): string {
  const seed =
    `${Date.now().toString(36)}${index.toString(36)}${Math.random()
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
  const actor = validId(membership?.node_id, NODE_ID) ||
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
    includeNetworkAcl: false,
    aclRevision: "1",
    aclIssuedAt: String(now),
    aclExpiresAt: String(now + 86_400_000),
    aclRules: [],
    grants: [],
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

export function structuredPolicyInput(
  draft: PolicyStructuredDraft,
): PolicyTestInput {
  const localPolicy: NodeLocalAccessPolicy | undefined = draft.includeLocalPolicy
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
          contributions: csv(veto.contributions) as NodeContributionVeto["contributions"],
        })),
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
    subjectValue:
      subject && "value" in subject ? String(subject.value) : "",
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
    subjectValue:
      subject && "value" in subject ? String(subject.value) : "",
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
    networkScopeKind: localPolicy?.network_scope.kind ?? fallback.networkScopeKind,
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
    includeNetworkAcl: Boolean(networkAcl),
    aclRevision: String(networkAcl?.revision ?? fallback.aclRevision),
    aclIssuedAt: String(
      networkAcl?.issued_at_unix_ms ?? fallback.aclIssuedAt,
    ),
    aclExpiresAt: String(
      networkAcl?.expires_at_unix_ms ?? fallback.aclExpiresAt,
    ),
    aclRules: networkAcl?.rules?.map(ruleDraftFromRule) ?? [],
    grants: networkAcl?.grants?.map(grantDraftFromGrant) ?? [],
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
        <Field id={`${prefix}-id`} labelText={label(locale, "policy.field.rule_id")}>
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
        <Field id={`${prefix}-id`} labelText={label(locale, "policy.field.grant_id")}>
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
        <span>{label(locale, "policy.field.requires_recent_authentication")}</span>
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
        <Field id={`${prefix}-id`} labelText={label(locale, "policy.field.veto_id")}>
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
          <span>{choice(locale, "policy.choice.precedence", explanation.precedence)}</span>
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
            <button type="button" onClick={() => onLocate(explanation.source_path)}>
              <code>{explanation.source_path}</code>
            </button>
          </dd>
        </div>
        <div>
          <dt>{label(locale, "policy.explain.rule")}</dt>
          <dd><code>{explanation.rule_id ?? "—"}</code></dd>
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
          <dd>{valueList(locale, "policy.choice.subject", explanation.subjects?.map(encodedSubject))}</dd>
        </div>
        <div>
          <dt>{label(locale, "policy.explain.resources")}</dt>
          <dd>{valueList(locale, "policy.choice.resource", explanation.resources)}</dd>
        </div>
        <div>
          <dt>{label(locale, "policy.explain.actions")}</dt>
          <dd>{valueList(locale, "policy.choice.action", explanation.actions)}</dd>
        </div>
        <div>
          <dt>{label(locale, "policy.explain.conditions")}</dt>
          <dd>
            {explanation.conditions?.length
              ? explanation.conditions
                  .map((condition) => choice(locale, "policy.choice.condition", condition.kind))
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
  const titleId = useId();
  const editorId = useId();
  const importId = useId();
  const editorRef = useRef<HTMLTextAreaElement>(null);
  const fileInputRef = useRef<HTMLInputElement>(null);
  const [structured, setStructured] = useState<PolicyStructuredDraft>(
    draft?.structured ?? defaultStructuredDraft(snapshot, selectedNetwork),
  );
  const [editor, setEditor] = useState(
    draft?.editor ?? prettyPolicy(structuredPolicyInput(structured)),
  );
  const [mode, setMode] = useState<EditorMode>(draft?.mode ?? "structured");
  const [result, setResult] = useState<PolicyTestResult | null>(
    draft?.result ?? null,
  );
  const [importName, setImportName] = useState(draft?.importName ?? "");
  const [activeIssuePath, setActiveIssuePath] = useState(
    draft?.activeIssuePath ?? "",
  );
  const [jsonError, setJsonError] = useState("");
  const [submitError, setSubmitError] = useState("");
  const [notice, setNotice] = useState("");
  const [running, setRunning] = useState(false);

  const pageBlocked = interactionBlocked
    ? pageStateDefinition(interactionBlocked)
    : null;
  const executionBlocked = action
    ? executionBlockReason(action, snapshot, selectedNetwork)
    : "backend_unavailable";
  const pending = Boolean(pendingAction) || running;
  const runReason =
    (pending && label(locale, "common.control_pending")) ||
    (pageBlocked && label(locale, pageBlocked.titleKey)) ||
    blockReasonText(executionBlocked, locale) ||
    (!action && label(locale, "blocked.backend_unavailable")) ||
    "";

  useEffect(() => {
    onDraftChange?.({
      structured,
      editor,
      mode,
      result,
      importName,
      activeIssuePath,
    });
  }, [
    activeIssuePath,
    editor,
    importName,
    mode,
    onDraftChange,
    result,
    structured,
  ]);

  function clearTransientState(): void {
    setResult(null);
    setJsonError("");
    setSubmitError("");
    setNotice("");
    setActiveIssuePath("");
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
        emptyRule(structured[key].length, source === "local" ? "deny" : "allow"),
      ],
    });
  }

  function selectMode(nextMode: EditorMode): void {
    if (nextMode === mode) return;
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

  async function importFile(event: ChangeEvent<HTMLInputElement>): Promise<void> {
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
    if (!action || executionBlocked || pageBlocked || pending) return;
    clearTransientState();
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
            <div className="policy-mode-switch" aria-label={label(locale, "policy.mode.label")}>
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
              <section className="policy-form-section" aria-labelledby={`${titleId}-context`}>
                <h4 id={`${titleId}-context`}>
                  <ListChecks size={16} />
                  {label(locale, "policy.section.context")}
                </h4>
                <div className="policy-context-grid">
                  <Field id="policy-network-id" labelText={label(locale, "policy.field.network_id")}>
                    <input
                      id="policy-network-id"
                      value={structured.networkId}
                      spellCheck={false}
                      placeholder="net_..."
                      onChange={(event) =>
                        updateStructured({ ...structured, networkId: event.target.value })
                      }
                    />
                  </Field>
                  <Field id="policy-actor" labelText={label(locale, "policy.field.actor")}>
                    <input
                      id="policy-actor"
                      value={structured.actor}
                      spellCheck={false}
                      placeholder="node_..."
                      onChange={(event) =>
                        updateStructured({ ...structured, actor: event.target.value })
                      }
                    />
                  </Field>
                  <Field id="policy-role" labelText={label(locale, "policy.field.role")}>
                    <select
                      id="policy-role"
                      value={structured.role}
                      onChange={(event) =>
                        updateStructured({
                          ...structured,
                          role: event.target.value as NetworkRole,
                          emergencyCoordinator: event.target.value === "temporary_coordinator",
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
                  <Field id="policy-groups" labelText={label(locale, "policy.field.groups")}>
                    <input
                      id="policy-groups"
                      value={structured.groups}
                      placeholder="group_..."
                      onChange={(event) =>
                        updateStructured({ ...structured, groups: event.target.value })
                      }
                    />
                  </Field>
                  <Field id="policy-now" labelText={label(locale, "policy.field.now_unix_ms")}>
                    <input
                      id="policy-now"
                      inputMode="numeric"
                      value={structured.nowUnixMs}
                      onChange={(event) =>
                        updateStructured({ ...structured, nowUnixMs: event.target.value })
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
                    <span>{label(locale, "policy.field.membership_active")}</span>
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
                    <span>{label(locale, "policy.field.recent_reauthentication")}</span>
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
                    <span>{label(locale, "policy.field.emergency_coordinator")}</span>
                  </label>
                </div>
              </section>

              <section className="policy-form-section" aria-labelledby={`${titleId}-request`}>
                <h4 id={`${titleId}-request`}>
                  <Gavel size={16} />
                  {label(locale, "policy.section.request")}
                </h4>
                <div className="policy-context-grid">
                  <Field id="policy-action" labelText={label(locale, "policy.field.action")}>
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
                  <Field id="policy-resource" labelText={label(locale, "policy.field.resource")}>
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
                    labelText={label(locale, "policy.field.resource_network_id")}
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
                  <Field id="policy-object-id" labelText={label(locale, "policy.field.object_id")}>
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
                  <Field id="policy-intent" labelText={label(locale, "policy.field.intent")}>
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

              <section className="policy-form-section" aria-labelledby={`${titleId}-local`}>
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
                      <Field id="policy-local-revision" labelText={label(locale, "policy.field.local_revision")}>
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
                      <Field id="policy-local-issued" labelText={label(locale, "policy.field.issued_at_unix_ms")}>
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
                      <Field id="policy-local-expires" labelText={label(locale, "policy.field.expires_at_unix_ms")}>
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
                      <Field id="policy-local-scope" labelText={label(locale, "policy.field.network_scope")}>
                        <select
                          id="policy-local-scope"
                          value={structured.networkScopeKind}
                          onChange={(event) =>
                            updateStructured({
                              ...structured,
                              networkScopeKind: event.target.value as "all" | "only",
                            })
                          }
                        >
                          <option value="all">{label(locale, "policy.scope.all")}</option>
                          <option value="only">{label(locale, "policy.scope.only")}</option>
                        </select>
                      </Field>
                      {structured.networkScopeKind === "only" ? (
                        <Field id="policy-local-network-ids" labelText={label(locale, "policy.field.network_ids")}>
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
                      <h5>{label(locale, "policy.section.rules")}</h5>
                      <button className="secondary-button compact" type="button" onClick={() => addRule("local")}>
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
                      <button className="secondary-button compact" type="button" onClick={() => updateStructured({
                        ...structured,
                        vetoes: [...structured.vetoes, emptyVeto(structured.vetoes.length)],
                      })}>
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
                              vetoes: structured.vetoes.map((entry, vetoIndex) =>
                                vetoIndex === index ? next : entry,
                              ),
                            })
                          }
                          onRemove={() =>
                            updateStructured({
                              ...structured,
                              vetoes: structured.vetoes.filter((_, vetoIndex) => vetoIndex !== index),
                            })
                          }
                        />
                      ))}
                    </div>
                  </>
                ) : null}
              </section>

              <section className="policy-form-section" aria-labelledby={`${titleId}-acl`}>
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
                      <Field id="policy-acl-revision" labelText={label(locale, "policy.field.acl_revision")}>
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
                      <Field id="policy-acl-issued" labelText={label(locale, "policy.field.issued_at_unix_ms")}>
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
                      <Field id="policy-acl-expires" labelText={label(locale, "policy.field.expires_at_unix_ms")}>
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
                      <h5>{label(locale, "policy.section.rules")}</h5>
                      <button className="secondary-button compact" type="button" onClick={() => addRule("acl")}>
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
                      <button className="secondary-button compact" type="button" onClick={() => updateStructured({
                        ...structured,
                        grants: [
                          ...structured.grants,
                          emptyGrant(structured.grants.length, Number(structured.nowUnixMs)),
                        ],
                      })}>
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
                              grants: structured.grants.map((entry, grantIndex) =>
                                grantIndex === index ? next : entry,
                              ),
                            })
                          }
                          onRemove={() =>
                            updateStructured({
                              ...structured,
                              grants: structured.grants.filter((_, grantIndex) => grantIndex !== index),
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
                <p id={editorErrorId} className="policy-json-error" role="alert">
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
                <small className="control-reason" role="status">{runReason}</small>
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
              <span className={`policy-decision-pill ${resultDecision.allowed ? "allowed" : "denied"}`}>
                {resultDecision.allowed ? <CheckCircle2 size={15} /> : <ShieldAlert size={15} />}
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
              <div className={`policy-decision ${resultDecision.allowed ? "allowed" : "denied"}`}>
                <div className="policy-decision-title">
                  {resultDecision.allowed ? <CheckCircle2 size={22} /> : <ShieldAlert size={22} />}
                  <strong>
                    {label(
                      locale,
                      resultDecision.allowed
                        ? "policy.result.allowed"
                        : "policy.result.denied",
                    )}
                  </strong>
                  <span>{choice(locale, "policy.choice.reason", resultDecision.reason)}</span>
                </div>
                <dl className="policy-decision-grid">
                  <div>
                    <dt>{label(locale, "policy.result.code")}</dt>
                    <dd><code>{resultDecision.code ?? "—"}</code></dd>
                  </div>
                  <div>
                    <dt>{label(locale, "policy.result.layer")}</dt>
                    <dd>
                      {resultDecision.layer
                        ? choice(locale, "policy.choice.layer", resultDecision.layer)
                        : "—"}
                    </dd>
                  </div>
                  <div>
                    <dt>{label(locale, "policy.result.rule")}</dt>
                    <dd><code>{resultDecision.rule_id ?? "—"}</code></dd>
                  </div>
                  <div>
                    <dt>{label(locale, "policy.result.evaluated_at")}</dt>
                    <dd>{formatTimestamp(locale, resultDecision.evaluated_at_unix_ms)}</dd>
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
                    <strong>{label(locale, "policy.result.expired_grants")}</strong>
                    <code>{grantList(locale, resultDecision.expired_grant_ids)}</code>
                  </span>
                  <span>
                    <strong>{label(locale, "policy.result.revoked_grants")}</strong>
                    <code>{grantList(locale, resultDecision.revoked_grant_ids)}</code>
                  </span>
                  <span>
                    <strong>{label(locale, "policy.result.reauth_grants")}</strong>
                    <code>{grantList(locale, resultDecision.reauthentication_required_grant_ids)}</code>
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
                  <li>{label(locale, "policy.deny_priority.expired_revoked")}</li>
                  <li>{label(locale, "policy.deny_priority.temporary")}</li>
                  <li>{label(locale, "policy.deny_priority.network_scope")}</li>
                  <li>{label(locale, "policy.deny_priority.no_recommendation")}</li>
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
                        className={activeIssuePath === issue.path ? "active" : ""}
                      >
                        <button type="button" onClick={() => focusPath(issue.path)}>
                          <span className="policy-validation-source">
                            {choice(locale, "policy.validation.source", issue.source)}
                          </span>
                          <code>{issue.path || "/"}</code>
                          <strong>{issue.message}</strong>
                          <small>
                            {issue.code}
                            {issue.related_rule_id ? ` · ${issue.related_rule_id}` : ""}
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
                        {result.precedence.findIndex((item) => item.precedence === entry.precedence) + 1}
                      </span>
                      <span>
                        {choice(locale, "policy.choice.precedence", entry.precedence)}
                      </span>
                      {entry.source ? (
                        <small>{choice(locale, "policy.choice.source", entry.source)}</small>
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
    </section>
  );
}
