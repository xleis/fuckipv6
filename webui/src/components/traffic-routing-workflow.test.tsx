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
  NodeCommandOutcome,
  NodeSnapshot,
  NodeUiAction,
  NodeUiManifest,
  TrafficPolicyApplyResult,
  TrafficPolicyHistoryEntry,
  TrafficPolicyHistorySnapshot,
  TrafficPolicyPreview,
  TrafficPolicyRollbackResult,
  TrafficPolicyTestComparison,
  TrafficRoutingPageModel,
  TrafficRoutingRuleView,
  TrafficRoutingTarget,
  TrafficRule,
} from "../../../contracts/typescript/node-contract-v1";
import type { ActionValues } from "../contract/invariants";
import { installTestI18nRuntime } from "../test/runtime";
import {
  TRAFFIC_POLICY_ACTION_IDS,
  TrafficRoutingWorkflow,
  type TrafficPolicyDraftMemo,
} from "./traffic-routing-workflow";

const manifest = manifestFixture as NodeUiManifest;
const baseSnapshot = productFixture.snapshot as NodeSnapshot;
const routingActions = (
  manifest.pages.find((page) => page.id === "routing")?.actions ?? []
).filter((action) => TRAFFIC_POLICY_ACTION_IDS.has(action.id));
const implementedActions = routingActions.map(
  (action): NodeUiAction => ({ ...action, backend_status: "implemented" }),
);

const targets: TrafficRoutingTarget[] = [
  {
    id: "direct",
    kind: "direct",
    label: "Direct",
    search_terms: ["direct"],
  },
  {
    id: "home",
    kind: "network",
    label: "Home Network",
    search_terms: ["home", "家庭"],
    navigation: {
      object_id: "home",
      page_id: "networks",
      network_name: "home",
    },
  },
  {
    id: "peer-tokyo",
    kind: "exit",
    label: "Tokyo Gateway",
    search_terms: ["tky", "japan"],
    navigation: {
      object_id: "peer-tokyo",
      page_id: "exit_node",
      network_name: "home",
    },
  },
  {
    id: "reject",
    kind: "reject",
    label: "Reject",
    search_terms: ["blocked"],
  },
];

function ruleView(
  ruleId: string,
  priority: number,
  action: TrafficRoutingRuleView["action"],
  matcher: TrafficRoutingRuleView["matcher"],
  overrides: Partial<TrafficRoutingRuleView> = {},
): TrafficRoutingRuleView {
  return {
    action,
    description: `Rule ${ruleId}`,
    enabled: true,
    matcher,
    move_down: null,
    move_up: null,
    priority,
    risks: [],
    rule_id: ruleId,
    target: {
      id:
        action.kind === "network" || action.kind === "exit"
          ? action.value
          : action.kind,
      kind: action.kind,
      resolved: true,
    },
    ...overrides,
  };
}

function routingPage(
  overrides: Partial<TrafficRoutingPageModel> = {},
): TrafficRoutingPageModel {
  return {
    authority_revision: 7,
    can_edit: true,
    default_policy: {
      action: { kind: "direct" },
      confirmed_at_unix_ms: 1_700_000_000_000,
      confirmed_by: "owner",
      target: { id: "direct", kind: "direct", resolved: true },
    },
    failure_policy: {},
    hard_constraints: {},
    node_id: "node-self",
    rules: [
      ruleView("rule-a", 1, { kind: "direct" }, {
        exact_domains: ["api.example.com"],
      }),
      ruleView("rule-b", 2, { kind: "network", value: "home" }, {
        domain_suffixes: ["example.net"],
      }),
      ruleView("rule-c", 3, { kind: "exit", value: "peer-tokyo" }, {
        destination_cidrs: ["203.0.113.0/24"],
        destination_ports: [{ start: 443, end: 443 }],
        protocols: ["tcp"],
      }),
    ],
    targets,
    workflow: { base_revision: 7, status: "draft" },
    ...overrides,
  };
}

function snapshotWithPage(
  page: TrafficRoutingPageModel | null,
): NodeSnapshot {
  return {
    ...baseSnapshot,
    memberships: [
      {
        created_at_unix_ms: 1_700_000_000_000,
        local_policy_revision: 1,
        network_id: "network-home",
        network_name: "home",
        node_id: "node-self",
        role: "owner",
        state: "active",
        updated_at_unix_ms: 1_700_000_000_000,
      },
    ],
    traffic_routing_page: page,
  };
}

function cloneRules(page: TrafficRoutingPageModel): TrafficRule[] {
  return page.rules.map((rule) => ({
    action: { ...rule.action },
    description: rule.description,
    enabled: rule.enabled,
    matcher: { ...rule.matcher },
    priority: rule.priority,
    rule_id: rule.rule_id,
  }));
}

function testDraft(): TrafficPolicyDraftMemo["test"] {
  return {
    applicationKind: "unknown",
    applicationValue: "",
    destinationIp: "",
    destinationPort: "",
    domain: "",
    networkId: "network-home",
    protocol: "tcp",
  };
}

function historyEntry(): TrafficPolicyHistoryEntry {
  return {
    action: "apply",
    actor: "owner",
    affected_objects: [{ kind: "rule", object_id: "rule-a" }],
    applied_revision: 7,
    candidate_revision: 7,
    content_hash: "hash-history",
    diffs: [{ change: "rule_updated", path: "/rules/rule-a" }],
    occurred_at: "2026-11-09T00:00:00Z",
    outcome: "applied",
    previous_revision: 6,
    source: "webui",
    transaction_id: "tx-history",
  };
}

function previewAt(revision: number): TrafficPolicyPreview {
  const page = routingPage();
  return {
    affected_objects: [{ kind: "rule", object_id: "rule-a" }],
    can_apply: true,
    candidate: {
      default_policy: {
        action: page.default_policy.action,
        confirmed_at_unix_ms: page.default_policy.confirmed_at_unix_ms,
        confirmed_by: page.default_policy.confirmed_by,
      },
      failure_policy: page.failure_policy,
      node_id: page.node_id,
      revision: revision + 1,
      rules: cloneRules(page),
    },
    candidate_revision: revision + 1,
    current: {
      default_policy: {
        action: page.default_policy.action,
        confirmed_at_unix_ms: page.default_policy.confirmed_at_unix_ms,
        confirmed_by: page.default_policy.confirmed_by,
      },
      failure_policy: page.failure_policy,
      node_id: page.node_id,
      revision,
      rules: cloneRules(page),
    },
    current_revision: revision,
    diffs: [{ change: "rules_reordered", path: "/rules" }],
    expected_revision: revision,
    has_changes: true,
    revision_conflict: false,
    risks: [],
  };
}

function applyAt(revision: number): TrafficPolicyApplyResult {
  const preview = previewAt(revision - 1);
  return {
    affected_objects: preview.affected_objects,
    content_hash: "hash-applied",
    diffs: preview.diffs,
    history: historyEntry(),
    no_op: false,
    policy: preview.candidate,
    previous_revision: revision - 1,
    revision,
  };
}

function historyAt(revision: number): TrafficPolicyHistorySnapshot {
  return {
    current_revision: revision,
    entries: [historyEntry()],
  };
}

function rollbackAt(
  targetRevision: number,
  revision: number,
): TrafficPolicyRollbackResult {
  const preview = previewAt(revision - 1);
  return {
    affected_objects: preview.affected_objects,
    content_hash: "hash-rolled-back",
    diffs: preview.diffs,
    history: {
      ...historyEntry(),
      action: "rollback",
      applied_revision: revision,
      candidate_revision: revision,
      outcome: "rolled_back",
      previous_revision: revision - 1,
      target_revision: targetRevision,
    },
    policy: { ...preview.candidate, revision },
    previous_revision: revision - 1,
    revision,
    target_revision: targetRevision,
  };
}

function renderWorkflow(
  onSubmit: (
    action: NodeUiAction,
    values: ActionValues,
  ) => Promise<NodeCommandOutcome>,
  options: {
    actions?: NodeUiAction[];
    draft?: TrafficPolicyDraftMemo | null;
    onDraftChange?: (draft: TrafficPolicyDraftMemo) => void;
    onNavigate?: (pageId: string, networkName?: string) => void;
    page?: TrafficRoutingPageModel | null;
  } = {},
) {
  const onNavigate = options.onNavigate ?? vi.fn();
  const view = render(
    <TrafficRoutingWorkflow
      actions={options.actions ?? implementedActions}
      snapshot={snapshotWithPage(
        options.page === undefined ? routingPage() : options.page,
      )}
      selectedNetwork="home"
      locale="zh-CN"
      pendingAction={null}
      interactionBlocked={null}
      draft={options.draft}
      onDraftChange={options.onDraftChange}
      onSubmit={onSubmit}
      onNavigate={onNavigate}
    />,
  );
  return { ...view, onNavigate };
}

function ruleOrder(container: HTMLElement): string[] {
  return Array.from(container.querySelectorAll<HTMLElement>("[data-rule-id]")).map(
    (rule) => rule.dataset.ruleId ?? "",
  );
}

function ruleCard(container: HTMLElement, ruleId: string): HTMLElement {
  const card = container.querySelector<HTMLElement>(
    `[data-rule-id="${ruleId}"]`,
  );
  if (!card) throw new Error(`missing rule ${ruleId}`);
  return card;
}

function noOpOutcome(): Promise<NodeCommandOutcome> {
  return Promise.resolve({ status: "completed", data: { message: "ok" } });
}

beforeEach(() => {
  installTestI18nRuntime();
});

afterEach(() => {
  cleanup();
});

describe("authoritative traffic routing workflow", () => {
  it("does not construct an editor without the authoritative routing page", () => {
    const { container } = renderWorkflow(noOpOutcome, { page: null });

    expect(
      container.querySelector('[data-routing-authority="missing"]'),
    ).toHaveTextContent("translated:traffic.routing.authority.unavailable");
    expect(screen.queryByRole("list", { name: "" })).not.toBeInTheDocument();
    expect(container.querySelectorAll("[data-rule-id]")).toHaveLength(0);
  });

  it("applies equivalent rule order through buttons, keyboard, and drag", () => {
    const expected = ["rule-b", "rule-a", "rule-c"];

    const buttonView = renderWorkflow(noOpOutcome);
    fireEvent.click(
      within(ruleCard(buttonView.container, "rule-b")).getByRole("button", {
        name: "translated:traffic.routing.rule.move_up 2",
      }),
    );
    expect(ruleOrder(buttonView.container)).toEqual(expected);
    buttonView.unmount();

    const keyboardView = renderWorkflow(noOpOutcome);
    fireEvent.keyDown(ruleCard(keyboardView.container, "rule-b"), {
      altKey: true,
      key: "ArrowUp",
    });
    expect(ruleOrder(keyboardView.container)).toEqual(expected);
    keyboardView.unmount();

    const dragView = renderWorkflow(noOpOutcome);
    const dataTransfer = {
      getData: vi.fn(() => "rule-b"),
      setData: vi.fn(),
    };
    fireEvent.dragStart(ruleCard(dragView.container, "rule-b"), {
      dataTransfer,
    });
    fireEvent.dragOver(ruleCard(dragView.container, "rule-a"), {
      dataTransfer,
    });
    fireEvent.drop(ruleCard(dragView.container, "rule-a"), { dataTransfer });
    expect(ruleOrder(dragView.container)).toEqual(expected);
    expect(dataTransfer.setData).toHaveBeenCalledWith(
      "application/x-fuckipv6-rule",
      "rule-b",
    );
  });

  it("renders draft and authoritative workflow lifecycle states", () => {
    const draftView = renderWorkflow(noOpOutcome);
    fireEvent.click(
      within(ruleCard(draftView.container, "rule-b")).getByRole("button", {
        name: "translated:traffic.routing.rule.move_up 2",
      }),
    );
    expect(
      draftView.container.querySelector('[data-traffic-state="draft"]'),
    ).toBeInTheDocument();
    draftView.unmount();

    const states: Array<[TrafficRoutingPageModel["workflow"], string]> = [
      [
        {
          base_revision: 7,
          candidate_revision: 8,
          operation_id: "operation-1",
          status: "applying",
        },
        "applying",
      ],
      [{ revision: 8, status: "applied" }, "applied"],
      [
        {
          base_revision: 7,
          candidate_revision: 8,
          reason_code: "apply_failed",
          stage: "persist",
          status: "failed",
        },
        "failed",
      ],
      [
        { reason_code: "authority_changed", revision: 8, status: "stale" },
        "authority_stale",
      ],
      [{ reason_code: "not_configured", status: "unconfigured" }, "unconfigured"],
    ];

    for (const [workflow, stateId] of states) {
      const view = renderWorkflow(noOpOutcome, {
        page: routingPage({ workflow }),
      });
      expect(
        view.container.querySelector(`[data-traffic-state="${stateId}"]`),
      ).toBeInTheDocument();
      view.unmount();
    }
  });

  it("edits structured matchers and only offers known application identities", async () => {
    const user = userEvent.setup();
    const drafts: TrafficPolicyDraftMemo[] = [];
    const page = routingPage({
      rules: [
        ruleView("rule-a", 1, { kind: "direct" }, {
          protocols: ["tcp"],
        }),
      ],
    });
    const { container } = renderWorkflow(noOpOutcome, {
      page,
      onDraftChange: (draft) => drafts.push(draft),
    });

    const matcher = () =>
      ruleCard(container, "rule-a").querySelector<HTMLElement>(
        ".traffic-matcher-editor",
      )!;

    const exactDomains = within(matcher()).getByLabelText(
      "translated:traffic.routing.match.exact_domains",
    );
    await user.clear(exactDomains);
    await user.type(exactDomains, "api.example.com, login.example.com");
    fireEvent.change(
      within(matcher()).getByLabelText(
        "translated:traffic.routing.match.domain_suffixes",
      ),
      { target: { value: ".example.net, .internal.test" } },
    );
    fireEvent.change(
      within(matcher()).getByLabelText(
        "translated:traffic.routing.match.destination_cidrs",
      ),
      { target: { value: "203.0.113.0/24, 2001:db8::/32" } },
    );
    fireEvent.change(
      within(matcher()).getByLabelText(
        "translated:traffic.routing.match.destination_ports",
      ),
      { target: { value: "80, 443, 8000-8080" } },
    );
    fireEvent.click(within(matcher()).getByRole("checkbox", { name: "ICMP" }));

    const applicationKind = within(matcher()).getByLabelText(
      "translated:traffic.routing.match.application_kind",
    ) as HTMLSelectElement;
    expect(Array.from(applicationKind.options).map((option) => option.value)).toEqual([
      "package_id",
      "executable_path",
    ]);
    await user.type(
      within(matcher()).getByLabelText(
        "translated:traffic.routing.match.application_value",
      ),
      "org.example.browser",
    );
    await user.click(
      within(matcher()).getByRole("button", {
        name: "translated:traffic.routing.match.add_application",
      }),
    );

    await waitFor(() => {
      expect(drafts.at(-1)?.rules[0].matcher).toEqual({
        applications: [{ kind: "package_id", value: "org.example.browser" }],
        destination_cidrs: ["203.0.113.0/24", "2001:db8::/32"],
        destination_ports: [
          { end: 80, start: 80 },
          { end: 443, start: 443 },
          { end: 8080, start: 8000 },
        ],
        domain_suffixes: [".example.net", ".internal.test"],
        exact_domains: ["api.example.com", "login.example.com"],
        protocols: ["tcp", "icmp"],
      });
    });
  });

  it("blocks testing and preview while a structured port matcher is invalid", async () => {
    const user = userEvent.setup();
    const page = routingPage();
    const draft: TrafficPolicyDraftMemo = {
      applied: null,
      baseRevision: page.authority_revision,
      history: null,
      preview: null,
      rules: cloneRules(page),
      test: {
        ...testDraft(),
        destinationIp: "203.0.113.10",
        destinationPort: "443",
      },
      testResult: null,
    };
    const { container } = renderWorkflow(noOpOutcome, { draft, page });
    const testButton = screen.getByRole("button", {
      name: "translated:traffic.routing.test.run",
    });
    expect(testButton).toBeEnabled();

    const ports = within(ruleCard(container, "rule-a")).getByLabelText(
      "translated:traffic.routing.match.destination_ports",
    );
    await user.clear(ports);
    await user.type(ports, "443-");

    expect(testButton).toBeDisabled();
    expect(
      within(ruleCard(container, "rule-a")).getByText(
        "translated:traffic.routing.match.invalid_port",
      ),
    ).toBeInTheDocument();
    expect(
      screen.getAllByText("translated:traffic.routing.match.fix_errors"),
    ).not.toHaveLength(0);

    await user.clear(ports);
    await user.type(ports, "443");
    await waitFor(() => expect(testButton).toBeEnabled());
  });

  it("searches only authoritative terms and navigates with authoritative identity", async () => {
    const user = userEvent.setup();
    const onNavigate = vi.fn();
    renderWorkflow(noOpOutcome, { onNavigate });

    const search = screen.getByRole("searchbox", {
      name: "translated:traffic.routing.targets.search",
    });
    await user.type(search, "Tokyo");
    expect(
      await screen.findByText("translated:traffic.routing.targets.no_results"),
    ).toBeInTheDocument();

    await user.clear(search);
    await user.type(search, "japan");
    expect(await screen.findByText("Tokyo Gateway")).toBeInTheDocument();
    await user.click(
      screen.getByRole("button", {
        name: "translated:traffic.routing.targets.jump",
      }),
    );
    expect(onNavigate).toHaveBeenCalledWith("exit_node", "home");
  });

  it("blocks unavailable history, apply, and rollback actions", () => {
    const page = routingPage();
    const draft: TrafficPolicyDraftMemo = {
      applied: null,
      baseRevision: page.authority_revision,
      history: { current_revision: 7, entries: [historyEntry()] },
      preview: null,
      rules: cloneRules(page),
      test: testDraft(),
      testResult: null,
    };
    renderWorkflow(noOpOutcome, {
      actions: routingActions,
      draft,
    });

    const historyButton = screen.getByRole("button", {
      name: "translated:traffic.routing.history.load",
    });
    const previewButton = screen.getByRole("button", {
      name: "translated:traffic.routing.preview.run",
    });
    const applyButton = screen.getByRole("button", {
      name: "translated:traffic.routing.apply.run",
    });
    const rollbackPreviewButton = screen.getByRole("button", {
      name: "translated:traffic.routing.rollback.preview",
    });

    expect(historyButton).toBeDisabled();
    expect(previewButton).toBeDisabled();
    expect(applyButton).toBeDisabled();
    expect(rollbackPreviewButton).toBeDisabled();
    expect(
      historyButton.closest("section"),
    ).toHaveTextContent("translated:blocked.backend_unavailable");
    expect(
      previewButton.closest("section"),
    ).toHaveTextContent("translated:blocked.backend_unavailable");
    expect(
      rollbackPreviewButton.closest("section"),
    ).toHaveTextContent("translated:blocked.backend_unavailable");
  });

  it("sends the complete current, candidate, and structured test payload", async () => {
    const user = userEvent.setup();
    const page = routingPage({
      hard_constraint_target: {
        id: "peer-tokyo",
        kind: "exit",
        resolved: true,
      },
      hard_constraints: {
        fixed_exit: "peer-tokyo",
        fixed_network: "network-home",
      },
    });
    const comparison: TrafficPolicyTestComparison = {
      after: {
        action: { kind: "network", value: "network-home" },
        failure_decisions: [],
        matched_rule: "rule-b",
        reason: "first_match",
        visibility: "full",
      },
      before: {
        action: { kind: "direct" },
        failure_decisions: [],
        matched_rule: null,
        reason: "default",
        visibility: "full",
      },
      delta: {
        changed: true,
        failure_outcomes_changed: false,
        matched_rule_changed: true,
        route_changed: true,
      },
      network_id: "network-home",
      source: { kind: "manual" },
    };
    const onSubmit = vi.fn(
      async (
        _action: NodeUiAction,
        _values: ActionValues,
      ): Promise<NodeCommandOutcome> => {
        void _action;
        void _values;
        return {
          data: comparison,
          status: "traffic_policy_test",
        };
      },
    );
    renderWorkflow(onSubmit, { page });

    fireEvent.change(
      screen.getByLabelText("translated:traffic.routing.test.domain"),
      { target: { value: "api.example.com" } },
    );
    fireEvent.change(
      screen.getByLabelText("translated:traffic.routing.test.destination"),
      { target: { value: "203.0.113.10" } },
    );
    fireEvent.change(
      screen.getByLabelText("translated:traffic.routing.test.port"),
      { target: { value: "443" } },
    );
    fireEvent.change(
      screen.getByLabelText("translated:traffic.routing.test.protocol"),
      { target: { value: "udp" } },
    );
    fireEvent.change(
      screen.getByLabelText("translated:traffic.routing.test.application_kind"),
      { target: { value: "package_id" } },
    );
    fireEvent.change(
      screen.getByLabelText("translated:traffic.routing.test.application"),
      { target: { value: "org.example.browser" } },
    );
    await user.click(
      screen.getByRole("button", {
        name: "translated:traffic.routing.test.run",
      }),
    );

    await waitFor(() => expect(onSubmit).toHaveBeenCalledTimes(1));
    const [action, values] = onSubmit.mock.calls[0];
    expect(action.id).toBe("test_traffic_policy");
    const current = JSON.parse(String(values.current_json));
    const candidate = JSON.parse(String(values.candidate_json));
    const input = JSON.parse(String(values.input_json));

    expect(current.revision).toBe(7);
    expect(current.rules).toHaveLength(3);
    expect(candidate.revision).toBe(8);
    expect(candidate.rules).toHaveLength(3);
    expect(input).toEqual({
      fixed_exit: "peer-tokyo",
      fixed_network: "network-home",
      flow: {
        application: { kind: "package_id", value: "org.example.browser" },
        destination_ip: "203.0.113.10",
        destination_port: 443,
        domain: "api.example.com",
        protocol: "udp",
      },
      network_id: "network-home",
      source: { kind: "manual" },
    });
    expect(await screen.findByText("translated:traffic.routing.test.before")).toBeInTheDocument();
    expect(screen.getByText("translated:traffic.routing.test.after")).toBeInTheDocument();
  });

  it("completes test, preview, apply, history, and rollback through authoritative actions", async () => {
    const user = userEvent.setup();
    const page = routingPage();
    const draft: TrafficPolicyDraftMemo = {
      applied: null,
      baseRevision: page.authority_revision,
      history: null,
      preview: null,
      rules: cloneRules(page),
      test: {
        ...testDraft(),
        destinationIp: "203.0.113.10",
        destinationPort: "443",
      },
      testResult: null,
    };
    const comparison: TrafficPolicyTestComparison = {
      after: {
        action: { kind: "direct" },
        failure_decisions: [],
        matched_rule: "rule-a",
        reason: "first_match",
        visibility: "full",
      },
      before: {
        action: { kind: "direct" },
        failure_decisions: [],
        matched_rule: null,
        reason: "default",
        visibility: "full",
      },
      delta: {
        changed: true,
        failure_outcomes_changed: false,
        matched_rule_changed: true,
        route_changed: false,
      },
      network_id: "network-home",
      source: { kind: "manual" },
    };
    const onSubmit = vi.fn(
      async (
        action: NodeUiAction,
        _values: ActionValues,
      ): Promise<NodeCommandOutcome> => {
        void _values;
        switch (action.id) {
          case "test_traffic_policy":
            return { data: comparison, status: "traffic_policy_test" };
          case "preview_traffic_policy":
            return { data: previewAt(7), status: "traffic_policy_preview" };
          case "apply_traffic_policy":
            return { data: applyAt(8), status: "traffic_policy_applied" };
          case "get_traffic_policy_history":
            return { data: historyAt(8), status: "traffic_policy_history" };
          case "preview_traffic_policy_rollback":
            return { data: previewAt(8), status: "traffic_policy_preview" };
          case "rollback_traffic_policy":
            return { data: rollbackAt(7, 9), status: "traffic_policy_rolled_back" };
          default:
            return {
              data: { message: "ok" },
              status: "completed",
            };
        }
      },
    );
    const workflow = (revision: number) => (
      <TrafficRoutingWorkflow
        actions={implementedActions}
        snapshot={snapshotWithPage(routingPage({ authority_revision: revision }))}
        selectedNetwork="home"
        locale="zh-CN"
        pendingAction={null}
        interactionBlocked={null}
        draft={draft}
        onSubmit={onSubmit}
        onNavigate={vi.fn()}
      />
    );
    const view = render(workflow(7));

    await user.click(
      screen.getByRole("button", {
        name: "translated:traffic.routing.test.run",
      }),
    );
    await screen.findByText("translated:traffic.routing.test.after");
    await user.click(
      screen.getByRole("button", {
        name: "translated:traffic.routing.preview.run",
      }),
    );
    await waitFor(() =>
      expect(
        screen.getByRole("button", {
          name: "translated:traffic.routing.apply.run",
        }),
      ).toBeEnabled(),
    );
    await user.click(
      screen.getByRole("button", {
        name: "translated:traffic.routing.apply.run",
      }),
    );
    await user.click(
      await screen.findByRole("button", {
        name: "translated:confirm.apply_traffic_policy.action",
      }),
    );
    await waitFor(() => expect(onSubmit).toHaveBeenCalledTimes(3));
    view.rerender(workflow(8));

    await user.click(
      screen.getByRole("button", {
        name: "translated:traffic.routing.history.load",
      }),
    );
    await waitFor(() => expect(onSubmit).toHaveBeenCalledTimes(4));
    await user.click(
      screen.getByRole("button", {
        name: "translated:traffic.routing.rollback.preview",
      }),
    );
    await waitFor(() => expect(onSubmit).toHaveBeenCalledTimes(5));
    await user.click(
      await screen.findByRole("button", {
        name: "translated:traffic.routing.rollback.run",
      }),
    );
    await waitFor(() => expect(onSubmit).toHaveBeenCalledTimes(6));

    expect(onSubmit.mock.calls.map(([action]) => action.id)).toEqual([
      "test_traffic_policy",
      "preview_traffic_policy",
      "apply_traffic_policy",
      "get_traffic_policy_history",
      "preview_traffic_policy_rollback",
      "rollback_traffic_policy",
    ]);
    expect(onSubmit.mock.calls[1][1]).toEqual({
      draft_json: expect.any(String),
      expected_revision: "7",
    });
    expect(onSubmit.mock.calls[2][1]).toEqual({
      confirm_risk: true,
      draft_json: expect.any(String),
      expected_revision: "7",
    });
    expect(onSubmit.mock.calls[3][1]).toEqual({});
    expect(onSubmit.mock.calls[4][1]).toEqual({
      expected_revision: "8",
      target_revision: "7",
    });
    expect(onSubmit.mock.calls[5][1]).toEqual({
      confirm_risk: true,
      expected_revision: "8",
      target_revision: "7",
    });
  });

  it("shows only the authoritative unavailable impact for disabled and deleted rules", () => {
    const page = routingPage({
      rules: [
        ruleView("rule-a", 1, { kind: "direct" }, {}, { enabled: false }),
        ruleView("rule-b", 2, { kind: "network", value: "home" }, {}),
      ],
    });
    const { container } = renderWorkflow(noOpOutcome, { page });

    expect(
      within(ruleCard(container, "rule-a")).getByText(
        "translated:traffic.routing.rule.impact_unavailable",
      ),
    ).toBeInTheDocument();
    fireEvent.click(
      within(ruleCard(container, "rule-b")).getByRole("button", {
        name: "translated:traffic.routing.rule.delete 2",
      }),
    );
    expect(
      within(ruleCard(container, "rule-b")).getByText(
        "translated:traffic.routing.rule.impact_unavailable",
      ),
    ).toBeInTheDocument();
    expect(
      container.querySelectorAll(".traffic-rule-impact"),
    ).toHaveLength(2);
    expect(container.querySelector("[data-hit-sample]")).toBeNull();
  });

  it("does not expose a raw traffic policy form through the action panel", () => {
    const exitNodeActions =
      manifest.pages.find((page) => page.id === "exit_node")?.actions ?? [];
    expect(
      exitNodeActions.some((action) => TRAFFIC_POLICY_ACTION_IDS.has(action.id)),
    ).toBe(false);
    expect(
      routingActions.every((action) => TRAFFIC_POLICY_ACTION_IDS.has(action.id)),
    ).toBe(true);
  });
});
