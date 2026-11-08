import * as Dialog from "@radix-ui/react-dialog";
import {
  AlertTriangle,
  ArrowDown,
  ArrowUp,
  CheckCircle2,
  ExternalLink,
  FlaskConical,
  GripVertical,
  History,
  LoaderCircle,
  Plus,
  RotateCcw,
  Search,
  ShieldAlert,
  Trash2,
  X,
} from "lucide-react";
import {
  useEffect,
  useId,
  useMemo,
  useState,
  type DragEvent,
  type JSX,
  type KeyboardEvent,
} from "react";
import type {
  ApplicationIdentity,
  NetworkId,
  NodeCommandOutcome,
  NodeSnapshot,
  NodeId,
  NodeUiAction,
  PortRange,
  TrafficAction,
  TrafficMatch,
  TrafficPolicy,
  TrafficPolicyApplyResult,
  TrafficPolicyDraft,
  TrafficPolicyHistoryEntry,
  TrafficPolicyHistorySnapshot,
  TrafficPolicyPreview,
  TrafficPolicyRollbackResult,
  TrafficPolicyTestComparison,
  TrafficPolicyTestDecision,
  TrafficPolicyTestInput,
  TrafficProtocol,
  TrafficRoutingPageModel,
  TrafficRoutingTarget,
  TrafficRule,
  TrafficRuleId,
} from "../../../contracts/typescript/node-contract-v1";
import {
  blockReasonText,
  executionBlockReason,
  label,
  outcomeSummary,
  problemText,
  type ActionValues,
} from "../contract/invariants";
import { tr } from "../i18n";
import { pageStateDefinition, type PageStateKind } from "../state-matrix";

export const TRAFFIC_POLICY_ACTION_IDS = new Set([
  "get_traffic_policy",
  "preview_traffic_policy",
  "preview_traffic_policy_rollback",
  "apply_traffic_policy",
  "rollback_traffic_policy",
  "get_traffic_policy_history",
  "test_traffic_policy",
]);

export type TrafficRoutingTestDraft = {
  networkId: string;
  domain: string;
  destinationIp: string;
  destinationPort: string;
  protocol: TrafficProtocol;
  applicationKind: ApplicationIdentity["kind"];
  applicationValue: string;
};

export type TrafficPolicyDraftMemo = {
  baseRevision: number;
  rules: TrafficRule[];
  test: TrafficRoutingTestDraft;
  testResult: TrafficPolicyTestComparison | null;
  preview: TrafficPolicyPreview | null;
  applied: TrafficPolicyApplyResult | TrafficPolicyRollbackResult | null;
  history: TrafficPolicyHistorySnapshot | null;
};

type TrafficRoutingWorkflowProps = {
  actions: NodeUiAction[];
  snapshot: NodeSnapshot;
  selectedNetwork: string;
  locale: Locale;
  pendingAction: string | null;
  interactionBlocked?: PageStateKind | null;
  draft?: TrafficPolicyDraftMemo | null;
  onDraftChange?(draft: TrafficPolicyDraftMemo): void;
  onSubmit(
    action: NodeUiAction,
    values: ActionValues,
  ): Promise<NodeCommandOutcome>;
  onNavigate(pageId: string, networkName?: string): void;
};

type BusyState = "test" | "preview" | "apply" | "history" | "rollback";
type RollbackConfirmation = {
  targetRevision: number;
  preview: TrafficPolicyPreview;
};

const PROTOCOLS: TrafficProtocol[] = ["tcp", "udp", "icmp"];

function cloneRules(page: TrafficRoutingPageModel): TrafficRule[] {
  return page.rules.map((rule) => ({
    rule_id: rule.rule_id,
    priority: rule.priority,
    enabled: rule.enabled,
    description: rule.description,
    matcher: { ...rule.matcher },
    action: { ...rule.action },
  }));
}

function normalizeRules(rules: TrafficRule[]): TrafficRule[] {
  return rules.map((rule, index) => ({ ...rule, priority: index + 1 }));
}

function generatedRuleId(): TrafficRuleId {
  const random = globalThis.crypto?.getRandomValues
    ? Array.from(globalThis.crypto.getRandomValues(new Uint8Array(32)))
        .map((value) => value.toString(16).padStart(2, "0"))
        .join("")
    : `${Date.now().toString(36)}${Math.random().toString(36).slice(2)}`;
  const suffix = random
    .replace(/[^A-Za-z0-9_-]/g, "")
    .padEnd(43, "A")
    .slice(0, 43);
  return `rule_${suffix}`;
}

function initialTestDraft(
  snapshot: NodeSnapshot,
  selectedNetwork: string,
): TrafficRoutingTestDraft {
  const membership =
    snapshot.memberships?.find(
      (entry) => entry.network_name === selectedNetwork,
    ) ?? snapshot.memberships?.[0];
  return {
    networkId: membership?.network_id ?? "",
    domain: "",
    destinationIp: "",
    destinationPort: "",
    protocol: "tcp",
    applicationKind: "unknown",
    applicationValue: "",
  };
}

function initialDraft(
  page: TrafficRoutingPageModel,
  snapshot: NodeSnapshot,
  selectedNetwork: string,
): TrafficPolicyDraftMemo {
  return {
    baseRevision: page.authority_revision,
    rules: cloneRules(page),
    test: initialTestDraft(snapshot, selectedNetwork),
    testResult: null,
    preview: null,
    applied: null,
    history: null,
  };
}

function rulesEqual(left: TrafficRule[], right: TrafficRule[]): boolean {
  return JSON.stringify(normalizeRules(left)) === JSON.stringify(normalizeRules(right));
}

function currentPolicy(page: TrafficRoutingPageModel): TrafficPolicy {
  return {
    node_id: page.node_id,
    revision: page.authority_revision,
    default_policy: {
      action: { ...page.default_policy.action },
      confirmed_by: page.default_policy.confirmed_by,
      confirmed_at_unix_ms: page.default_policy.confirmed_at_unix_ms,
    },
    failure_policy: page.failure_policy,
    rules: cloneRules(page),
  };
}

function candidatePolicy(
  page: TrafficRoutingPageModel,
  rules: TrafficRule[],
): TrafficPolicy {
  return {
    node_id: page.node_id,
    revision: page.authority_revision + 1,
    default_policy: {
      action: { ...page.default_policy.action },
      confirmed_by: page.default_policy.confirmed_by,
      confirmed_at_unix_ms: page.default_policy.confirmed_at_unix_ms,
    },
    failure_policy: page.failure_policy,
    rules: normalizeRules(rules),
  };
}

function draftPolicy(
  page: TrafficRoutingPageModel,
  rules: TrafficRule[],
): TrafficPolicyDraft {
  return {
    node_id: page.node_id,
    default_policy: {
      action: { ...page.default_policy.action },
      confirmed_by: page.default_policy.confirmed_by,
      confirmed_at_unix_ms: page.default_policy.confirmed_at_unix_ms,
    },
    failure_policy: page.failure_policy,
    rules: normalizeRules(rules),
  };
}

function actionSelectValue(action: TrafficAction): string {
  if (action.kind === "network" || action.kind === "exit") {
    return `${action.kind}:${action.value}`;
  }
  return action.kind;
}

function actionFromValue(
  value: string,
  targets: TrafficRoutingTarget[],
): TrafficAction | null {
  if (value === "direct" || value === "reject") return { kind: value };
  const separator = value.indexOf(":");
  if (separator < 0) return null;
  const kind = value.slice(0, separator);
  const id = value.slice(separator + 1);
  const target = targets.find(
    (candidate) => candidate.kind === kind && candidate.id === id,
  );
  if (!target) return null;
  if (target.kind === "network") {
    return { kind: "network", value: id as NetworkId };
  }
  if (target.kind === "exit") {
    return { kind: "exit", value: id as NodeId };
  }
  return { kind: target.kind };
}

function targetForAction(
  action: TrafficAction,
  targets: TrafficRoutingTarget[],
): TrafficRoutingTarget | undefined {
  if (action.kind === "direct" || action.kind === "reject") {
    return targets.find((target) => target.kind === action.kind);
  }
  return targets.find(
    (target) => target.kind === action.kind && target.id === action.value,
  );
}

function targetLabel(
  target: TrafficRoutingTarget | undefined,
  action: TrafficAction,
): string {
  if (target) return target.label;
  if (action.kind === "network" || action.kind === "exit") {
    return `${action.kind}:${action.value}`;
  }
  return action.kind;
}

function splitValues(value: string): string[] {
  return value
    .split(/[\n,]/)
    .map((entry) => entry.trim())
    .filter(Boolean);
}

function valuesText(values: string[] | undefined): string {
  return (values ?? []).join(", ");
}

function portText(range: PortRange): string {
  return range.start === range.end
    ? String(range.start)
    : `${range.start}-${range.end}`;
}

function parsePorts(value: string): {
  ranges: PortRange[];
  error: string;
} {
  const ranges: PortRange[] = [];
  for (const token of splitValues(value)) {
    const match = /^(\d+)(?:-(\d+))?$/.exec(token);
    if (!match) return { ranges: [], error: `invalid_port:${token}` };
    const start = Number(match[1]);
    const end = match[2] ? Number(match[2]) : start;
    if (start < 1 || start > 65_535 || end < start || end > 65_535) {
      return { ranges: [], error: `invalid_port:${token}` };
    }
    ranges.push({ start, end });
  }
  return { ranges, error: "" };
}

function applicationChip(application: ApplicationIdentity): string {
  return application.kind === "unknown"
    ? "unknown"
    : `${application.kind}:${application.value}`;
}

function policyActionLabel(
  action: TrafficAction,
  page: TrafficRoutingPageModel,
): string {
  return targetLabel(targetForAction(action, page.targets), action);
}

function testDecisionText(
  decision: TrafficPolicyTestDecision,
  page: TrafficRoutingPageModel,
): string {
  const rule =
    decision.visibility === "full"
      ? decision.matched_rule ?? "default"
      : "summary";
  return `${policyActionLabel(decision.action, page)} · ${rule}`;
}

function rejectedMessage(
  outcome: NodeCommandOutcome,
  locale: Locale,
): string {
  return outcome.status === "rejected"
    ? problemText(outcome.data, locale)
    : "";
}

function reasonText(
  reason: ReturnType<typeof executionBlockReason>,
  locale: Locale,
): string {
  return blockReasonText(reason, locale);
}

function rollbackTargetRevision(entry: TrafficPolicyHistoryEntry): number {
  return entry.applied_revision ?? entry.target_revision ?? entry.candidate_revision;
}

function ListEditor({
  disabled,
  labelKey,
  locale,
  placeholder,
  values,
  onChange,
}: {
  disabled: boolean;
  labelKey: string;
  locale: Locale;
  placeholder: string;
  values: string[] | undefined;
  onChange(values: string[]): void;
}): JSX.Element {
  const external = valuesText(values);
  const [editing, setEditing] = useState({
    external,
    value: external,
  });
  const value = editing.external === external ? editing.value : external;

  return (
    <label className="policy-field">
      <span>{label(locale, labelKey)}</span>
      <input
        disabled={disabled}
        value={value}
        placeholder={placeholder}
        onChange={(event) => {
          const raw = event.target.value;
          const next = splitValues(raw);
          setEditing({ external: valuesText(next), value: raw });
          onChange(next);
        }}
        onBlur={() => setEditing({ external, value: external })}
      />
    </label>
  );
}

function MatcherEditor({
  matcher,
  disabled,
  locale,
  onChange,
  onValidityChange,
}: {
  matcher: TrafficMatch;
  disabled: boolean;
  locale: Locale;
  onChange(matcher: TrafficMatch): void;
  onValidityChange(error: string): void;
}) {
  const id = useId();
  const portKey = JSON.stringify(matcher.destination_ports ?? []);
  const [portsState, setPortsState] = useState({
    key: portKey,
    value: valuesText((matcher.destination_ports ?? []).map(portText)),
    error: "",
  });
  const ports =
    portsState.key === portKey
      ? portsState.value
      : valuesText((matcher.destination_ports ?? []).map(portText));
  const portError = portsState.key === portKey ? portsState.error : "";
  const [applicationKind, setApplicationKind] = useState<
    "package_id" | "executable_path"
  >("package_id");
  const [applicationValue, setApplicationValue] = useState("");

  function update(next: TrafficMatch): void {
    onChange(next);
  }


  function toggleProtocol(protocol: TrafficProtocol, checked: boolean): void {
    const selected = new Set(matcher.protocols ?? []);
    if (checked) selected.add(protocol);
    else selected.delete(protocol);
    update({ ...matcher, protocols: PROTOCOLS.filter((item) => selected.has(item)) });
  }

  function addApplication(): void {
    const value = applicationValue.trim();
    if (!value) return;
    const applications = [
      ...(matcher.applications ?? []),
      { kind: applicationKind, value },
    ];
    update({ ...matcher, applications });
    setApplicationValue("");
  }

  function removeApplication(index: number): void {
    const applications = (matcher.applications ?? []).filter(
      (_, candidate) => candidate !== index,
    );
    update({ ...matcher, applications });
  }

  return (
    <div className="traffic-matcher-editor">
      <div className="traffic-match-grid">
        <ListEditor
          disabled={disabled}
          labelKey="traffic.routing.match.exact_domains"
          locale={locale}
          placeholder="example.com, api.example.net"
          values={matcher.exact_domains}
          onChange={(values) => update({ ...matcher, exact_domains: values })}
        />
        <ListEditor
          disabled={disabled}
          labelKey="traffic.routing.match.domain_suffixes"
          locale={locale}
          placeholder="example.com, corp.example"
          values={matcher.domain_suffixes}
          onChange={(values) => update({ ...matcher, domain_suffixes: values })}
        />
        <ListEditor
          disabled={disabled}
          labelKey="traffic.routing.match.destination_cidrs"
          locale={locale}
          placeholder="203.0.113.0/24, 2001:db8::/32"
          values={matcher.destination_cidrs}
          onChange={(values) => update({ ...matcher, destination_cidrs: values })}
        />
        <label className="policy-field">
          <span>{label(locale, "traffic.routing.match.destination_ports")}</span>
          <input
            id={`${id}-ports`}
            disabled={disabled}
            value={ports}
            placeholder="443, 8000-8080"
            aria-invalid={Boolean(portError)}
            aria-describedby={portError ? `${id}-ports-error` : undefined}
            onChange={(event) => {
              const value = event.target.value;
              const parsed = parsePorts(value);
              setPortsState({
                key: portKey,
                value,
                error: parsed.error,
              });
              onValidityChange(parsed.error);
              if (!parsed.error) {
                update({ ...matcher, destination_ports: parsed.ranges });
              }
            }}
          />
          {portError ? (
            <small className="form-error" id={`${id}-ports-error`}>
              {tr(locale, "traffic.routing.match.invalid_port", {
                value: portError.slice("invalid_port:".length),
              })}
            </small>
          ) : null}
        </label>
      </div>

      <fieldset className="traffic-protocol-fieldset" disabled={disabled}>
        <legend>{label(locale, "traffic.routing.match.protocols")}</legend>
        {PROTOCOLS.map((protocol) => (
          <label className="policy-check compact" key={protocol}>
            <input
              type="checkbox"
              checked={(matcher.protocols ?? []).includes(protocol)}
              onChange={(event) => toggleProtocol(protocol, event.target.checked)}
            />
            <span>{protocol.toUpperCase()}</span>
          </label>
        ))}
      </fieldset>

      <div className="traffic-applications">
        <div className="traffic-applications-heading">
          <strong>{label(locale, "traffic.routing.match.applications")}</strong>
          <span>{(matcher.applications ?? []).length}</span>
        </div>
        <div className="traffic-application-chips">
          {(matcher.applications ?? []).length ? (
            (matcher.applications ?? []).map((application, index) => (
              <span className="traffic-chip" key={`${applicationChip(application)}:${index}`}>
                <code>{applicationChip(application)}</code>
                <button
                  type="button"
                  disabled={disabled}
                  aria-label={`${label(locale, "traffic.routing.match.remove_application")} ${applicationChip(application)}`}
                  onClick={() => removeApplication(index)}
                >
                  <X size={12} />
                </button>
              </span>
            ))
          ) : (
            <span className="traffic-muted">
              {label(locale, "traffic.routing.match.any_application")}
            </span>
          )}
        </div>
        <div className="traffic-application-add">
          <select
            disabled={disabled}
            value={applicationKind}
            aria-label={label(locale, "traffic.routing.match.application_kind")}
            onChange={(event) =>
              setApplicationKind(
                event.target.value as "package_id" | "executable_path",
              )
            }
          >
            <option value="package_id">package_id</option>
            <option value="executable_path">executable_path</option>
          </select>
          <input
            disabled={disabled}
            value={applicationValue}
            aria-label={label(locale, "traffic.routing.match.application_value")}
            placeholder={
              applicationKind === "executable_path"
                ? "/usr/bin/browser"
                : "org.example.browser"
            }
            onChange={(event) => setApplicationValue(event.target.value)}
          />
          <button
            className="secondary-button compact"
            type="button"
            disabled={disabled || !applicationValue.trim()}
            onClick={addApplication}
          >
            <Plus size={13} />
            {label(locale, "traffic.routing.match.add_application")}
          </button>
        </div>
      </div>
    </div>
  );
}

export function TrafficRoutingWorkflow({
  actions,
  snapshot,
  selectedNetwork,
  locale,
  pendingAction,
  interactionBlocked = null,
  draft,
  onDraftChange,
  onSubmit,
  onNavigate,
}: TrafficRoutingWorkflowProps): JSX.Element {
  const page = snapshot.traffic_routing_page ?? null;
  const titleId = useId();
  const testRegionId = useId();
  const [state, setState] = useState<TrafficPolicyDraftMemo | null>(
    page ? (draft ?? initialDraft(page, snapshot, selectedNetwork)) : null,
  );
  const [busy, setBusy] = useState<BusyState | null>(null);
  const [notice, setNotice] = useState("");
  const [error, setError] = useState("");
  const [targetQuery, setTargetQuery] = useState("");
  const [pendingDelete, setPendingDelete] = useState<string | null>(null);
  const [confirmApply, setConfirmApply] = useState(false);
  const [matcherErrors, setMatcherErrors] = useState<Record<string, string>>({});
  const [rollbackConfirmation, setRollbackConfirmation] =
    useState<RollbackConfirmation | null>(null);

  const previewAction = actions.find(
    (action) => action.id === "preview_traffic_policy",
  );
  const previewRollbackAction = actions.find(
    (action) => action.id === "preview_traffic_policy_rollback",
  );
  const applyAction = actions.find(
    (action) => action.id === "apply_traffic_policy",
  );
  const rollbackAction = actions.find(
    (action) => action.id === "rollback_traffic_policy",
  );
  const historyAction = actions.find(
    (action) => action.id === "get_traffic_policy_history",
  );
  const testAction = actions.find(
    (action) => action.id === "test_traffic_policy",
  );

  useEffect(() => {
    if (state && onDraftChange) onDraftChange(state);
  }, [onDraftChange, state]);

  const targets = useMemo(() => page?.targets ?? [], [page?.targets]);
  const targetResults = useMemo(() => {
    const query = targetQuery.trim().toLocaleLowerCase();
    if (!query) return [];
    return targets
      .filter((target) =>
        (target.search_terms ?? []).some((term) =>
          term.toLocaleLowerCase().includes(query),
        ),
      )
      .slice(0, 20);
  }, [targetQuery, targets]);

  const pageBlocked = interactionBlocked
    ? pageStateDefinition(interactionBlocked)
    : null;
  const pending = Boolean(pendingAction) || busy !== null;
  const unavailableReason = label(locale, "blocked.backend_unavailable");
  const pageReason = pageBlocked ? label(locale, pageBlocked.titleKey) : "";
  const pendingReason = pending ? label(locale, "common.control_pending") : "";
  const firstReason = (...reasons: Array<string | false | null>): string =>
    reasons.find((reason): reason is string => Boolean(reason)) ?? "";
  const actionReason = (
    action: NodeUiAction | undefined,
    extra: Array<string | false | null> = [],
  ): string =>
    firstReason(
      pendingReason,
      pageReason,
      action
        ? reasonText(
            executionBlockReason(action, snapshot, selectedNetwork),
            locale,
          )
        : unavailableReason,
      ...extra,
    );

  if (!page || !state) {
    return (
      <section className="traffic-routing-workspace" aria-labelledby={titleId}>
        <header className="policy-heading">
          <div className="policy-heading-mark" aria-hidden="true">
            <ShieldAlert size={22} />
          </div>
          <div className="policy-heading-copy">
            <span>{label(locale, "traffic.routing.eyebrow")}</span>
            <h2 id={titleId}>{label(locale, "traffic.routing.title")}</h2>
            <p>{label(locale, "traffic.routing.description")}</p>
          </div>
        </header>
        <section className="policy-result-empty" data-routing-authority="missing">
          <AlertTriangle size={24} />
          <strong>{label(locale, "traffic.routing.authority.unavailable")}</strong>
          <p>{label(locale, "traffic.routing.authority.unavailable_detail")}</p>
        </section>
      </section>
    );
  }

  const activePage = page;
  const activeState = state;
  const dirty = !rulesEqual(activeState.rules, cloneRules(activePage));
  const staleDraft = activeState.baseRevision !== activePage.authority_revision;
  const matcherValidationReason = activeState.rules.some(
    (rule) => matcherErrors[rule.rule_id],
  )
    ? label(locale, "traffic.routing.match.fix_errors")
    : "";
  const authoringBlocked = !activePage.can_edit
    ? label(locale, "traffic.routing.read_only")
    : activePage.workflow.status === "applying"
      ? label(locale, "traffic.routing.state.applying")
      : staleDraft
        ? label(locale, "traffic.routing.state.stale_draft")
        : pageReason;
  const testBlockedReason = actionReason(testAction, [
    !activeState.test.networkId &&
      label(locale, "traffic.routing.test.network.required"),
    !activeState.test.destinationIp &&
      label(locale, "traffic.routing.test.destination.required"),
    !activeState.test.destinationPort &&
      label(locale, "traffic.routing.test.port.required"),
    staleDraft && label(locale, "traffic.routing.state.stale_draft"),
    matcherValidationReason,
  ]);
  const previewBlockedReason = actionReason(previewAction, [
    authoringBlocked,
    matcherValidationReason,
    !activeState.testResult &&
      label(locale, "traffic.routing.preview.test_required"),
  ]);
  const stalePreview = Boolean(
    activeState.preview &&
      activeState.preview.expected_revision !== activePage.authority_revision,
  );
  const previewCurrent = Boolean(
    activeState.preview &&
      activeState.preview.expected_revision === activePage.authority_revision,
  );
  const applyBlockedReason = actionReason(applyAction, [
    authoringBlocked,
    matcherValidationReason,
    !activeState.preview &&
      label(locale, "traffic.routing.preview.required"),
    stalePreview && label(locale, "traffic.routing.preview.stale"),
    Boolean(activeState.preview?.revision_conflict) &&
      label(locale, "traffic.routing.preview.conflict"),
    Boolean(activeState.preview && !activeState.preview.can_apply) &&
      label(locale, "traffic.routing.preview.blocked"),
    Boolean(activeState.preview && !activeState.preview.has_changes) &&
      label(locale, "traffic.routing.preview.no_changes"),
  ]);
  const historyBlockedReason = actionReason(historyAction);
  const rollbackBlockedReason = actionReason(rollbackAction, [
    authoringBlocked,
    !rollbackConfirmation &&
      label(locale, "traffic.routing.rollback.preview_required"),
  ]);
  const rollbackPreviewBlockedReason = actionReason(previewRollbackAction, [
    authoringBlocked,
  ]);

  const updateState = (next: TrafficPolicyDraftMemo): void => {
    setState(next);
    setNotice("");
    setError("");
  };

  function updateMatcherError(ruleId: string, error: string): void {
    setMatcherErrors((current) => {
      if (!error) {
        if (!(ruleId in current)) return current;
        const next = { ...current };
        delete next[ruleId];
        return next;
      }
      return current[ruleId] === error
        ? current
        : { ...current, [ruleId]: error };
    });
  }

  function invalidateTransaction(
    next: TrafficPolicyDraftMemo,
  ): TrafficPolicyDraftMemo {
    return {
      ...next,
      testResult: null,
      preview: null,
      applied: null,
    };
  }

  function updateRule(ruleId: string, patch: Partial<TrafficRule>): void {
    const rules = activeState.rules.map((rule) =>
      rule.rule_id === ruleId ? { ...rule, ...patch } : rule,
    );
    updateState(
      invalidateTransaction({ ...activeState, rules: normalizeRules(rules) }),
    );
  }

  function reorderRules(sourceId: string, targetId: string): void {
    if (sourceId === targetId) return;
    const from = activeState.rules.findIndex((rule) => rule.rule_id === sourceId);
    const to = activeState.rules.findIndex((rule) => rule.rule_id === targetId);
    if (from < 0 || to < 0) return;
    const rules = [...activeState.rules];
    const [moved] = rules.splice(from, 1);
    rules.splice(to, 0, moved);
    updateState(
      invalidateTransaction({ ...activeState, rules: normalizeRules(rules) }),
    );
  }

  function moveRule(ruleId: string, direction: "up" | "down"): void {
    const index = activeState.rules.findIndex((rule) => rule.rule_id === ruleId);
    const targetIndex = direction === "up" ? index - 1 : index + 1;
    if (index < 0 || targetIndex < 0 || targetIndex >= activeState.rules.length) {
      return;
    }
    reorderRules(ruleId, activeState.rules[targetIndex].rule_id);
  }

  function addRule(): void {
    const rule: TrafficRule = {
      rule_id: generatedRuleId(),
      priority: activeState.rules.length + 1,
      enabled: true,
      description: "",
      matcher: {},
      action: { kind: "direct" },
    };
    updateState(
      invalidateTransaction({
        ...activeState,
        rules: [...activeState.rules, rule],
      }),
    );
  }

  function deleteRule(ruleId: string): void {
    updateState(
      invalidateTransaction({
        ...activeState,
        rules: activeState.rules
          .filter((rule) => rule.rule_id !== ruleId)
          .map((rule, index) => ({ ...rule, priority: index + 1 })),
      }),
    );
    setPendingDelete(null);
  }

  function changeTest(patch: Partial<TrafficRoutingTestDraft>): void {
    updateState({
      ...activeState,
      test: { ...activeState.test, ...patch },
      testResult: null,
      preview: null,
      applied: null,
    });
  }

  async function request(
    action: NodeUiAction | undefined,
    values: ActionValues,
  ): Promise<NodeCommandOutcome> {
    if (!action) throw new Error(unavailableReason);
    const outcome = await onSubmit(action, values);
    const rejected = rejectedMessage(outcome, locale);
    if (rejected) throw new Error(rejected);
    return outcome;
  }

  async function runTest(): Promise<void> {
    if (!testAction || testBlockedReason) return;
    setBusy("test");
    setError("");
    setNotice("");
    try {
      const port = Number(activeState.test.destinationPort);
      if (!Number.isInteger(port) || port < 1 || port > 65_535) {
        throw new Error(label(locale, "traffic.routing.test.port.invalid"));
      }
      const application: ApplicationIdentity =
        activeState.test.applicationKind === "unknown"
          ? { kind: "unknown" }
          : {
              kind: activeState.test.applicationKind,
              value: activeState.test.applicationValue.trim(),
            };
      if (application.kind !== "unknown" && !application.value) {
        throw new Error(label(locale, "traffic.routing.test.application.required"));
      }
      const input: TrafficPolicyTestInput = {
        network_id: activeState.test.networkId as TrafficPolicyTestInput["network_id"],
        source: { kind: "manual" },
        flow: {
          application,
          domain: activeState.test.domain.trim() || null,
          destination_ip: activeState.test.destinationIp.trim(),
          destination_port: port,
          protocol: activeState.test.protocol,
        },
      };
      if (activePage.hard_constraints?.fixed_exit) {
        input.fixed_exit = activePage.hard_constraints.fixed_exit;
      }
      if (activePage.hard_constraints?.fixed_network) {
        input.fixed_network = activePage.hard_constraints.fixed_network;
      }
      const outcome = await request(testAction, {
        current_json: JSON.stringify(currentPolicy(activePage)),
        candidate_json: JSON.stringify(
          candidatePolicy(activePage, activeState.rules),
        ),
        input_json: JSON.stringify(input),
      });
      if (outcome.status !== "traffic_policy_test") {
        throw new Error(label(locale, "common.operation_failed"));
      }
      setState((current) =>
        current ? { ...current, testResult: outcome.data, preview: null } : current,
      );
      setNotice(outcomeSummary(outcome, locale));
    } catch (failure) {
      setError(failure instanceof Error ? failure.message : String(failure));
    } finally {
      setBusy(null);
    }
  }

  async function previewPolicy(): Promise<void> {
    if (!previewAction || previewBlockedReason) return;
    setBusy("preview");
    setError("");
    setNotice("");
    try {
      const outcome = await request(previewAction, {
        expected_revision: String(activePage.authority_revision),
        draft_json: JSON.stringify(draftPolicy(activePage, activeState.rules)),
      });
      if (outcome.status !== "traffic_policy_preview") {
        throw new Error(label(locale, "common.operation_failed"));
      }
      setState((current) =>
        current
          ? { ...current, preview: outcome.data, applied: null }
          : current,
      );
      setNotice(outcomeSummary(outcome, locale));
    } catch (failure) {
      setError(failure instanceof Error ? failure.message : String(failure));
    } finally {
      setBusy(null);
    }
  }

  async function applyPolicy(): Promise<void> {
    if (!applyAction || applyBlockedReason || !activeState.preview) return;
    setConfirmApply(false);
    setBusy("apply");
    setError("");
    setNotice("");
    try {
      const outcome = await request(applyAction, {
        expected_revision: String(activePage.authority_revision),
        draft_json: JSON.stringify(draftPolicy(activePage, activeState.rules)),
        confirm_risk: true,
      });
      if (outcome.status !== "traffic_policy_applied") {
        throw new Error(label(locale, "common.operation_failed"));
      }
      setState((current) =>
        current
          ? {
              ...current,
              baseRevision: outcome.data.revision,
              rules: outcome.data.policy.rules ?? [],
              testResult: null,
              preview: null,
              applied: outcome.data,
            }
          : current,
      );
      setNotice(outcomeSummary(outcome, locale));
    } catch (failure) {
      setError(failure instanceof Error ? failure.message : String(failure));
    } finally {
      setBusy(null);
    }
  }

  async function loadHistory(): Promise<void> {
    if (!historyAction || historyBlockedReason) return;
    setBusy("history");
    setError("");
    try {
      const outcome = await request(historyAction, {});
      if (outcome.status !== "traffic_policy_history") {
        throw new Error(label(locale, "common.operation_failed"));
      }
      setState((current) =>
        current ? { ...current, history: outcome.data } : current,
      );
      setNotice(outcomeSummary(outcome, locale));
    } catch (failure) {
      setError(failure instanceof Error ? failure.message : String(failure));
    } finally {
      setBusy(null);
    }
  }

  async function previewRollback(targetRevision: number): Promise<void> {
    if (!previewRollbackAction || rollbackPreviewBlockedReason) return;
    setBusy("rollback");
    setError("");
    setNotice("");
    try {
      const outcome = await request(previewRollbackAction, {
        expected_revision: String(activePage.authority_revision),
        target_revision: String(targetRevision),
      });
      if (outcome.status !== "traffic_policy_preview") {
        throw new Error(label(locale, "common.operation_failed"));
      }
      setRollbackConfirmation({
        targetRevision,
        preview: outcome.data,
      });
      setNotice(outcomeSummary(outcome, locale));
    } catch (failure) {
      setError(failure instanceof Error ? failure.message : String(failure));
    } finally {
      setBusy(null);
    }
  }

  async function rollbackPolicy(): Promise<void> {
    if (!rollbackAction || rollbackBlockedReason || !rollbackConfirmation) return;
    const confirmation = rollbackConfirmation;
    setRollbackConfirmation(null);
    setBusy("rollback");
    setError("");
    setNotice("");
    try {
      const outcome = await request(rollbackAction, {
        expected_revision: String(activePage.authority_revision),
        target_revision: String(confirmation.targetRevision),
        confirm_risk: true,
      });
      if (outcome.status !== "traffic_policy_rolled_back") {
        throw new Error(label(locale, "common.operation_failed"));
      }
      setState((current) =>
        current
          ? {
              ...current,
              baseRevision: outcome.data.revision,
              rules: outcome.data.policy.rules ?? [],
              testResult: null,
              preview: null,
              applied: outcome.data,
            }
          : current,
      );
      setNotice(outcomeSummary(outcome, locale));
    } catch (failure) {
      setError(failure instanceof Error ? failure.message : String(failure));
    } finally {
      setBusy(null);
    }
  }

  const workflowStates: Array<{
    id: string;
    text: string;
    tone: "neutral" | "active" | "success" | "warning" | "danger";
  }> = [];
  if (dirty) workflowStates.push({ id: "draft", text: label(locale, "traffic.routing.state.draft"), tone: "warning" });
  if (staleDraft) workflowStates.push({ id: "stale", text: label(locale, "traffic.routing.state.stale"), tone: "danger" });
  if (activePage.workflow.status === "applying" || busy === "apply" || busy === "rollback") {
    workflowStates.push({ id: "applying", text: label(locale, "traffic.routing.state.applying"), tone: "active" });
  }
  if (activePage.workflow.status === "applied" && !dirty) {
    workflowStates.push({ id: "applied", text: label(locale, "traffic.routing.state.applied"), tone: "success" });
  }
  if (activePage.workflow.status === "failed") {
    workflowStates.push({ id: "failed", text: label(locale, "traffic.routing.state.failed"), tone: "danger" });
  }
  if (activePage.workflow.status === "stale") {
    workflowStates.push({ id: "authority_stale", text: label(locale, "traffic.routing.state.stale"), tone: "warning" });
  }
  if (activePage.workflow.status === "unconfigured") {
    workflowStates.push({ id: "unconfigured", text: label(locale, "traffic.routing.state.unconfigured"), tone: "neutral" });
  }

  const fixedTarget = activePage.hard_constraint_target
    ? targetForAction(
        activePage.hard_constraint_target.kind === "network"
          ? { kind: "network", value: activePage.hard_constraint_target.id as NetworkId }
          : activePage.hard_constraint_target.kind === "exit"
            ? { kind: "exit", value: activePage.hard_constraint_target.id as NodeId }
            : { kind: activePage.hard_constraint_target.kind },
        activePage.targets,
      )
    : undefined;

  return (
    <section className="traffic-routing-workspace" aria-labelledby={titleId}>
      <header className="policy-heading">
        <div className="policy-heading-mark" aria-hidden="true">
          <ShieldAlert size={22} />
        </div>
        <div className="policy-heading-copy">
          <span>{label(locale, "traffic.routing.eyebrow")}</span>
          <h2 id={titleId}>{label(locale, "traffic.routing.title")}</h2>
          <p>{label(locale, "traffic.routing.description")}</p>
        </div>
        <div className="traffic-authority-badge" data-authority-revision={activePage.authority_revision}>
          <span>{label(locale, "traffic.routing.authority_revision")}</span>
          <strong>{activePage.authority_revision}</strong>
        </div>
      </header>

      <div className="traffic-state-track" aria-live="polite">
        {workflowStates.length ? (
          workflowStates.map((workflow) => (
            <span
              className={`status-pill traffic-state traffic-state-${workflow.tone}`}
              data-traffic-state={workflow.id}
              key={workflow.id}
            >
              {workflow.tone === "active" ? (
                <LoaderCircle className="spin" size={13} />
              ) : null}
              {workflow.text}
            </span>
          ))
        ) : (
          <span className="status-pill neutral">
            {label(locale, "traffic.routing.state.clean")}
          </span>
        )}
      </div>

      <div className="traffic-authority-grid">
        <section className="policy-form-section">
          <h4>{label(locale, "traffic.routing.default_policy")}</h4>
          <dl className="traffic-facts">
            <div>
              <dt>{label(locale, "traffic.routing.action")}</dt>
              <dd>{policyActionLabel(activePage.default_policy.action, activePage)}</dd>
            </div>
            <div>
              <dt>{label(locale, "traffic.routing.confirmed_by")}</dt>
              <dd>{activePage.default_policy.confirmed_by}</dd>
            </div>
            <div>
              <dt>{label(locale, "traffic.routing.failure_policy")}</dt>
              <dd>
                {activePage.failure_policy.direct_fallback
                  ? tr(locale, "traffic.routing.direct_fallback", {
                      failures: activePage.failure_policy.direct_fallback.allowed_failures.join(", "),
                    })
                  : label(locale, "traffic.routing.fail_closed")}
              </dd>
            </div>
          </dl>
        </section>
        <section className="policy-form-section">
          <h4>{label(locale, "traffic.routing.hard_constraints")}</h4>
          {activePage.hard_constraints?.fixed_exit || activePage.hard_constraints?.fixed_network ? (
            <div className="traffic-constraint">
              <ShieldAlert size={17} />
              <div>
                <strong>
                  {fixedTarget?.label ??
                    activePage.hard_constraints.fixed_exit ??
                    activePage.hard_constraints.fixed_network}
                </strong>
                <span>{label(locale, "traffic.routing.hard_constraint_note")}</span>
                {activePage.hard_constraint_target?.resolved === false ? (
                  <small className="form-error">
                    {label(locale, "traffic.routing.target.unresolved")}
                  </small>
                ) : null}
              </div>
            </div>
          ) : (
            <p className="traffic-muted">{label(locale, "traffic.routing.no_hard_constraint")}</p>
          )}
        </section>
      </div>

      <section className="policy-form-section" aria-labelledby={`${titleId}-target-search`}>
        <div className="policy-section-subheading">
          <h4 id={`${titleId}-target-search`}>{label(locale, "traffic.routing.targets")}</h4>
          <span>{targets.length}</span>
        </div>
        <label className="traffic-search-field">
          <Search size={15} />
          <input
            type="search"
            value={targetQuery}
            aria-label={label(locale, "traffic.routing.targets.search")}
            placeholder={label(locale, "traffic.routing.targets.search_placeholder")}
            onChange={(event) => setTargetQuery(event.target.value)}
          />
        </label>
        {targetQuery.trim() ? (
          targetResults.length ? (
            <ul className="traffic-target-results">
              {targetResults.map((target) => (
                <li key={`${target.kind}:${target.id}`}>
                  <div>
                    <strong>{target.label}</strong>
                    <span>{target.kind} · <code>{target.id}</code></span>
                  </div>
                  {target.navigation ? (
                    <button
                      className="secondary-button compact"
                      type="button"
                      onClick={() =>
                        onNavigate(
                          target.navigation?.page_id ?? "",
                          target.navigation?.network_name ?? undefined,
                        )
                      }
                    >
                      <ExternalLink size={13} />
                      {label(locale, "traffic.routing.targets.jump")}
                    </button>
                  ) : (
                    <span className="traffic-muted">{label(locale, "traffic.routing.targets.no_navigation")}</span>
                  )}
                </li>
              ))}
            </ul>
          ) : (
            <p className="traffic-muted" role="status">
              {label(locale, "traffic.routing.targets.no_results")}
            </p>
          )
        ) : (
          <p className="traffic-muted">{label(locale, "traffic.routing.targets.search_hint")}</p>
        )}
      </section>

      <section className="traffic-rule-section" aria-labelledby={`${titleId}-rules`}>
        <div className="traffic-rule-heading">
          <div>
            <span>{label(locale, "traffic.routing.rules.eyebrow")}</span>
            <h3 id={`${titleId}-rules`}>{label(locale, "traffic.routing.rules.title")}</h3>
            <p>{label(locale, "traffic.routing.rules.description")}</p>
          </div>
          <button
            className="secondary-button"
            type="button"
            disabled={Boolean(authoringBlocked) || pending}
            aria-describedby={authoringBlocked ? `${titleId}-authoring-reason` : undefined}
            onClick={addRule}
          >
            <Plus size={14} />
            {label(locale, "traffic.routing.rules.add")}
          </button>
        </div>
        {authoringBlocked ? (
          <p className="control-reason" id={`${titleId}-authoring-reason`} role="status">
            {authoringBlocked}
          </p>
        ) : null}

        {activeState.rules.length ? (
          <ol className="traffic-rule-list">
            {activeState.rules.map((rule, index) => {
              const authorityRule = activePage.rules.find(
                (candidate) => candidate.rule_id === rule.rule_id,
              );
              const target = targetForAction(rule.action, activePage.targets);
              const selectOptions = [...activePage.targets];
              if (!target && rule.action.kind !== "direct" && rule.action.kind !== "reject") {
                selectOptions.push({
                  kind: rule.action.kind,
                  id: rule.action.value,
                  label: `${rule.action.kind}:${rule.action.value}`,
                  search_terms: [],
                });
              }
              return (
                <li
                  className={`traffic-rule-card ${rule.enabled ? "" : "disabled"}`}
                  data-rule-id={rule.rule_id}
                  draggable={Boolean(activePage.can_edit && !authoringBlocked && !pending)}
                  key={rule.rule_id}
                  tabIndex={0}
                  onDragStart={(event: DragEvent<HTMLLIElement>) => {
                    event.dataTransfer?.setData("text/plain", rule.rule_id);
                    event.dataTransfer?.setData("application/x-fuckipv6-rule", rule.rule_id);
                  }}
                  onDragOver={(event: DragEvent<HTMLLIElement>) => {
                    if (activePage.can_edit && !authoringBlocked && !pending) {
                      event.preventDefault();
                    }
                  }}
                  onDrop={(event: DragEvent<HTMLLIElement>) => {
                    event.preventDefault();
                    const sourceId =
                      event.dataTransfer?.getData("application/x-fuckipv6-rule") ||
                      event.dataTransfer?.getData("text/plain");
                    if (sourceId) reorderRules(sourceId, rule.rule_id);
                  }}
                  onKeyDown={(event: KeyboardEvent<HTMLLIElement>) => {
                    if (!event.altKey || authoringBlocked || pending) return;
                    if (event.key === "ArrowUp") {
                      event.preventDefault();
                      moveRule(rule.rule_id, "up");
                    } else if (event.key === "ArrowDown") {
                      event.preventDefault();
                      moveRule(rule.rule_id, "down");
                    }
                  }}
                >
                  <div className="traffic-rule-card-heading">
                    <div className="traffic-rule-order">
                      <GripVertical size={17} aria-hidden="true" />
                      <span>{index + 1}</span>
                    </div>
                    <label className="policy-check compact">
                      <input
                        type="checkbox"
                        checked={rule.enabled}
                        disabled={Boolean(authoringBlocked) || pending}
                        onChange={(event) =>
                          updateRule(rule.rule_id, { enabled: event.target.checked })
                        }
                      />
                      <span>{label(locale, "traffic.routing.rule.enabled")}</span>
                    </label>
                    <div className="traffic-rule-order-actions">
                      <button
                        className="icon-button"
                        type="button"
                        disabled={index === 0 || Boolean(authoringBlocked) || pending}
                        aria-label={`${label(locale, "traffic.routing.rule.move_up")} ${index + 1}`}
                        onClick={() => moveRule(rule.rule_id, "up")}
                      >
                        <ArrowUp size={15} />
                      </button>
                      <button
                        className="icon-button"
                        type="button"
                        disabled={index === activeState.rules.length - 1 || Boolean(authoringBlocked) || pending}
                        aria-label={`${label(locale, "traffic.routing.rule.move_down")} ${index + 1}`}
                        onClick={() => moveRule(rule.rule_id, "down")}
                      >
                        <ArrowDown size={15} />
                      </button>
                      <button
                        className="icon-button danger"
                        type="button"
                        disabled={Boolean(authoringBlocked) || pending}
                        aria-label={`${label(locale, "traffic.routing.rule.delete")} ${index + 1}`}
                        onClick={() => setPendingDelete(rule.rule_id)}
                      >
                        <Trash2 size={15} />
                      </button>
                    </div>
                  </div>

                  <div className="traffic-rule-fields">
                    <label className="policy-field traffic-rule-description">
                      <span>{label(locale, "traffic.routing.rule.description")}</span>
                      <input
                        value={rule.description}
                        maxLength={256}
                        disabled={Boolean(authoringBlocked) || pending}
                        placeholder={label(locale, "traffic.routing.rule.description_placeholder")}
                        onChange={(event) =>
                          updateRule(rule.rule_id, { description: event.target.value })
                        }
                      />
                    </label>
                    <label className="policy-field">
                      <span>{label(locale, "traffic.routing.rule.action")}</span>
                      <select
                        value={actionSelectValue(rule.action)}
                        disabled={Boolean(authoringBlocked) || pending}
                        onChange={(event) => {
                          const action = actionFromValue(event.target.value, selectOptions);
                          if (action) updateRule(rule.rule_id, { action });
                        }}
                      >
                        {selectOptions.map((option) => (
                          <option
                            key={`${option.kind}:${option.id}`}
                            value={
                              option.kind === "network" || option.kind === "exit"
                                ? `${option.kind}:${option.id}`
                                : option.kind
                            }
                          >
                            {option.label} · {option.kind}
                          </option>
                        ))}
                      </select>
                    </label>
                  </div>

                  <MatcherEditor
                    matcher={rule.matcher}
                    disabled={Boolean(authoringBlocked) || pending}
                    locale={locale}
                    onChange={(matcher) => updateRule(rule.rule_id, { matcher })}
                    onValidityChange={(error) =>
                      updateMatcherError(rule.rule_id, error)
                    }
                  />

                  {authorityRule?.risks.length ? (
                    <div className="traffic-rule-risks">
                      <AlertTriangle size={14} />
                      <span>
                        {authorityRule.risks
                          .map((risk) => risk.kind)
                          .join(", ")}
                      </span>
                    </div>
                  ) : null}

                  {(!rule.enabled || pendingDelete === rule.rule_id) ? (
                    <div className="traffic-rule-impact" role="status">
                      <ShieldAlert size={15} />
                      <div>
                        <strong>{label(locale, "traffic.routing.rule.impact_title")}</strong>
                        <span>{label(locale, "traffic.routing.rule.impact_unavailable")}</span>
                      </div>
                    </div>
                  ) : null}

                  {pendingDelete === rule.rule_id ? (
                    <div className="traffic-delete-confirm" role="alertdialog" aria-label={label(locale, "traffic.routing.rule.delete_confirm")}>
                      <p>{label(locale, "traffic.routing.rule.delete_confirm_detail")}</p>
                      <div>
                        <button
                          className="secondary-button compact"
                          type="button"
                          onClick={() => setPendingDelete(null)}
                        >
                          {label(locale, "common.cancel")}
                        </button>
                        <button
                          className="danger-button compact"
                          type="button"
                          onClick={() => deleteRule(rule.rule_id)}
                        >
                          <Trash2 size={13} />
                          {label(locale, "traffic.routing.rule.delete")}
                        </button>
                      </div>
                    </div>
                  ) : null}
                </li>
            );
          })}
          </ol>
        ) : (
          <div className="policy-result-empty">
            <ShieldAlert size={22} />
            <strong>{label(locale, "traffic.routing.rules.empty")}</strong>
            <p>{label(locale, "traffic.routing.rules.empty_detail")}</p>
          </div>
        )}
      </section>

      <section className="policy-form-section" aria-labelledby={testRegionId}>
        <div className="policy-section-subheading">
          <h4 id={testRegionId}>
            <FlaskConical size={16} />
            {label(locale, "traffic.routing.test.title")}
          </h4>
          <span className="status-pill neutral">
            {label(locale, "traffic.routing.test.comparison")}
          </span>
        </div>
        <p className="traffic-muted">{label(locale, "traffic.routing.test.description")}</p>
        <div className="traffic-test-grid">
          <label className="policy-field">
            <span>{label(locale, "traffic.routing.test.network")}</span>
            <select
              value={activeState.test.networkId}
              disabled={Boolean(pending)}
              onChange={(event) => changeTest({ networkId: event.target.value })}
            >
              <option value="">{label(locale, "traffic.routing.test.network.select")}</option>
              {(snapshot.memberships ?? []).map((membership) => (
                <option value={membership.network_id} key={membership.network_id}>
                  {membership.network_name}
                </option>
              ))}
            </select>
          </label>
          <label className="policy-field">
            <span>{label(locale, "traffic.routing.test.domain")}</span>
            <input
              value={activeState.test.domain}
              disabled={Boolean(pending)}
              placeholder="api.example.com"
              onChange={(event) => changeTest({ domain: event.target.value })}
            />
          </label>
          <label className="policy-field">
            <span>{label(locale, "traffic.routing.test.destination")}</span>
            <input
              value={activeState.test.destinationIp}
              disabled={Boolean(pending)}
              placeholder="203.0.113.10"
              onChange={(event) => changeTest({ destinationIp: event.target.value })}
            />
          </label>
          <label className="policy-field">
            <span>{label(locale, "traffic.routing.test.port")}</span>
            <input
              type="number"
              min={1}
              max={65_535}
              value={activeState.test.destinationPort}
              disabled={Boolean(pending)}
              onChange={(event) => changeTest({ destinationPort: event.target.value })}
            />
          </label>
          <label className="policy-field">
            <span>{label(locale, "traffic.routing.test.protocol")}</span>
            <select
              value={activeState.test.protocol}
              disabled={Boolean(pending)}
              onChange={(event) =>
                changeTest({ protocol: event.target.value as TrafficProtocol })
              }
            >
              {PROTOCOLS.map((protocol) => (
                <option value={protocol} key={protocol}>{protocol}</option>
              ))}
            </select>
          </label>
          <label className="policy-field">
            <span>{label(locale, "traffic.routing.test.application_kind")}</span>
            <select
              value={activeState.test.applicationKind}
              disabled={Boolean(pending)}
              onChange={(event) =>
                changeTest({
                  applicationKind: event.target.value as ApplicationIdentity["kind"],
                })
              }
            >
              <option value="unknown">unknown</option>
              <option value="package_id">package_id</option>
              <option value="executable_path">executable_path</option>
            </select>
          </label>
          <label className="policy-field">
            <span>{label(locale, "traffic.routing.test.application")}</span>
            <input
              value={activeState.test.applicationValue}
              disabled={Boolean(pending) || activeState.test.applicationKind === "unknown"}
              placeholder="org.example.browser"
              onChange={(event) => changeTest({ applicationValue: event.target.value })}
            />
          </label>
        </div>
        <div className="traffic-action-row">
          <button
            className="primary-button"
            type="button"
            disabled={Boolean(testBlockedReason)}
            aria-describedby={testBlockedReason ? `${titleId}-test-reason` : undefined}
            onClick={() => void runTest()}
          >
            {busy === "test" ? <LoaderCircle className="spin" size={15} /> : <FlaskConical size={15} />}
            {label(locale, "traffic.routing.test.run")}
          </button>
          {testBlockedReason ? (
            <small className="control-reason" id={`${titleId}-test-reason`} role="status">
              {testBlockedReason}
            </small>
          ) : null}
        </div>
        {activeState.testResult ? (
          <div className="traffic-test-result" aria-live="polite">
            <div>
              <span>{label(locale, "traffic.routing.test.before")}</span>
              <strong>{testDecisionText(activeState.testResult.before, activePage)}</strong>
            </div>
            <div>
              <span>{label(locale, "traffic.routing.test.after")}</span>
              <strong>{testDecisionText(activeState.testResult.after, activePage)}</strong>
            </div>
            <div className="traffic-test-delta">
              <span>
                {tr(locale, "traffic.routing.test.delta", {
                  route: String(activeState.testResult.delta.route_changed),
                  rule: String(activeState.testResult.delta.matched_rule_changed ?? false),
                  failure: String(activeState.testResult.delta.failure_outcomes_changed ?? false),
                })}
              </span>
            </div>
          </div>
        ) : null}
      </section>

      <section className="policy-form-section" aria-labelledby={`${titleId}-transaction`}>
        <div className="policy-section-subheading">
          <h4 id={`${titleId}-transaction`}>{label(locale, "traffic.routing.transaction.title")}</h4>
          <span className="status-pill neutral">
            revision {activePage.authority_revision} → {activePage.authority_revision + 1}
          </span>
        </div>
        <p className="traffic-muted">{label(locale, "traffic.routing.transaction.description")}</p>
        <div className="traffic-action-row">
          <button
            className="secondary-button"
            type="button"
            disabled={Boolean(previewBlockedReason)}
            aria-describedby={previewBlockedReason ? `${titleId}-preview-reason` : undefined}
            onClick={() => void previewPolicy()}
          >
            {busy === "preview" ? <LoaderCircle className="spin" size={15} /> : null}
            {label(locale, "traffic.routing.preview.run")}
          </button>
          <button
            className="primary-button"
            type="button"
            disabled={Boolean(applyBlockedReason) || !previewCurrent}
            aria-describedby={applyBlockedReason ? `${titleId}-apply-reason` : undefined}
            onClick={() => setConfirmApply(true)}
          >
            {label(locale, "traffic.routing.apply.run")}
          </button>
          {previewBlockedReason ? (
            <small className="control-reason" id={`${titleId}-preview-reason`} role="status">
              {previewBlockedReason}
            </small>
          ) : null}
          {applyBlockedReason ? (
            <small className="control-reason" id={`${titleId}-apply-reason`} role="status">
              {applyBlockedReason}
            </small>
          ) : null}
        </div>
        {activeState.preview ? (
          <div className="traffic-preview-summary" data-revision-conflict={activeState.preview.revision_conflict}>
            <div className="traffic-preview-metrics">
              <span>{tr(locale, "traffic.routing.preview.diffs", { count: activeState.preview.diffs.length })}</span>
              <span>{tr(locale, "traffic.routing.preview.objects", { count: activeState.preview.affected_objects.length })}</span>
              <span>{tr(locale, "traffic.routing.preview.risks", { count: activeState.preview.risks.length })}</span>
            </div>
            {activeState.preview.revision_conflict ? (
              <p className="form-error" role="alert">{label(locale, "traffic.routing.preview.conflict")}</p>
            ) : null}
            {activeState.preview.diffs.length ? (
              <ol className="traffic-diff-list">
                {activeState.preview.diffs.map((diff, index) => (
                  <li key={`${diff.path}:${diff.change}:${index}`}>
                    <code>{diff.path}</code>
                    <span>{diff.change}</span>
                  </li>
                ))}
              </ol>
            ) : (
              <p className="traffic-muted">{label(locale, "traffic.routing.preview.no_changes")}</p>
            )}
          </div>
        ) : null}
      </section>

      <section className="policy-form-section" aria-labelledby={`${titleId}-history`}>
        <div className="policy-section-subheading">
          <h4 id={`${titleId}-history`}>
            <History size={16} />
            {label(locale, "traffic.routing.history.title")}
          </h4>
          <button
            className="secondary-button compact"
            type="button"
            disabled={Boolean(historyBlockedReason)}
            aria-describedby={historyBlockedReason ? `${titleId}-history-reason` : undefined}
            onClick={() => void loadHistory()}
          >
            {busy === "history" ? <LoaderCircle className="spin" size={14} /> : <History size={14} />}
            {label(locale, "traffic.routing.history.load")}
          </button>
          {historyBlockedReason ? (
            <small className="control-reason" id={`${titleId}-history-reason`} role="status">
              {historyBlockedReason}
            </small>
          ) : null}
        </div>
        {activeState.history ? (
          activeState.history.entries.length ? (
            <ol className="traffic-history-list">
              {activeState.history.entries.map((entry: TrafficPolicyHistoryEntry) => (
                <li key={entry.transaction_id}>
                  <div className="traffic-history-copy">
                    <strong>{entry.action} · {entry.outcome}</strong>
                    <span>
                      revision {entry.previous_revision} → {entry.candidate_revision}
                      {entry.applied_revision ? ` · applied ${entry.applied_revision}` : ""}
                    </span>
                    <small>{entry.occurred_at} · {entry.actor} · {entry.source}</small>
                  </div>
                  <div className="traffic-history-actions">
                    <button
                      className="secondary-button compact"
                      type="button"
                      disabled={Boolean(rollbackPreviewBlockedReason) || pending}
                      onClick={() => void previewRollback(rollbackTargetRevision(entry))}
                    >
                      <RotateCcw size={13} />
                      {label(locale, "traffic.routing.rollback.preview")}
                    </button>
                  </div>
                  {entry.diffs.length ? (
                    <details>
                      <summary>{tr(locale, "traffic.routing.history.details", { count: entry.diffs.length })}</summary>
                      <ol className="traffic-diff-list">
                        {entry.diffs.map((diff, index) => (
                          <li key={`${diff.path}:${diff.change}:${index}`}>
                            <code>{diff.path}</code>
                            <span>{diff.change}</span>
                          </li>
                        ))}
                      </ol>
                    </details>
                  ) : null}
                </li>
              ))}
            </ol>
          ) : (
            <p className="traffic-muted">{label(locale, "traffic.routing.history.empty")}</p>
          )
        ) : (
          <p className="traffic-muted">{label(locale, "traffic.routing.history.not_loaded")}</p>
        )}
        {rollbackPreviewBlockedReason ? (
          <small className="control-reason" role="status">{rollbackPreviewBlockedReason}</small>
        ) : null}
        {rollbackConfirmation ? (
          <div className="traffic-rollback-confirmation" role="alertdialog" aria-label={label(locale, "traffic.routing.rollback.confirm_title")}>
            <ShieldAlert size={17} />
            <div>
              <strong>{label(locale, "traffic.routing.rollback.confirm_title")}</strong>
              <p>
                {tr(locale, "traffic.routing.rollback.confirm_detail", {
                  revision: rollbackConfirmation.targetRevision,
                  current: activePage.authority_revision,
                })}
              </p>
              {rollbackConfirmation.preview.revision_conflict ? (
                <p className="form-error">{label(locale, "traffic.routing.preview.conflict")}</p>
              ) : null}
            </div>
            <div className="traffic-delete-confirm-actions">
              <button
                className="secondary-button compact"
                type="button"
                onClick={() => setRollbackConfirmation(null)}
              >
                {label(locale, "common.cancel")}
              </button>
              <button
                className="danger-button compact"
                type="button"
                disabled={Boolean(rollbackBlockedReason)}
                aria-describedby={rollbackBlockedReason ? `${titleId}-rollback-reason` : undefined}
                onClick={() => void rollbackPolicy()}
              >
                {busy === "rollback" ? <LoaderCircle className="spin" size={13} /> : <RotateCcw size={13} />}
                {label(locale, "traffic.routing.rollback.run")}
              </button>
            </div>
            {rollbackBlockedReason ? (
              <small className="control-reason" id={`${titleId}-rollback-reason`} role="status">
                {rollbackBlockedReason}
              </small>
            ) : null}
          </div>
        ) : null}
      </section>

      {notice ? (
        <div className="inline-notice success" role="status">
          <CheckCircle2 size={16} />
          <span>{notice}</span>
        </div>
      ) : null}
      {error ? (
        <div className="inline-notice danger" role="alert">
          <AlertTriangle size={16} />
          <span>{error}</span>
        </div>
      ) : null}

      {activeState.applied ? (
        <section className="traffic-applied" aria-live="polite">
          <CheckCircle2 size={18} />
          <div>
            <strong>{label(locale, "traffic.routing.applied.title")}</strong>
            <span>
              {tr(locale, "traffic.routing.applied.revision", {
                previous: activeState.applied.previous_revision,
                revision: activeState.applied.revision,
              })}
            </span>
            <small>
              {tr(locale, "traffic.routing.applied.diffs", {
                count: activeState.applied.diffs.length,
              })}
            </small>
          </div>
        </section>
      ) : null}

      <Dialog.Root open={confirmApply} onOpenChange={setConfirmApply}>
        <Dialog.Portal>
          <Dialog.Overlay className="dialog-overlay" />
          <Dialog.Content className="dialog-content confirm-dialog">
            <div className="dialog-heading">
              <div className="dialog-icon warning">
                <ShieldAlert size={20} />
              </div>
              <div>
                <Dialog.Title>{label(locale, "confirm.apply_traffic_policy.title")}</Dialog.Title>
                <Dialog.Description>
                  {label(locale, "confirm.apply_traffic_policy.consequence")}
                </Dialog.Description>
              </div>
            </div>
            <dl className="traffic-confirm-facts">
              <div>
                <dt>{label(locale, "traffic.routing.confirm.base")}</dt>
                <dd>{activePage.authority_revision}</dd>
              </div>
              <div>
                <dt>{label(locale, "traffic.routing.confirm.candidate")}</dt>
                <dd>{activePage.authority_revision + 1}</dd>
              </div>
              <div>
                <dt>{label(locale, "traffic.routing.confirm.rules")}</dt>
                <dd>{activeState.rules.length}</dd>
              </div>
              <div>
                <dt>{label(locale, "traffic.routing.confirm.diffs")}</dt>
                <dd>{activeState.preview?.diffs.length ?? 0}</dd>
              </div>
            </dl>
            <div className="dialog-actions">
              <Dialog.Close asChild>
                <button className="secondary-button" type="button">
                  {label(locale, "common.cancel")}
                </button>
              </Dialog.Close>
              <button
                className="danger-button"
                type="button"
                disabled={Boolean(applyBlockedReason)}
                aria-describedby={applyBlockedReason ? `${titleId}-dialog-apply-reason` : undefined}
                onClick={() => void applyPolicy()}
              >
                {busy === "apply" ? <LoaderCircle className="spin" size={14} /> : null}
                {label(locale, "confirm.apply_traffic_policy.action")}
              </button>
            </div>
            {applyBlockedReason ? (
              <small className="control-reason" id={`${titleId}-dialog-apply-reason`} role="status">
                {applyBlockedReason}
              </small>
            ) : null}
          </Dialog.Content>
        </Dialog.Portal>
      </Dialog.Root>
    </section>
  );
}
