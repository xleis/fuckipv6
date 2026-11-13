import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import manifestFixture from "../../../contracts/typescript/fixtures/node-manifest.json";
import productFixture from "../../../contracts/typescript/fixtures/product-visual-snapshot.json";
import type {
  EffectiveAccess,
  NodeCommandOutcome,
  NodeSnapshot,
  NodeUiAction,
  NodeUiManifest,
  PolicyApplyResult,
  PolicyCurrent,
  PolicyHistoryEntry,
  PolicyHistorySnapshot,
  PolicyPreview,
  PolicyRiskSummary,
  PolicyRollbackResult,
  PolicySnapshot,
  PolicyTestInput,
  PolicyTestResult,
} from "../../../contracts/typescript/node-contract-v1";
import type { ActionValues } from "../contract/invariants";
import { installTestI18nRuntime } from "../test/runtime";
import {
  POLICY_ACTION_IDS,
  PolicyWorkflow,
  type PolicyDraftMemo,
} from "./policy-workflow";

const manifest = manifestFixture as NodeUiManifest;
const snapshot = productFixture.snapshot as NodeSnapshot;
const policyActions = (
  manifest.pages.find((page) => page.id === "settings")?.actions ?? []
).filter((action) => POLICY_ACTION_IDS.has(action.id));

const nodeId = `node_${"N".repeat(43)}`;
const networkId = `net_${"W".repeat(43)}`;
const groupId = `group_${"G".repeat(43)}`;
const ruleId = `srule_${"R".repeat(43)}`;
const grantId = `srule_${"Q".repeat(43)}`;
const vetoId = `srule_${"V".repeat(43)}`;
const confirmFactOrder = [
  "scope",
  "revision",
  "duration",
  "undo",
  "failure",
  "audit",
];

const snapshotWithContext: NodeSnapshot = {
  ...snapshot,
  identity: snapshot.identity
    ? { ...snapshot.identity, node_id: nodeId }
    : null,
  memberships: [
    {
      network_id: networkId,
      network_name: "home",
      node_id: nodeId,
      role: "owner",
      state: "active",
      local_policy_revision: 7,
      created_at_unix_ms: 1_700_000_000_000,
      updated_at_unix_ms: 1_700_000_100_000,
    },
  ],
};

function policyInput(): PolicyTestInput {
  return {
    network_id: networkId,
    actor: nodeId,
    role: "member",
    groups: [groupId],
    membership_active: true,
    recent_reauthentication: true,
    emergency_coordinator: false,
    request: {
      action: "read",
      resource: "network",
      resource_network_id: networkId,
      intent: "read_only",
    },
    now_unix_ms: 1_800_000_000_000,
  };
}

function accessDecision(allowed: boolean): EffectiveAccess {
  return {
    allowed,
    code: allowed ? null : "explicit_deny",
    evaluated_at_unix_ms: 1_800_000_000_000,
    expired_grant_ids: [],
    layer: allowed ? "network" : "request",
    local_policy_revision: 7,
    matched_allow_rule_ids: allowed ? [ruleId] : [],
    matched_deny_rule_ids: allowed ? [] : [ruleId],
    network_acl_revision: 3,
    reason: allowed ? "allowed" : "default_denied",
    reauthentication_required_grant_ids: [],
    reauthentication_satisfied: true,
    revoked_grant_ids: [],
    rule_id: ruleId,
  };
}

function policyResult(
  allowed: boolean,
  overrides: Partial<PolicyTestResult> = {},
): PolicyTestResult {
  return {
    decision: accessDecision(allowed),
    explanations: [],
    precedence: [
      {
        precedence: allowed ? "allow" : "default_denied",
        triggered: true,
        source: allowed ? "network_acl_rule" : "access_context",
        rule_id: allowed ? ruleId : null,
      },
    ],
    validation_issues: [],
    ...overrides,
  };
}

const emptyPolicySnapshot: PolicySnapshot = {
  local_policy: null,
  network_acls: [],
};

function policySnapshotWithRule(): PolicySnapshot {
  return {
    local_policy: {
      node_id: nodeId,
      revision: 7,
      issued_at_unix_ms: 1_700_000_000_000,
      expires_at_unix_ms: 1_900_000_000_000,
      network_scope: { kind: "only", network_ids: [networkId] },
      rules: [
        {
          rule_id: ruleId,
          subjects: [{ kind: "any_member" }],
          resources: ["network"],
          actions: ["read"],
          effect: "deny",
          conditions: [{ kind: "membership_active" }],
        },
      ],
    },
    network_acls: [],
  };
}

function riskSummary(
  overrides: Partial<PolicyRiskSummary> = {},
): PolicyRiskSummary {
  return {
    level: "none",
    privilege_expansion: false,
    reasons: [],
    requires_explicit_confirmation: false,
    requires_recent_reauthentication: false,
    ...overrides,
  };
}

function policyHistoryEntry(
  overrides: Partial<PolicyHistoryEntry> = {},
): PolicyHistoryEntry {
  return {
    action: "apply",
    actor: nodeId,
    affected_objects: [],
    diffs: [
      {
        path: "/local_policy/revision",
        before: 6,
        after: 7,
        change: "local_policy_updated",
      },
    ],
    network_acl_revisions: {},
    occurred_at: "2026-11-08T00:00:00Z",
    previous_revision: 6,
    revision: 7,
    risk: riskSummary(),
    source: "webui",
    transaction_id: "tx_apply_7",
    ...overrides,
  };
}

function currentPolicy(
  revision = 7,
  snapshotValue: PolicySnapshot = emptyPolicySnapshot,
): PolicyCurrent {
  return { revision, snapshot: snapshotValue };
}

function policyHistory(
  entries: PolicyHistoryEntry[] = [policyHistoryEntry()],
  currentRevision = 7,
): PolicyHistorySnapshot {
  return { current_revision: currentRevision, entries };
}

function policyPreview(
  overrides: Partial<PolicyPreview> = {},
): PolicyPreview {
  return {
    affected_objects: [{ kind: "local_policy", network_id: networkId }],
    can_apply: true,
    candidate: {},
    current: emptyPolicySnapshot,
    current_revision: 7,
    diffs: [
      {
        path: "/local_policy/revision",
        before: 7,
        after: 8,
        change: "local_policy_updated",
      },
    ],
    expected_revision: 7,
    has_changes: true,
    includes_local_policy: false,
    network_ids: [networkId],
    revision_conflict: false,
    risk: riskSummary(),
    rollback_target_revision: null,
    ...overrides,
  };
}

function policyApplied(
  overrides: Partial<PolicyApplyResult> = {},
): PolicyApplyResult {
  const revision = overrides.revision ?? 8;
  return {
    affected_objects: [{ kind: "local_policy", network_id: networkId }],
    audit: policyHistoryEntry({
      revision,
      previous_revision: 7,
      transaction_id: `tx_apply_${revision}`,
    }),
    diffs: [
      {
        path: "/local_policy/revision",
        before: 7,
        after: 8,
        change: "local_policy_updated",
      },
    ],
    no_op: false,
    previous_revision: 7,
    revision,
    risk: riskSummary(),
    snapshot: emptyPolicySnapshot,
    ...overrides,
  };
}

function policyRolledBack(
  overrides: Partial<PolicyRollbackResult> = {},
): PolicyRollbackResult {
  const revision = overrides.revision ?? 8;
  const targetRevision = overrides.target_revision ?? 5;
  return {
    affected_objects: [{ kind: "local_policy", network_id: networkId }],
    audit: policyHistoryEntry({
      action: "rollback",
      previous_revision: 7,
      revision,
      target_revision: targetRevision,
      transaction_id: `tx_rollback_${revision}`,
    }),
    diffs: [
      {
        path: "/local_policy/revision",
        before: 7,
        after: 5,
        change: "local_policy_updated",
      },
    ],
    previous_revision: 7,
    revision,
    risk: riskSummary(),
    snapshot: emptyPolicySnapshot,
    target_revision: targetRevision,
    ...overrides,
  };
}

type PolicyResponderOptions = {
  current?: PolicyCurrent;
  history?: PolicyHistorySnapshot;
  preview?: PolicyPreview | ((values: ActionValues) => PolicyPreview);
  previewRollback?: PolicyPreview;
  applied?: PolicyApplyResult;
  rolledBack?: PolicyRollbackResult;
  testResult?: PolicyTestResult | (() => PolicyTestResult);
};

function policyResponder(options: PolicyResponderOptions = {}) {
  return vi.fn(
    async (
      action: NodeUiAction,
      values: ActionValues,
    ): Promise<NodeCommandOutcome> => {
      switch (action.id) {
        case "get_policy":
          return {
            status: "policy",
            data: options.current ?? currentPolicy(),
          };
        case "get_policy_history":
          return {
            status: "policy_history",
            data: options.history ?? policyHistory(),
          };
        case "test_policy": {
          const result =
            typeof options.testResult === "function"
              ? options.testResult()
              : (options.testResult ?? policyResult(true));
          return { status: "policy_test", data: result };
        }
        case "preview_policy": {
          const preview =
            typeof options.preview === "function"
              ? options.preview(values)
              : (options.preview ?? policyPreview());
          return { status: "policy_preview", data: preview };
        }
        case "preview_policy_rollback":
          return {
            status: "policy_preview",
            data:
              options.previewRollback ??
              policyPreview({
                expected_revision: 7,
                current_revision: 7,
                rollback_target_revision: 5,
              }),
          };
        case "apply_policy":
          return {
            status: "policy_applied",
            data: options.applied ?? policyApplied(),
          };
        case "rollback_policy":
          return {
            status: "policy_rolled_back",
            data: options.rolledBack ?? policyRolledBack(),
          };
        default:
          throw new Error(`unexpected action ${action.id}`);
      }
    },
  );
}

function callsFor(
  onSubmit: ReturnType<typeof policyResponder>,
  actionId: string,
) {
  return onSubmit.mock.calls.filter(([action]) => action.id === actionId);
}

async function waitForInitialPolicyLoad(
  onSubmit: ReturnType<typeof policyResponder>,
): Promise<void> {
  await waitFor(() => {
    expect(callsFor(onSubmit, "get_policy")).toHaveLength(1);
    expect(callsFor(onSubmit, "get_policy_history")).toHaveLength(1);
  });
}

function renderWorkflow(
  onSubmit: (
    action: NodeUiAction,
    values: ActionValues,
  ) => Promise<NodeCommandOutcome>,
  overrides: {
    actions?: NodeUiAction[];
    snapshot?: NodeSnapshot;
    draft?: PolicyDraftMemo | null;
    onDraftChange?: (draft: PolicyDraftMemo) => void;
  } = {},
) {
  return render(
    <PolicyWorkflow
      actions={overrides.actions ?? policyActions}
      snapshot={overrides.snapshot ?? snapshot}
      selectedNetwork="home"
      locale="zh-CN"
      pendingAction={null}
      interactionBlocked={null}
      draft={overrides.draft}
      onDraftChange={overrides.onDraftChange}
      onSubmit={onSubmit}
    />,
  );
}

beforeEach(() => {
  installTestI18nRuntime();
});

afterEach(() => {
  cleanup();
});

describe("explainable policy workflow", () => {
  it("builds test_policy from structured fields without requiring JSON", async () => {
    const user = userEvent.setup();
    const onSubmit = policyResponder();
    const { container } = renderWorkflow(onSubmit, {
      snapshot: snapshotWithContext,
    });

    expect(container.querySelector("textarea")).toBeNull();
    expect(
      screen.getByRole("textbox", { name: "translated:policy.field.network_id" }),
    ).toHaveValue(networkId);
    expect(
      screen.getByRole("textbox", { name: "translated:policy.field.actor" }),
    ).toHaveValue(nodeId);
    expect(screen.getByText("translated:policy.not_applied")).toBeInTheDocument();
    expect(
      screen.getByText("translated:policy.deny_priority.no_recommendation"),
    ).toBeInTheDocument();

    await user.click(
      screen.getByRole("button", { name: "translated:policy.run" }),
    );

    await waitFor(() => expect(callsFor(onSubmit, "test_policy")).toHaveLength(1));
    const [action, values] = callsFor(onSubmit, "test_policy")[0];
    expect(action.id).toBe("test_policy");
    expect(Object.keys(values)).toEqual(["policy_json"]);
    expect(JSON.parse(String(values.policy_json))).toMatchObject({
      network_id: networkId,
      actor: nodeId,
      role: "owner",
      membership_active: true,
      recent_reauthentication: true,
      request: {
        action: "read",
        resource: "network",
        resource_network_id: networkId,
        intent: "read_only",
      },
    });
    expect(
      (await screen.findAllByText("translated:policy.result.allowed")).length,
    ).toBeGreaterThan(0);
  });

  it("imports a complete advanced JSON document and maps it back to structured fields", async () => {
    const user = userEvent.setup();
    const onSubmit = policyResponder({ testResult: policyResult(false) });
    const { container } = renderWorkflow(onSubmit, {
      snapshot: snapshotWithContext,
    });
    const document: PolicyTestInput = {
      ...policyInput(),
      local_policy: {
        node_id: nodeId,
        revision: 9,
        issued_at_unix_ms: 1_700_000_000_000,
        expires_at_unix_ms: 1_900_000_000_000,
        network_scope: { kind: "only", network_ids: [networkId] },
        rules: [
          {
            rule_id: ruleId,
            subjects: [{ kind: "any_member" }],
            resources: ["network"],
            actions: ["read"],
            effect: "deny",
            conditions: [{ kind: "membership_active" }],
          },
        ],
        contribution_vetoes: [
          {
            veto_id: vetoId,
            network_id: networkId,
            contributions: ["relay"],
          },
        ],
        relay_limits: {
          max_connections: 12,
          max_concurrent_streams: 64,
          max_bandwidth_kbps: 9_000,
          max_bytes: 1_000_000,
          max_cpu_percent: 60,
          max_memory_bytes: 200_000_000,
          max_session_duration_secs: 1_800,
          quota_reset_at_unix_ms: 1_800_000_100_000,
        },
      },
      network_acl: {
        network_id: networkId,
        revision: 3,
        issued_at_unix_ms: 1_700_000_000_000,
        expires_at_unix_ms: 1_900_000_000_000,
        rules: [
          {
            rule_id: ruleId,
            subjects: [{ kind: "any_member" }],
            resources: ["network"],
            actions: ["read"],
            effect: "allow",
          },
        ],
        grants: [
          {
            grant_id: grantId,
            subjects: [{ kind: "node", value: nodeId }],
            resources: ["network"],
            actions: ["read"],
            issued_at_unix_ms: 1_700_000_000_000,
            expires_at_unix_ms: 1_900_000_000_000,
            requires_recent_reauthentication: true,
          },
        ],
        relay_limits: {
          max_connections: 8,
          max_concurrent_streams: 32,
          max_bandwidth_kbps: 4_000,
          max_bytes: 500_000,
          max_cpu_percent: 50,
          max_memory_bytes: 100_000_000,
          max_session_duration_secs: 900,
          quota_reset_at_unix_ms: 1_800_000_200_000,
        },
      },
    };
    const raw = JSON.stringify(document, null, 2);
    const file = new File([raw], "policy-test.json", {
      type: "application/json",
    });
    Object.defineProperty(file, "text", { value: async () => raw });

    await user.upload(
      container.querySelector('input[type="file"]') as HTMLInputElement,
      file,
    );

    expect(
      (await screen.findAllByText("translated:policy.imported_from")).length,
    ).toBeGreaterThan(0);
    const editor = container.querySelector("textarea") as HTMLTextAreaElement;
    expect(editor).toHaveValue(raw);
    expect(editor).toHaveAttribute("aria-invalid", "false");

    await user.click(
      screen.getByRole("button", {
        name: "translated:policy.mode.structured",
      }),
    );
    expect(
      screen.getByRole("checkbox", { name: "translated:policy.include_local" }),
    ).toBeChecked();
    expect(
      screen.getByRole("checkbox", { name: "translated:policy.include_acl" }),
    ).toBeChecked();
    expect(
      screen.getByRole("textbox", {
        name: "translated:policy.field.local_revision",
      }),
    ).toHaveValue("9");
    expect(screen.getAllByDisplayValue(ruleId)).toHaveLength(2);
    expect(screen.getByDisplayValue(grantId)).toBeInTheDocument();
    const localConnections = container.querySelector(
      "#policy-local-relay-limit-max-connections",
    ) as HTMLInputElement;
    const aclBytes = container.querySelector(
      "#policy-acl-relay-limit-max-bytes",
    ) as HTMLInputElement;
    expect(localConnections).toHaveValue("12");
    expect(aclBytes).toHaveValue("500000");
    expect(
      container.querySelector("#policy-local-relay-limit-max-cpu-percent"),
    ).toHaveValue("60");
    expect(
      container.querySelector("#policy-acl-relay-limit-quota-reset-at-unix-ms"),
    ).toHaveValue("1800000200000");

    await user.clear(localConnections);
    await user.type(localConnections, "6");
    await user.clear(aclBytes);
    await user.type(aclBytes, "250000");
    await user.click(
      screen.getByRole("button", { name: "translated:policy.run" }),
    );
    await waitFor(() =>
      expect(callsFor(onSubmit, "test_policy")).toHaveLength(1),
    );
    const candidate = JSON.parse(
      String(callsFor(onSubmit, "test_policy")[0][1].policy_json),
    ) as PolicyTestInput;
    expect(candidate.local_policy?.relay_limits).toMatchObject({
      max_connections: 6,
      max_concurrent_streams: 64,
      max_bandwidth_kbps: 9_000,
      max_bytes: 1_000_000,
      max_cpu_percent: 60,
      max_memory_bytes: 200_000_000,
      max_session_duration_secs: 1_800,
      quota_reset_at_unix_ms: 1_800_000_100_000,
    });
    expect(candidate.network_acl?.relay_limits).toMatchObject({
      max_connections: 8,
      max_concurrent_streams: 32,
      max_bandwidth_kbps: 4_000,
      max_bytes: 250_000,
      max_cpu_percent: 50,
      max_memory_bytes: 100_000_000,
      max_session_duration_secs: 900,
      quota_reset_at_unix_ms: 1_800_000_200_000,
    });
  });

  it("rejects duplicate JSON keys and exposes the exact field path", async () => {
    const user = userEvent.setup();
    const onSubmit = policyResponder();
    const { container } = renderWorkflow(onSubmit, {
      snapshot: snapshotWithContext,
    });
    await user.click(
      screen.getByRole("button", {
        name: "translated:policy.mode.advanced",
      }),
    );
    const editor = container.querySelector("textarea") as HTMLTextAreaElement;
    const duplicate = JSON.stringify(policyInput()).replace(
      `"network_id":"${networkId}"`,
      `"network_id":"${networkId}","network_id":"${networkId}"`,
    );
    fireEvent.change(editor, { target: { value: duplicate } });

    expect(editor).toHaveAttribute("aria-invalid", "true");
    expect(screen.getByRole("alert")).toHaveTextContent(
      "translated:validation.policy_json_path",
    );
    expect(screen.getByText("/network_id")).toBeInTheDocument();

    await user.click(
      screen.getByRole("button", { name: "translated:policy.run" }),
    );
    expect(callsFor(onSubmit, "test_policy")).toHaveLength(0);
  });

  it("loads the authoritative policy and audit history before editing", async () => {
    const onSubmit = policyResponder({
      current: currentPolicy(7, policySnapshotWithRule()),
      history: policyHistory([
        policyHistoryEntry({ transaction_id: "tx_authoritative_7" }),
      ]),
    });
    renderWorkflow(onSubmit, { snapshot: snapshotWithContext });

    await waitForInitialPolicyLoad(onSubmit);

    expect(callsFor(onSubmit, "get_policy")[0][1]).toEqual({});
    expect(callsFor(onSubmit, "get_policy_history")[0][1]).toEqual({});
    expect(
      screen.getByText("translated:policy.current.rules_count"),
    ).toBeInTheDocument();
    expect(screen.getByText("tx_authoritative_7")).toBeInTheDocument();
  });

  it("previews and atomically applies the exact revisioned candidate", async () => {
    const user = userEvent.setup();
    const onSubmit = policyResponder({
      preview: policyPreview({
        includes_local_policy: true,
        risk: riskSummary({
          level: "high",
          privilege_expansion: true,
          reasons: ["allow_rule_added"],
          requires_explicit_confirmation: true,
          requires_recent_reauthentication: true,
        }),
      }),
    });
    renderWorkflow(onSubmit, { snapshot: snapshotWithContext });
    await waitForInitialPolicyLoad(onSubmit);

    await user.click(
      screen.getByRole("button", { name: "translated:policy.preview" }),
    );
    await waitFor(() =>
      expect(callsFor(onSubmit, "preview_policy")).toHaveLength(1),
    );

    const previewValues = callsFor(onSubmit, "preview_policy")[0][1];
    expect(previewValues.expected_revision).toBe("7");
    const previewCandidate = JSON.parse(
      String(previewValues.candidate_json),
    ) as PolicySnapshot;

    const applyButton = screen.getByRole("button", {
      name: "translated:policy.apply",
    });
    expect(applyButton).toBeDisabled();
    expect(
      screen.getByText("translated:policy.transaction.confirm_required"),
    ).toBeInTheDocument();

    await user.click(
      screen.getByRole("checkbox", {
        name: "translated:policy.transaction.confirm_privilege_expansion",
      }),
    );
    expect(applyButton).toBeEnabled();
    await user.click(applyButton);

    const dialog = await screen.findByRole("dialog");
    expect(
      within(dialog).getByText("translated:confirm.apply_policy.title"),
    ).toBeInTheDocument();
    expect(
      Array.from(dialog.querySelectorAll("[data-confirm-fact]")).map((fact) =>
        fact.getAttribute("data-confirm-fact"),
      ),
    ).toEqual(confirmFactOrder);
    expect(
      within(dialog).getByText("translated:confirm.policy.duration"),
    ).toBeInTheDocument();
    expect(
      within(dialog).getByText("translated:confirm.policy.undo"),
    ).toBeInTheDocument();
    await user.click(
      within(dialog).getByRole("button", {
        name: "translated:confirm.apply_policy.action",
      }),
    );

    await waitFor(() =>
      expect(callsFor(onSubmit, "apply_policy")).toHaveLength(1),
    );
    const applyValues = callsFor(onSubmit, "apply_policy")[0][1];
    expect(applyValues).toMatchObject({
      expected_revision: "7",
      network_ids: networkId,
      includes_local_policy: "true",
      confirm_privilege_expansion: "true",
    });
    expect(JSON.parse(String(applyValues.candidate_json))).toEqual(
      previewCandidate,
    );
    await waitFor(() =>
      expect(callsFor(onSubmit, "get_policy_history")).toHaveLength(2),
    );
    expect(
      screen.getByText("translated:policy.applied.title"),
    ).toBeInTheDocument();
    expect(screen.getByText("tx_apply_8")).toBeInTheDocument();
  });

  it("blocks apply when the preview reports a revision conflict", async () => {
    const user = userEvent.setup();
    const onSubmit = policyResponder({
      preview: policyPreview({
        current_revision: 9,
        revision_conflict: true,
        can_apply: false,
      }),
    });
    const { container } = renderWorkflow(onSubmit, {
      snapshot: snapshotWithContext,
    });
    await waitForInitialPolicyLoad(onSubmit);

    expect(screen.getByText("translated:policy.state.idle")).toBeInTheDocument();

    await user.click(
      screen.getByRole("button", { name: "translated:policy.preview" }),
    );
    await waitFor(() =>
      expect(callsFor(onSubmit, "preview_policy")).toHaveLength(1),
    );

    expect(
      screen.getAllByText("translated:policy.transaction.revision_conflict")
        .length,
    ).toBeGreaterThan(0);
    expect(
      screen.getByRole("button", { name: "translated:policy.apply" }),
    ).toBeDisabled();
    expect(
      container.querySelector('[data-policy-state="conflict"]'),
    ).toHaveTextContent("translated:policy.state.conflict");
  });

  it("explains read-only policy access and associates it with the apply control", async () => {
    const user = userEvent.setup();
    const onSubmit = policyResponder();
    const captured: { draft: PolicyDraftMemo | null } = { draft: null };
    renderWorkflow(onSubmit, {
      snapshot: snapshotWithContext,
      onDraftChange: (draft) => {
        captured.draft = draft;
      },
    });
    await waitForInitialPolicyLoad(onSubmit);
    await user.click(
      screen.getByRole("button", { name: "translated:policy.preview" }),
    );
    await waitFor(() =>
      expect(callsFor(onSubmit, "preview_policy")).toHaveLength(1),
    );
    await waitFor(() => expect(captured.draft?.preview).not.toBeNull());
    const draft = captured.draft;
    if (!draft) throw new Error("policy draft was not captured");
    cleanup();

    const permissions = snapshotWithContext.permissions;
    if (!permissions) throw new Error("fixture permissions missing");
    const deniedSnapshot: NodeSnapshot = {
      ...snapshotWithContext,
      permissions: {
        ...permissions,
        permissions: permissions.permissions.filter(
          (permission) => permission !== "manage_networks",
        ),
      },
    };
    renderWorkflow(onSubmit, {
      actions: policyActions.filter((action) =>
        [
          "preview_policy",
          "preview_policy_rollback",
          "apply_policy",
          "rollback_policy",
        ].includes(action.id),
      ),
      snapshot: deniedSnapshot,
      draft,
    });

    const applyButton = screen.getByRole("button", {
      name: "translated:policy.apply",
    });
    expect(applyButton).toBeDisabled();
    const describedBy = applyButton.getAttribute("aria-describedby");
    expect(describedBy).toBeTruthy();
    const reason = document.getElementById(describedBy ?? "");
    expect(reason).toHaveTextContent(
      "translated:settings.policy_access.read_only",
    );
    expect(reason).toHaveTextContent("translated:blocked.permission_denied");
    expect(reason).toHaveTextContent(
      "translated:settings.policy_access.required_permission",
    );
    expect(reason).toHaveTextContent(
      "translated:settings.policy_access.request_path",
    );
    expect(reason).toHaveTextContent("translated:settings.policy_access.command");
  });

  it("blocks apply when the current revision advances after preview", async () => {
    const user = userEvent.setup();
    const onSubmit = policyResponder({
      preview: policyPreview({
        expected_revision: 6,
        current_revision: 7,
      }),
    });
    renderWorkflow(onSubmit, { snapshot: snapshotWithContext });
    await waitForInitialPolicyLoad(onSubmit);

    await user.click(
      screen.getByRole("button", { name: "translated:policy.preview" }),
    );
    await waitFor(() =>
      expect(callsFor(onSubmit, "preview_policy")).toHaveLength(1),
    );

    expect(
      screen.getAllByText("translated:policy.transaction.stale_preview")
        .length,
    ).toBeGreaterThan(0);
    expect(
      screen.getByRole("button", { name: "translated:policy.apply" }),
    ).toBeDisabled();
  });

  it("explains a no-op preview without allowing a new revision", async () => {
    const user = userEvent.setup();
    const onSubmit = policyResponder({
      preview: policyPreview({
        can_apply: false,
        diffs: [],
        has_changes: false,
      }),
    });
    renderWorkflow(onSubmit, { snapshot: snapshotWithContext });
    await waitForInitialPolicyLoad(onSubmit);

    await user.click(
      screen.getByRole("button", { name: "translated:policy.preview" }),
    );
    await waitFor(() =>
      expect(callsFor(onSubmit, "preview_policy")).toHaveLength(1),
    );

    expect(
      screen.getAllByText("translated:policy.transaction.no_changes").length,
    ).toBeGreaterThan(0);
    expect(
      screen.getByRole("button", { name: "translated:policy.apply" }),
    ).toBeDisabled();
  });

  it("previews and rolls back through a new revision with audit evidence", async () => {
    const user = userEvent.setup();
    const onSubmit = policyResponder({
      history: policyHistory([
        policyHistoryEntry({
          revision: 7,
          previous_revision: 6,
          transaction_id: "tx_current_7",
        }),
        policyHistoryEntry({
          revision: 5,
          previous_revision: 4,
          transaction_id: "tx_target_5",
        }),
      ]),
      previewRollback: policyPreview({
        expected_revision: 7,
        current_revision: 7,
        rollback_target_revision: 5,
      }),
      rolledBack: policyRolledBack(),
    });
    renderWorkflow(onSubmit, { snapshot: snapshotWithContext });
    await waitForInitialPolicyLoad(onSubmit);

    const rollbackButtons = await screen.findAllByRole("button", {
      name: "translated:policy.rollback.action",
    });
    await user.click(rollbackButtons[1]);
    await waitFor(() =>
      expect(callsFor(onSubmit, "preview_policy_rollback")).toHaveLength(1),
    );
    expect(callsFor(onSubmit, "preview_policy_rollback")[0][1]).toEqual({
      expected_revision: "7",
      target_revision: "5",
    });

    await user.click(
      screen.getByRole("button", {
        name: "translated:policy.rollback.execute",
      }),
    );
    const dialog = await screen.findByRole("dialog");
    expect(
      within(dialog).getByText("translated:confirm.rollback_policy.title"),
    ).toBeInTheDocument();
    expect(
      Array.from(dialog.querySelectorAll("[data-confirm-fact]")).map((fact) =>
        fact.getAttribute("data-confirm-fact"),
      ),
    ).toEqual(confirmFactOrder);
    expect(
      within(dialog).getByText("translated:confirm.policy.duration"),
    ).toBeInTheDocument();
    expect(
      within(dialog).getByText("translated:confirm.policy.undo"),
    ).toBeInTheDocument();
    expect(
      within(dialog).queryByText("translated:policy.rollback.new_revision"),
    ).not.toBeInTheDocument();
    await user.click(
      within(dialog).getByRole("button", {
        name: "translated:confirm.rollback_policy.action",
      }),
    );

    await waitFor(() =>
      expect(callsFor(onSubmit, "rollback_policy")).toHaveLength(1),
    );
    expect(callsFor(onSubmit, "rollback_policy")[0][1]).toMatchObject({
      expected_revision: "7",
      target_revision: "5",
      network_ids: networkId,
      includes_local_policy: "false",
      confirm_privilege_expansion: "false",
    });
    await waitFor(() =>
      expect(callsFor(onSubmit, "get_policy_history")).toHaveLength(2),
    );
    expect(screen.getByText("tx_rollback_8")).toBeInTheDocument();
    expect(
      screen.getByText("translated:policy.history.action.rollback"),
    ).toBeInTheDocument();
  });

  it("renders allowed and denied outcomes from the authoritative result", async () => {
    const user = userEvent.setup();
    let allowed = true;
    const onSubmit = policyResponder({
      testResult: () => policyResult(allowed),
    });
    renderWorkflow(onSubmit, { snapshot: snapshotWithContext });

    await user.click(
      screen.getByRole("button", { name: "translated:policy.run" }),
    );
    expect(
      (await screen.findAllByText("translated:policy.result.allowed")).length,
    ).toBeGreaterThan(0);
    expect(
      screen.getByText("translated:policy.choice.reason.allowed"),
    ).toBeInTheDocument();

    allowed = false;
    await user.click(
      screen.getByRole("button", { name: "translated:policy.run" }),
    );
    expect(
      (await screen.findAllByText("translated:policy.result.denied")).length,
    ).toBeGreaterThan(0);
    expect(
      screen.getByText("translated:policy.choice.reason.default_denied"),
    ).toBeInTheDocument();
  });

  it("locates validation issues in advanced JSON", async () => {
    const user = userEvent.setup();
    const onSubmit = policyResponder({
      testResult: policyResult(false, {
          validation_issues: [
            {
              path: "/request/action",
              code: "invalid_policy_action",
              source: "network_acl",
              related_rule_id: ruleId,
              message: "Action is not permitted by this rule",
            },
          ],
        }),
    });
    const { container } = renderWorkflow(onSubmit, {
      snapshot: snapshotWithContext,
    });

    await user.click(
      screen.getByRole("button", { name: "translated:policy.run" }),
    );
    const path = await screen.findByText("/request/action");
    await user.click(path.closest("button") as HTMLButtonElement);

    const editor = container.querySelector("textarea") as HTMLTextAreaElement;
    await waitFor(() => expect(editor).toHaveFocus());
    expect(editor.selectionStart).toBeGreaterThan(0);
    expect(
      screen.getByRole("button", {
        name: "translated:policy.mode.advanced",
      }),
    ).toHaveAttribute("aria-pressed", "true");
  });

  it("shows precedence, rule source, affected objects, and explanations", async () => {
    const user = userEvent.setup();
    const result = policyResult(false, {
      precedence: [
        {
          precedence: "local_contribution_veto",
          triggered: true,
          source: "local_contribution_veto",
          rule_id: vetoId,
        },
        {
          precedence: "network_scope_restricted",
          triggered: false,
          source: "local_network_scope",
          rule_id: null,
        },
      ],
      explanations: [
        {
          source: "network_acl_rule",
          source_path: "/network_acl/rules/0",
          rule_id: ruleId,
          effect: "deny",
          subjects: [{ kind: "any_member" }],
          resources: ["network"],
          actions: ["read"],
          conditions: [{ kind: "membership_active" }],
          matched: true,
          applied: true,
          precedence: "explicit_deny",
        },
      ],
    });
    const onSubmit = policyResponder({ testResult: result });
    renderWorkflow(onSubmit, { snapshot: snapshotWithContext });

    await user.click(
      screen.getByRole("button", { name: "translated:policy.run" }),
    );

    expect(
      await screen.findByText("translated:policy.precedence.title"),
    ).toBeInTheDocument();
    expect(
      screen.getAllByText(
        "translated:policy.choice.precedence.local_contribution_veto",
      ).length,
    ).toBeGreaterThan(0);
    expect(
      screen.getByText(
        "translated:policy.choice.precedence.network_scope_restricted",
      ),
    ).toBeInTheDocument();
    expect(
      screen.getAllByText("translated:policy.choice.source.network_acl_rule")
        .length,
    ).toBeGreaterThan(0);
    const explanationSection = screen
      .getByText("translated:policy.explain.title")
      .closest("section") as HTMLElement;
    expect(
      within(explanationSection).getByText(
        "translated:policy.choice.resource.network",
      ),
    ).toBeInTheDocument();
    expect(
      within(explanationSection).getByText("translated:policy.explain.matched"),
    ).toBeInTheDocument();
    expect(
      within(explanationSection).getByText("translated:policy.explain.applied"),
    ).toBeInTheDocument();
    expect(
      within(explanationSection).getByText("/network_acl/rules/0"),
    ).toBeInTheDocument();
  });

  it("does not coerce the visual fixture's invalid node id into the actor", () => {
    renderWorkflow(policyResponder());

    expect(
      screen.getByRole("textbox", { name: "translated:policy.field.actor" }),
    ).toHaveValue("");
    expect(
      screen.getByRole("textbox", { name: "translated:policy.field.network_id" }),
    ).toHaveValue("");
  });
});
