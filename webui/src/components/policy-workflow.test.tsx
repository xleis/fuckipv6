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
  PolicyTestInput,
  PolicyTestResult,
} from "../../../contracts/typescript/node-contract-v1";
import type { ActionValues } from "../contract/invariants";
import { installTestI18nRuntime } from "../test/runtime";
import { POLICY_ACTION_IDS, PolicyWorkflow } from "./policy-workflow";

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

function renderWorkflow(
  onSubmit: (
    action: NodeUiAction,
    values: ActionValues,
  ) => Promise<NodeCommandOutcome>,
  overrides: {
    actions?: NodeUiAction[];
    snapshot?: NodeSnapshot;
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
    const onSubmit = vi.fn(
      async (
        action: NodeUiAction,
        values: ActionValues,
      ): Promise<NodeCommandOutcome> => {
        void action;
        void values;
        return { status: "policy_test", data: policyResult(true) };
      },
    );
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

    await waitFor(() => expect(onSubmit).toHaveBeenCalledTimes(1));
    const [action, values] = onSubmit.mock.calls[0];
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
    const onSubmit = vi.fn(
      async (): Promise<NodeCommandOutcome> => ({
        status: "policy_test",
        data: policyResult(false),
      }),
    );
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
  });

  it("rejects duplicate JSON keys and exposes the exact field path", async () => {
    const user = userEvent.setup();
    const onSubmit = vi.fn(
      async (): Promise<NodeCommandOutcome> => ({
        status: "policy_test",
        data: policyResult(true),
      }),
    );
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
    expect(onSubmit).not.toHaveBeenCalled();
  });

  it("renders allowed and denied outcomes from the authoritative result", async () => {
    const user = userEvent.setup();
    let allowed = true;
    const onSubmit = vi.fn(
      async (): Promise<NodeCommandOutcome> => ({
        status: "policy_test",
        data: policyResult(allowed),
      }),
    );
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
    const onSubmit = vi.fn(
      async (): Promise<NodeCommandOutcome> => ({
        status: "policy_test",
        data: policyResult(false, {
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
      }),
    );
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
    const onSubmit = vi.fn(
      async (): Promise<NodeCommandOutcome> => ({
        status: "policy_test",
        data: result,
      }),
    );
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
    renderWorkflow(vi.fn(async (): Promise<NodeCommandOutcome> => ({
      status: "policy_test",
      data: policyResult(true),
    })));

    expect(
      screen.getByRole("textbox", { name: "translated:policy.field.actor" }),
    ).toHaveValue("");
    expect(
      screen.getByRole("textbox", { name: "translated:policy.field.network_id" }),
    ).toHaveValue("");
  });
});
