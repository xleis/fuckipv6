import {
  cleanup,
  fireEvent,
  render,
  screen,
  within,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import manifestFixture from "../../../contracts/typescript/fixtures/node-manifest.json";
import productFixture from "../../../contracts/typescript/fixtures/product-visual-snapshot.json";
import type {
  ActivityEntry,
  NodeExitSelectionSnapshot,
  NodeProblem,
  NodeSnapshot,
  RelayAuthorizationAuditSnapshot,
  RelayGovernanceAuditSnapshot,
  RelayHealthSnapshot,
  NodeUiManifest,
  NodeUiPage,
} from "../../../contracts/typescript/node-contract-v1";
import type { ActivityPresentationSignals } from "../activity";
import { PageContent } from "./page-content";
import { installTestI18nRuntime, translated } from "../test/runtime";

const manifest = manifestFixture as NodeUiManifest;
const networksPage = manifest.pages.find(
  (page) => page.id === "networks",
) as NodeUiPage;
const activityPage = manifest.pages.find(
  (page) => page.id === "activity",
) as NodeUiPage;
const settingsPage = manifest.pages.find(
  (page) => page.id === "settings",
) as NodeUiPage;
const exitNodePage = manifest.pages.find(
  (page) => page.id === "exit_node",
) as NodeUiPage;
const relayPage = manifest.pages.find(
  (page) => page.id === "relay",
) as NodeUiPage;
const baseSnapshot = productFixture.snapshot as NodeSnapshot;
const oldTermsHash = "hash_AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
const currentTermsHash = "hash_BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB";

function policySnapshot(exitRoleEnabled: boolean): NodeSnapshot {
  return {
    ...baseSnapshot,
    memberships: [
      {
        network_name: "home",
        network_id: "net_policy_home",
        node_id: "node-visual-01",
        state: "active",
        role: "member",
        local_policy_revision: 2,
        network_policy_revision: 4,
        privacy_policy_revision: 3,
        member_limit: 16,
        exit_contribution: "required",
        exit_contribution_scope: "all_members",
        terms_hash: oldTermsHash,
        current_terms: {
          network_policy_revision: 5,
          privacy_policy_revision: 3,
          exit_contribution: "required",
          exit_contribution_scope: "all_members",
          member_limit: 16,
        },
        current_terms_hash: currentTermsHash,
        policy_confirmation_required: true,
        acknowledged_at_unix_ms: null,
        created_at_unix_ms: 1_700_000_000_000,
        updated_at_unix_ms: 1_700_000_100_000,
        suspension_reason: null,
      },
    ],
    networks: (baseSnapshot.networks ?? []).map((network) =>
      network.name === "home"
        ? {
            ...network,
            roles: exitRoleEnabled
              ? (["member", "exit_node"] as typeof network.roles)
              : (["member"] as typeof network.roles),
          }
        : network,
    ),
  };
}

function renderNetworks(snapshot: NodeSnapshot, onRequestAction = vi.fn()) {
  render(
    <PageContent
      page={networksPage}
      snapshot={snapshot}
      locale="zh-CN"
      selectedNetwork="home"
      recentDiagnostics={null}
      onSelectNetwork={() => {}}
      onNavigate={() => {}}
      onRequestAction={onRequestAction}
      onRunAction={async () => {}}
    />,
  );
  return onRequestAction;
}

function renderSettings(
  snapshot: NodeSnapshot,
  actions = settingsPage.actions,
) {
  return render(
    <PageContent
      page={settingsPage}
      snapshot={snapshot}
      locale="zh-CN"
      selectedNetwork="home"
      recentDiagnostics={null}
      onSelectNetwork={() => {}}
      onNavigate={() => {}}
      onRequestAction={() => {}}
      onRunAction={async () => {}}
      manifestActions={actions}
    />,
  );
}

function activity(overrides: Partial<ActivityEntry> = {}): ActivityEntry {
  return {
    audit_id: "audit-default",
    actor: "node-visual-01",
    action: "network.create",
    resource: "node_command:create_network",
    outcome: "succeeded",
    occurred_at_unix_ms: Date.UTC(2026, 0, 1, 12, 0, 0),
    summary: "authoritative activity",
    category: "user_activity",
    severity: "info",
    details: {},
    object_kind: "network",
    ...overrides,
  };
}

function activityElement(
  snapshot: NodeSnapshot,
  activitySignals?: ActivityPresentationSignals,
) {
  return (
    <PageContent
      page={activityPage}
      snapshot={snapshot}
      locale="zh-CN"
      selectedNetwork="home"
      recentDiagnostics={null}
      onSelectNetwork={() => {}}
      onNavigate={() => {}}
      onRequestAction={() => {}}
      onRunAction={async () => {}}
      activitySignals={activitySignals}
    />
  );
}

function exitElement(snapshot: NodeSnapshot) {
  return (
    <PageContent
      page={exitNodePage}
      snapshot={snapshot}
      locale="zh-CN"
      selectedNetwork="home"
      recentDiagnostics={null}
      onSelectNetwork={() => {}}
      onNavigate={() => {}}
      onRequestAction={() => {}}
      onRunAction={async () => {}}
    />
  );
}

function relayHealth(
  overrides: Partial<RelayHealthSnapshot> = {},
): RelayHealthSnapshot {
  return {
    status: "healthy",
    lifecycle: "running",
    listen_addr: "127.0.0.1:6667",
    listener_bound: true,
    network_reachability: "reachable",
    forwarding_observed: true,
    authorization_valid: true,
    quota_limited: false,
    limits: {
      max_connections: 16,
      max_concurrent_streams: 32,
      max_bandwidth_kbps: 1000,
      max_bytes: 1_000_000,
      max_cpu_percent: 80,
      max_memory_bytes: 512_000_000,
      max_session_duration_secs: 3600,
      quota_reset_at_unix_ms: Date.UTC(2026, 0, 2, 0, 0, 0),
    },
    limit_sources: ["network_acl", "node_local", "provider_default"],
    usage: {
      active_connections: 2,
      active_streams: 3,
      active_sessions: 1,
      forwarded_packets: 100,
      forwarded_bytes: 4096,
      rejected_packets: 1,
      rejected_connections: 1,
      oldest_session_age_secs: 20,
      observed_cpu_percent: 12,
      observed_memory_bytes: 1024,
      cpu_measurement: "measured",
      memory_measurement: "measured",
      last_forwarded_at_unix_ms: Date.UTC(2026, 0, 1, 23, 59, 59),
      last_forwarding_failure_at_unix_ms: null,
      quota_started_at_unix_ms: Date.UTC(2026, 0, 1, 0, 0, 0),
      quota_reset_at_unix_ms: Date.UTC(2026, 0, 2, 0, 0, 0),
    },
    issues: [],
    observed_at_unix_ms: Date.UTC(2026, 0, 2, 0, 0, 0),
    reason_code: null,
    ...overrides,
  };
}

function relayElement(
  snapshot: NodeSnapshot,
  onExportRelayAudit?: (
    request: {
      network_id: string;
      from_unix_ms?: number;
      to_unix_ms?: number;
    },
  ) => Promise<{ filename: string; record_count: number }>,
) {
  return (
    <PageContent
      page={relayPage}
      snapshot={snapshot}
      locale="zh-CN"
      selectedNetwork="home"
      recentDiagnostics={null}
      onSelectNetwork={() => {}}
      onNavigate={() => {}}
      onRequestAction={() => {}}
      onRunAction={async () => {}}
      onExportRelayAudit={onExportRelayAudit}
    />
  );
}

function relayAuditSnapshot(): NodeSnapshot {
  return {
    ...baseSnapshot,
    memberships: [
      {
        network_name: "owner-relay",
        network_id: "net_owner_relay",
        node_id: "node-visual-01",
        state: "active",
        role: "member",
        local_policy_revision: 3,
        network_policy_revision: 3,
        privacy_policy_revision: 1,
        member_limit: 16,
        exit_contribution: "not_required",
        exit_contribution_scope: "all_members",
        terms_hash: null,
        current_terms: null,
        current_terms_hash: null,
        policy_confirmation_required: false,
        acknowledged_at_unix_ms: null,
        created_at_unix_ms: 1_700_000_000_000,
        updated_at_unix_ms: 1_700_000_100_000,
        suspension_reason: null,
      },
    ],
    embedded_services: [
      {
        network_name: "owner-relay",
        status: "running",
        relay_status: "running",
        relay_addr: "127.0.0.1:6667",
        relay_health: relayHealth(),
        relay_governance: {
          events: [],
          dropped_events: 0,
          payload: "AUDIT_PANEL_PAYLOAD_SENTINEL",
          destination: "2001:db8::audit-panel-secret",
        } as RelayGovernanceAuditSnapshot,
      },
    ],
  };
}

function relayAuditCheckbox(): HTMLInputElement {
  return screen.getByRole("checkbox", {
    name: translated("relay.durable_audit.confirm"),
  }) as HTMLInputElement;
}

function relayAuditExportButton(): HTMLButtonElement {
  return screen.getByTestId("relay-durable-audit-export") as HTMLButtonElement;
}

function datetimeLocal(unixMs: number): string {
  const date = new Date(unixMs);
  const pad = (value: number) => String(value).padStart(2, "0");
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}T${pad(date.getHours())}:${pad(date.getMinutes())}:${pad(date.getSeconds())}`;
}

beforeEach(() => {
  installTestI18nRuntime();
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

describe("administrator Exit governance", () => {
  it("renders the authoritative effective set, candidate reasons, and evidence source", () => {
    const selection: NodeExitSelectionSnapshot = {
      network_id: "net_governance",
      policy_revision: 12,
      policy: null,
      resolution: {
        policy_revision: 12,
        subject: "node-visual-01",
        mode: { mode: "fixed", exit: "exit-a" },
        default_exit: "exit-a",
        fixed_exit: "exit-a",
        candidates: [
          {
            exit: "exit-a",
            selectable: true,
            reason_code: "exit.selection.available",
            source: { scope: "member" },
            availability: {
              state: "available",
              source: "coordinator_health",
              observed_at_unix_ms: Date.UTC(2026, 0, 2, 3, 4, 5),
              declared_available: true,
              reason_code: null,
            },
            is_default: true,
          },
          {
            exit: "exit-b",
            selectable: false,
            reason_code: "exit.selection.fixed_exit_required",
            source: { scope: "selection_mode" },
            availability: {
              state: "available",
              source: "coordinator_health",
              observed_at_unix_ms: Date.UTC(2026, 0, 2, 3, 4, 5),
              declared_available: true,
              reason_code: null,
            },
            is_default: false,
          },
        ],
        selectable_exits: ["exit-a"],
        selected_default: "exit-a",
      },
      directory_generated_at_unix_ms: Date.UTC(2026, 0, 2, 3, 4, 0),
      directory_received_at_unix_ms: Date.UTC(2026, 0, 2, 3, 4, 1),
      evaluated_at_unix_ms: Date.UTC(2026, 0, 2, 3, 4, 2),
      reason_code: null,
    };
    const { rerender } = render(
      exitElement({ ...baseSnapshot, exit_selections: [selection] }),
    );

    const governance = screen.getByTestId("exit-governance");
    expect(governance).toHaveTextContent(translated("exit_governance.title"));
    expect(governance).toHaveTextContent("net_governance");
    expect(governance).toHaveTextContent(
      translated("exit_governance.mode.fixed"),
    );
    const selected = within(governance).getByTestId(
      "exit-candidate-net_governance-exit-a",
    );
    expect(selected).toHaveTextContent(
      translated("exit_governance.selectable"),
    );
    expect(selected).toHaveTextContent(
      translated("exit_governance.reason.available"),
    );
    expect(selected).toHaveTextContent(
      translated("exit_governance.source.member"),
    );
    const blocked = within(governance).getByTestId(
      "exit-candidate-net_governance-exit-b",
    );
    expect(blocked).toHaveTextContent(translated("exit_governance.blocked"));
    expect(blocked).toHaveTextContent(
      translated("exit_governance.reason.fixed_exit_required"),
    );
    expect(blocked).toHaveTextContent(
      translated("exit_governance.source.selection_mode"),
    );
    expect(blocked).toHaveTextContent("exit.selection.fixed_exit_required");
    expect(governance).toHaveTextContent(
      translated("exit_governance.policy_hidden"),
    );

    rerender(
      exitElement({
        ...baseSnapshot,
        exit_selections: [
          {
            ...selection,
            resolution: null,
            reason_code: "exit.selection.stale_directory",
          },
        ],
      }),
    );
    const unavailable = screen.getByTestId("exit-governance");
    expect(unavailable).toHaveTextContent(
      translated("exit_governance.unavailable"),
    );
    expect(unavailable).toHaveTextContent(
      translated("exit_governance.reason.stale_directory"),
    );
    expect(
      within(unavailable).getByTestId("exit-governance-reason"),
    ).toHaveTextContent("exit.selection.stale_directory");
  });
});

describe("local Exit lifecycle", () => {
  it("projects the authoritative stage, ownership, authorization, and structured problem", () => {
    const problem: NodeProblem = {
      category: "capability",
      code: "unavailable",
      message: "local Exit listener is unavailable",
      message_key: "problem.local_exit.listener_unavailable",
      retryable: true,
    };
    const snapshot: NodeSnapshot = {
      ...baseSnapshot,
      local_exit: {
        owner_exit_enabled: true,
        stage: "listening",
        network_allowed: false,
        healthy: false,
        active_clients: 2,
        total_clients: 7,
        last_transition_at_unix_ms: Date.UTC(2026, 0, 2, 3, 4, 5),
        problem,
      },
    };
    const { rerender } = render(exitElement(snapshot));
    const status = screen.getByTestId("local-exit-status");
    const value = (key: string) =>
      within(status).getByText(translated(key)).nextElementSibling;

    expect(status).toHaveTextContent(
      translated("gpui.section.local_exit_status"),
    );
    expect(value("gpui.local_exit.owner_intent")).toHaveTextContent(
      translated("common.enabled"),
    );
    expect(value("gpui.local_exit.stage")).toHaveTextContent(
      translated("gpui.local_exit.stage.listening"),
    );
    expect(value("gpui.local_exit.network_authorization")).toHaveTextContent(
      translated("common.disabled"),
    );
    expect(value("gpui.local_exit.health")).toHaveTextContent(
      translated("gpui.local_exit.unhealthy"),
    );
    expect(value("gpui.local_exit.clients")).toHaveAttribute(
      "data-active-clients",
      "2",
    );
    expect(value("gpui.local_exit.clients")).toHaveAttribute(
      "data-total-clients",
      "7",
    );
    expect(value("gpui.local_exit.last_transition")).toHaveTextContent(
      "2026-01-02T03:04:05.000Z",
    );

    const problemNotice = within(status).getByTestId("local-exit-problem");
    expect(problemNotice).toHaveTextContent(problem.message);
    expect(within(problemNotice).getByText(problem.code)).toBeInTheDocument();
    expect(
      within(problemNotice).getByText(problem.message_key),
    ).toBeInTheDocument();
    expect(problemNotice).toHaveTextContent(translated("common.enabled"));

    rerender(
      exitElement({
        ...snapshot,
        local_exit: { ...snapshot.local_exit, stage: "health_checking" },
      }),
    );
    expect(value("gpui.local_exit.stage")).toHaveTextContent(
      translated("gpui.local_exit.stage.health_checking"),
    );

    rerender(
      exitElement({
        ...snapshot,
        local_exit: { ...snapshot.local_exit, stage: "cleaning" },
      }),
    );
    expect(value("gpui.local_exit.stage")).toHaveTextContent(
      translated("gpui.local_exit.stage.cleaning"),
    );
  });
});

describe("security and privacy settings", () => {
  it("renders factual security state in the shared order without promotional copy", () => {
    const { container } = renderSettings(baseSnapshot);
    const facts = Array.from(
      container.querySelectorAll("[data-security-fact]"),
    ).map((fact) => fact.getAttribute("data-security-fact"));

    expect(facts).toEqual([
      "owner_identity",
      "node_id",
      "public_key_fingerprint",
      "access_model",
      "granted_permissions",
      "local_policy_revision",
      "local_policy_validity",
      "local_policy_rules",
      "local_policy_vetoes",
      "network_acls",
      "field_privacy_policies",
      "membership_policy_revisions",
      "policy_write_access",
    ]);

    const ownerSection = container.querySelector(
      '[data-settings-group="owner-security"]',
    ) as HTMLElement | null;
    expect(ownerSection).not.toBeNull();
    expect(ownerSection).toHaveTextContent(
      translated("settings.section.owner_security"),
    );
    expect(
      within(ownerSection as HTMLElement).queryByText(
        translated("identity.security.title"),
      ),
    ).not.toBeInTheDocument();
    expect(
      within(ownerSection as HTMLElement).queryByText(
        translated("settings.advanced.webui_session.detail"),
      ),
    ).not.toBeInTheDocument();
  });

  it("shows the read-only policy reason, permission, request path, and command", () => {
    const permissions = baseSnapshot.permissions;
    if (!permissions) throw new Error("fixture permissions missing");
    const snapshot: NodeSnapshot = {
      ...baseSnapshot,
      permissions: {
        ...permissions,
        permissions: permissions.permissions.filter(
          (permission) => permission !== "manage_networks",
        ),
      },
    };
    const actions = settingsPage.actions.filter(
      (action) => action.id === "apply_policy",
    );
    const { container } = renderSettings(snapshot, actions);
    const access = container.querySelector(
      '[data-security-fact="policy_write_access"]',
    ) as HTMLElement | null;

    expect(access).not.toBeNull();
    expect(access).toHaveAttribute("data-tone", "warn");
    expect(access).toHaveTextContent(
      translated("settings.policy_access.read_only"),
    );
    expect(access).toHaveTextContent("translated:blocked.permission_denied");
    expect(access).toHaveTextContent(
      translated("settings.policy_access.required_permission"),
    );
    expect(access).toHaveTextContent(
      translated("settings.policy_access.request_path"),
    );
    expect(access).toHaveTextContent(
      translated("settings.policy_access.command"),
    );
  });
});

describe("membership policy reconfirmation", () => {
  it("requires separate Exit consent and reports capability separately from enabled state", async () => {
    const user = userEvent.setup();
    const now = vi.spyOn(Date, "now").mockReturnValue(1_700_000_200_123);
    const onRequestAction = renderNetworks(policySnapshot(false));
    const card = screen.getByTestId("membership-policy-net_policy_home");

    expect(card).toHaveTextContent(
      "translated:membership.policy.confirmation_required",
    );
    expect(card).toHaveTextContent(
      "translated:join.exit.disclosure.purpose translated:join.exit.disclosure.bandwidth translated:join.exit.disclosure.traffic translated:join.exit.disclosure.logging translated:join.exit.disclosure.operational_impact translated:join.exit.disclosure.disable",
    );
    expect(card).toHaveTextContent("translated:join.exit.local_supported");
    expect(card).toHaveTextContent("translated:join.exit.local_not_enabled");
    expect(
      within(card).getByText("translated:join.network_policy_revision")
        .nextElementSibling,
    ).toHaveTextContent("5");
    expect(
      within(card).getByText("translated:join.privacy_policy_revision")
        .nextElementSibling,
    ).toHaveTextContent("3");
    expect(card).toHaveTextContent(currentTermsHash);

    const confirmation = within(card).getByRole("button", {
      name: "translated:membership.policy.confirm",
    });
    expect(confirmation).toBeDisabled();

    await user.click(
      within(card).getByRole("checkbox", {
        name: "translated:membership.policy.confirm_exit",
      }),
    );
    expect(confirmation).toBeEnabled();
    await user.click(confirmation);

    expect(onRequestAction).toHaveBeenCalledTimes(1);
    expect(onRequestAction).toHaveBeenCalledWith(
      "acknowledge_membership_policy",
      {
        network_id: "net_policy_home",
        expected_terms_hash: currentTermsHash,
        acknowledged_at_unix_ms: "1700000200123",
        exit_contribution_acknowledged: "true",
      },
    );
    expect(now).toHaveBeenCalled();
  });

  it("shows the actual enabled role only when the local Exit role is active", () => {
    renderNetworks(policySnapshot(true));
    const card = screen.getByTestId("membership-policy-net_policy_home");
    expect(card).toHaveTextContent("translated:join.exit.local_supported");
    expect(card).toHaveTextContent("translated:join.exit.local_enabled");
  });

  it("opens the member directory with the membership network ID", async () => {
    const user = userEvent.setup();
    const onRequestAction = renderNetworks(policySnapshot(false));

    await user.click(
      screen.getByRole("button", {
        name: "translated:common.actions · home",
      }),
    );
    await user.click(
      screen.getByRole("menuitem", {
        name: "translated:action.list_network_members",
      }),
    );

    expect(onRequestAction).toHaveBeenCalledWith("list_network_members", {
      network_id: "net_policy_home",
    });
  });

  it("does not expose member navigation for a network without a membership ID", async () => {
    const user = userEvent.setup();
    const snapshot = policySnapshot(false);
    snapshot.memberships = [];
    const onRequestAction = renderNetworks(snapshot);

    await user.click(
      screen.getByRole("button", {
        name: "translated:common.actions · home",
      }),
    );

    expect(
      screen.queryByRole("menuitem", {
        name: "translated:action.list_network_members",
      }),
    ).not.toBeInTheDocument();
    expect(onRequestAction).not.toHaveBeenCalled();
  });
});

describe("authoritative activity timeline", () => {
  it("renders one latest row per audit ID and applies field and inclusive time filters", async () => {
    const user = userEvent.setup();
    const start = Date.UTC(2026, 0, 1, 12, 0, 0);
    const end = start + 60 * 60 * 1_000;
    const outside = end + 60 * 60 * 1_000;
    const snapshot: NodeSnapshot = {
      ...baseSnapshot,
      recent_activity: [
        activity({
          audit_id: "audit-security",
          occurred_at_unix_ms: start,
          summary: "stale duplicate",
          network_id: "net_home",
          object_kind: "member",
          object_id: "node-peer",
          actor_role: "owner",
          category: "security_event",
          severity: "warning",
          outcome: "denied",
        }),
        activity({
          audit_id: "audit-failure",
          occurred_at_unix_ms: end,
          summary: "operation failed",
          category: "operation_failure",
          severity: "critical",
          outcome: "failed",
        }),
        activity({
          audit_id: "audit-outside",
          occurred_at_unix_ms: outside,
          summary: "outside range",
        }),
        activity({
          audit_id: "audit-security",
          occurred_at_unix_ms: start,
          summary: "latest duplicate",
          network_id: "net_home",
          object_kind: "member",
          object_id: "node-peer",
          actor_role: "owner",
          category: "security_event",
          severity: "warning",
          outcome: "denied",
        }),
      ],
    };

    render(activityElement(snapshot));

    expect(
      screen.getByTestId("activity-entry-audit-security"),
    ).toHaveTextContent("latest duplicate");
    expect(screen.queryByText("stale duplicate")).not.toBeInTheDocument();
    expect(screen.getAllByTestId(/^activity-entry-/)).toHaveLength(3);

    fireEvent.change(screen.getByLabelText("translated:activity.filter.from"), {
      target: { value: datetimeLocal(start) },
    });
    fireEvent.change(screen.getByLabelText("translated:activity.filter.to"), {
      target: { value: datetimeLocal(end) },
    });

    expect(
      screen.getByTestId("activity-entry-audit-security"),
    ).toBeInTheDocument();
    expect(
      screen.getByTestId("activity-entry-audit-failure"),
    ).toBeInTheDocument();
    expect(
      screen.queryByTestId("activity-entry-audit-outside"),
    ).not.toBeInTheDocument();

    await user.selectOptions(
      screen.getByLabelText("translated:activity.filter.category"),
      "operation_failure",
    );
    expect(
      screen.queryByTestId("activity-entry-audit-security"),
    ).not.toBeInTheDocument();
    expect(
      screen.getByTestId("activity-entry-audit-failure"),
    ).toBeInTheDocument();
  });

  it("preserves the reader scroll position when a live snapshot revision arrives", () => {
    const snapshot: NodeSnapshot = {
      ...baseSnapshot,
      revision: 20,
      recent_activity: [
        activity({
          audit_id: "audit-a",
          occurred_at_unix_ms: Date.UTC(2026, 0, 1, 12, 0, 1),
        }),
        activity({
          audit_id: "audit-b",
          occurred_at_unix_ms: Date.UTC(2026, 0, 1, 12, 0, 0),
        }),
      ],
    };
    const { rerender } = render(activityElement(snapshot));
    const timeline = screen.getByTestId("activity-timeline");
    timeline.scrollTop = 180;
    fireEvent.scroll(timeline);

    rerender(
      activityElement({
        ...snapshot,
        revision: 21,
        recent_activity: [
          activity({
            audit_id: "audit-live",
            occurred_at_unix_ms: Date.UTC(2026, 0, 1, 12, 0, 2),
          }),
          ...snapshot.recent_activity!,
        ],
      }),
    );

    expect(screen.getByTestId("activity-entry-audit-live")).toBeInTheDocument();
    expect(screen.getByTestId("activity-timeline").scrollTop).toBe(180);
  });

  it("marks stale and reconnecting snapshots without replacing the current rows", () => {
    const snapshot: NodeSnapshot = {
      ...baseSnapshot,
      recent_activity: [activity({ audit_id: "audit-preserved" })],
    };
    const { rerender } = render(
      activityElement(snapshot, { stale: true, refreshing: false }),
    );

    expect(screen.getByTestId("activity-sync-stale")).toHaveTextContent(
      "translated:activity.sync.stale.title",
    );
    expect(
      screen.getByTestId("activity-entry-audit-preserved"),
    ).toBeInTheDocument();

    rerender(activityElement(snapshot, { stale: false, refreshing: true }));
    expect(screen.queryByTestId("activity-sync-stale")).not.toBeInTheDocument();
    expect(screen.getByTestId("activity-sync-refreshing")).toHaveTextContent(
      "translated:activity.sync.refreshing.title",
    );
    expect(
      screen.getByTestId("activity-entry-audit-preserved"),
    ).toBeInTheDocument();
  });

  it("removes object links when current permission or the object disappears", () => {
    const nodeId = baseSnapshot.identity?.node_id;
    if (!nodeId) throw new Error("fixture identity missing");
    const snapshot: NodeSnapshot = {
      ...baseSnapshot,
      recent_activity: [
        activity({
          audit_id: "audit-node-link",
          object_kind: "node",
          object_id: nodeId,
        }),
      ],
    };
    const { rerender } = render(activityElement(snapshot));
    expect(
      screen.getByTestId("activity-object-link-audit-node-link"),
    ).toBeInTheDocument();

    rerender(
      activityElement({
        ...snapshot,
        permissions: { ...snapshot.permissions!, permissions: [] },
      }),
    );
    expect(
      screen.queryByTestId("activity-object-link-audit-node-link"),
    ).not.toBeInTheDocument();

    rerender(activityElement({ ...snapshot, identity: null }));
    expect(
      screen.queryByTestId("activity-object-link-audit-node-link"),
    ).not.toBeInTheDocument();
  });
});

describe("Relay capability status", () => {
  it("renders authoritative health, issues, limits, usage, and measurement states", () => {
    const listenerOnly = relayHealth({
      status: "unknown",
      forwarding_observed: false,
      issues: ["forwarding_not_observed"],
      usage: { ...relayHealth().usage, last_forwarded_at_unix_ms: null },
    });
    const stalled = relayHealth({
      status: "degraded",
      issues: ["forwarding_stalled"],
      usage: {
        ...relayHealth().usage,
        last_forwarding_failure_at_unix_ms: Date.UTC(2026, 0, 2, 0, 0, 1),
      },
    });
    const quota = relayHealth({
      status: "quota_exhausted",
      quota_limited: true,
      issues: ["quota_exhausted"],
      usage: { ...relayHealth().usage, active_connections: 16 },
    });
    const unsupported = relayHealth({
      status: "degraded",
      issues: ["cpu_measurement_unsupported", "memory_measurement_unsupported"],
      usage: {
        ...relayHealth().usage,
        observed_cpu_percent: null,
        observed_memory_bytes: null,
        cpu_measurement: "unsupported",
        memory_measurement: "unsupported",
      },
    });
    const snapshot: NodeSnapshot = {
      ...baseSnapshot,
      embedded_services: [
        ["listener-only", listenerOnly],
        ["healthy", relayHealth()],
        ["stalled", stalled],
        ["quota", quota],
        ["unsupported", unsupported],
      ].map(([network_name, health]) => ({
        network_name: network_name as string,
        status: "running",
        relay_status: "running",
        relay_addr: "127.0.0.1:6667",
        relay_health: health as RelayHealthSnapshot,
      })),
    };

    render(relayElement(snapshot));

    const listenerCard = screen
      .getAllByTestId("relay-health")
      .find((element) => element.dataset.network === "listener-only");
    expect(listenerCard).toHaveAttribute("data-relay-health", "unknown");
    expect(listenerCard).toHaveTextContent(translated("relay.health.unknown"));
    expect(listenerCard).not.toHaveTextContent(
      translated("relay.health.healthy"),
    );
    expect(listenerCard).toHaveTextContent(
      translated("relay.forwarding.not_observed"),
    );

    const healthyCard = screen
      .getAllByTestId("relay-health")
      .find((element) => element.dataset.network === "healthy");
    expect(healthyCard).toHaveAttribute("data-relay-health", "healthy");
    expect(healthyCard).toHaveTextContent(translated("relay.health.healthy"));
    expect(healthyCard).toHaveTextContent(
      translated("relay.reachability.reachable"),
    );
    expect(healthyCard).toHaveTextContent(
      translated("relay.authorization.valid"),
    );

    const stalledCard = screen
      .getAllByTestId("relay-health")
      .find((element) => element.dataset.network === "stalled");
    expect(stalledCard).toHaveAttribute("data-relay-health", "degraded");
    expect(stalledCard).toHaveTextContent(
      translated("relay.issue.forwarding_stalled"),
    );

    const quotaCard = screen
      .getAllByTestId("relay-health")
      .find((element) => element.dataset.network === "quota");
    expect(quotaCard).toHaveAttribute("data-relay-health", "quota_exhausted");
    expect(quotaCard).toHaveTextContent(translated("relay.quota.limited"));
    expect(quotaCard).toHaveTextContent(
      translated("relay.issue.quota_exhausted"),
    );

    const unsupportedCard = screen
      .getAllByTestId("relay-health")
      .find((element) => element.dataset.network === "unsupported");
    expect(unsupportedCard).toHaveTextContent(
      translated("relay.issue.cpu_measurement_unsupported"),
    );
    expect(unsupportedCard).toHaveTextContent(
      translated("relay.issue.memory_measurement_unsupported"),
    );
    expect(
      screen.getAllByText(translated("relay.measurement.unsupported")),
    ).toHaveLength(2);
    expect(
      screen.getAllByText(translated("relay.limit_source.network_acl")).length,
    ).toBeGreaterThan(0);
    const healthyUsage = screen
      .getAllByTestId("relay-usage")
      .find((element) => element.dataset.network === "healthy");
    expect(healthyUsage).toHaveTextContent("4096 B");
  });

  it("explains the provider/consumer authorization intersection without exposing sensitive internals", () => {
    const authorization: RelayAuthorizationAuditSnapshot = {
      network_id: "net_owner_relay",
      provider_node_id: "relay-provider-01",
      policy_revision: 17,
      policy_updated_at_unix_ms: Date.UTC(2026, 0, 2, 0, 0, 0),
      existing_session_policy: "reauthorize",
      decisions: [
        {
          node_id: "node-member-allowed",
          allowed: true,
          provider: {
            allowed: true,
            reason: "allowed",
            layer: "network",
            network_acl_revision: 12,
            local_policy_revision: 4,
            reauthentication_satisfied: true,
          },
          consumer: {
            allowed: true,
            reason: "allowed",
            layer: "local",
            network_acl_revision: 12,
            local_policy_revision: 4,
            reauthentication_satisfied: true,
          },
          final_reason: null,
        },
        {
          node_id: "node-member-denied",
          allowed: false,
          provider: {
            allowed: true,
            reason: "allowed",
            layer: "network",
            network_acl_revision: 12,
            local_policy_revision: 4,
            reauthentication_satisfied: true,
          },
          consumer: {
            allowed: false,
            reason: "explicit_deny",
            layer: "local",
            network_acl_revision: 12,
            local_policy_revision: 4,
            reauthentication_satisfied: false,
          },
          final_reason: "explicit_deny",
        },
      ],
      truncated: false,
    };
    const snapshot: NodeSnapshot = {
      ...baseSnapshot,
      embedded_services: [
        {
          network_name: "owner-relay",
          status: "running",
          relay_status: "running",
          relay_addr: "127.0.0.1:6667",
          relay_health: relayHealth(),
          relay_authorization: {
            ...authorization,
            decisions: authorization.decisions.map((decision) => ({
              ...decision,
              rule_id: "rule-must-not-render",
              grant_id: "grant-must-not-render",
              public_key: "public-key-must-not-render",
              virtual_ip: "2001:db8::relay-secret",
            })),
          } as RelayAuthorizationAuditSnapshot,
        },
      ],
    };

    render(relayElement(snapshot));

    const panel = screen.getByTestId("relay-authorization");
    expect(panel).toHaveAttribute("data-network", "owner-relay");
    expect(panel).toHaveTextContent(
      translated("gpui.section.relay_authorization"),
    );
    expect(panel).toHaveTextContent("17");
    expect(panel).toHaveTextContent(
      translated("relay.authorization.snapshot_detail"),
    );

    const decisions = screen.getAllByTestId("relay-authorization-decision");
    expect(decisions).toHaveLength(2);
    expect(decisions[0]).toHaveAttribute("data-allowed", "true");
    expect(decisions[0]).toHaveTextContent(
      translated("relay.authorization.allowed"),
    );
    expect(decisions[1]).toHaveAttribute("data-allowed", "false");
    expect(decisions[1]).toHaveTextContent(
      translated("relay.authorization.denied"),
    );
    expect(decisions[1]).toHaveTextContent(
      translated("relay.authorization.decision_detail"),
    );
    expect(decisions[1]).toHaveAttribute(
      "data-provider-reason",
      "allowed",
    );
    expect(decisions[1]).toHaveAttribute(
      "data-consumer-reason",
      "explicit_deny",
    );
    expect(decisions[1]).toHaveAttribute("data-provider-layer", "network");
    expect(decisions[1]).toHaveAttribute("data-consumer-layer", "local");
    expect(decisions[1]).toHaveAttribute("data-consumer-reauth", "false");
    expect(decisions[1]).toHaveAttribute("data-consumer-allowed", "false");

    for (const secret of [
      "rule-must-not-render",
      "grant-must-not-render",
      "public-key-must-not-render",
      "2001:db8::relay-secret",
    ]) {
      expect(panel.innerHTML).not.toContain(secret);
    }
  });

  it("does not render owner-only authorization diagnostics for a member projection", () => {
    const snapshot: NodeSnapshot = {
      ...baseSnapshot,
      embedded_services: [
        {
          network_name: "member-relay",
          status: "running",
          relay_status: "running",
          relay_addr: "127.0.0.1:6667",
          relay_health: relayHealth(),
        },
      ],
    };

    render(relayElement(snapshot));

    expect(screen.queryByTestId("relay-authorization")).not.toBeInTheDocument();
    expect(
      screen.queryByText(translated("gpui.section.relay_authorization")),
    ).not.toBeInTheDocument();
  });

  it("renders the owner-only governance ledger with bounded payload-free fields", () => {
    const events = Array.from({ length: 52 }, (_, index) => ({
      action:
        index === 2
          ? ("session_rejected" as const)
          : ("recovered" as const),
      occurred_at_unix_ms: Date.UTC(2026, 0, 2, 0, 0, index),
      node_id: "node-member-01",
      reason_code:
        index === 2
          ? "relay.session.quota_exhausted.2"
          : `relay.recovered.${index}`,
      observed: index === 2 ? 16 : null,
      limit: index === 2 ? 16 : null,
      reset_at_unix_ms:
        index === 2 ? Date.UTC(2026, 0, 2, 0, 5, 0) : null,
      payload: "TOP_SECRET_PAYLOAD",
      destination: "2001:db8::secret",
    }));
    const snapshot: NodeSnapshot = {
      ...baseSnapshot,
      embedded_services: [
        {
          network_name: "owner-relay",
          status: "running",
          relay_status: "running",
          relay_addr: "127.0.0.1:6667",
          relay_health: relayHealth(),
          relay_governance: {
            events,
            dropped_events: 7,
          } as RelayGovernanceAuditSnapshot,
        },
      ],
    };

    render(relayElement(snapshot));

    const governance = screen.getByTestId("relay-governance");
    expect(governance).toHaveAttribute("data-network", "owner-relay");
    expect(governance).toHaveTextContent(
      translated("gpui.section.relay_governance"),
    );
    expect(governance).toHaveTextContent(
      translated("relay.governance.action.session_rejected"),
    );
    const renderedEvents = screen.getAllByTestId("relay-governance-event");
    expect(renderedEvents).toHaveLength(50);
    expect(
      renderedEvents.every((event) =>
        event.textContent?.includes(
          translated("relay.governance.event.reason"),
        ),
      ),
    ).toBe(true);
    expect(
      renderedEvents[0].textContent?.includes(
        translated("relay.governance.event.node"),
      ),
    ).toBe(true);
    expect(renderedEvents[0]).toHaveAttribute(
      "data-action",
      "session_rejected",
    );
    expect(renderedEvents[0]).toHaveAttribute(
      "data-occurred-at",
      String(Date.UTC(2026, 0, 2, 0, 0, 2)),
    );
    expect(renderedEvents[renderedEvents.length - 1]).toHaveAttribute(
      "data-occurred-at",
      String(Date.UTC(2026, 0, 2, 0, 0, 51)),
    );
    expect(governance).toHaveTextContent(
      translated("relay.governance.dropped"),
    );
    expect(governance).toHaveTextContent(
      translated("relay.governance.bounded"),
    );
    expect(screen.getAllByTestId("relay-governance-event")).toHaveLength(50);
    expect(governance).not.toHaveTextContent("TOP_SECRET_PAYLOAD");
    expect(governance).not.toHaveTextContent("2001:db8::secret");
    expect(governance).not.toHaveTextContent("private_key");
  });

  it("filters the live governance ledger by exact trimmed node id", async () => {
    const user = userEvent.setup();
    const events = Array.from({ length: 60 }, (_, index) => ({
      action:
        index % 2 === 0
          ? ("session_rejected" as const)
          : ("recovered" as const),
      occurred_at_unix_ms: Date.UTC(2026, 0, 2, 1, 0, index),
      node_id:
        index % 2 === 0 ? "node-member-01" : "node-member-02",
      reason_code: `relay.test.${index}`,
      observed: null,
      limit: null,
      reset_at_unix_ms: null,
      payload: "FILTER_SECRET_PAYLOAD",
      destination: "2001:db8::filter-secret",
    }));
    const snapshot: NodeSnapshot = {
      ...baseSnapshot,
      embedded_services: [
        {
          network_name: "owner-relay",
          status: "running",
          relay_status: "running",
          relay_addr: "127.0.0.1:6667",
          relay_health: relayHealth(),
          relay_governance: {
            events,
            dropped_events: 0,
          } as RelayGovernanceAuditSnapshot,
        },
      ],
    };

    render(relayElement(snapshot));

    const governance = screen.getByTestId("relay-governance");
    const count = screen.getByTestId("relay-governance-count");
    const input = screen.getByTestId("relay-governance-node-filter");
    const clear = screen.getByTestId("relay-governance-clear-node-filter");
    expect(count).toHaveTextContent("60");
    expect(screen.getAllByTestId("relay-governance-event")).toHaveLength(50);
    expect(clear).toBeDisabled();

    await user.type(input, " node-member-01 ");
    expect(count).toHaveTextContent("30");
    expect(screen.getByTestId("relay-governance-filter-count")).toHaveTextContent(
      translated("relay.governance.node_filter.matches"),
    );
    const filteredEvents = screen.getAllByTestId("relay-governance-event");
    expect(filteredEvents).toHaveLength(30);
    expect(
      filteredEvents.every(
        (event) => event.dataset.nodeId === "node-member-01",
      ),
    ).toBe(true);

    await user.clear(input);
    await user.type(input, "node-missing");
    expect(count).toHaveTextContent("0");
    expect(screen.getByTestId("relay-governance-filter-empty")).toHaveTextContent(
      translated("relay.governance.node_filter.empty"),
    );
    expect(
      screen.queryByTestId("relay-governance-event"),
    ).not.toBeInTheDocument();

    await user.click(clear);
    expect(input).toHaveValue("");
    expect(count).toHaveTextContent("60");
    expect(screen.getAllByTestId("relay-governance-event")).toHaveLength(50);
    expect(
      screen.queryByTestId("relay-governance-filter-empty"),
    ).not.toBeInTheDocument();
    expect(governance).not.toHaveTextContent("FILTER_SECRET_PAYLOAD");
    expect(governance).not.toHaveTextContent("2001:db8::filter-secret");
  });

  it("exports durable Relay audit for the resolved network and selected time range", async () => {
    const user = userEvent.setup();
    const onExport = vi.fn(async () => ({
      filename: "owner-security-audit-export.json",
      record_count: 2,
    }));
    const from = Date.UTC(2026, 0, 1, 0, 0, 0);
    const to = Date.UTC(2026, 0, 2, 0, 0, 0);
    render(relayElement(relayAuditSnapshot(), onExport));

    const panel = screen.getByTestId("relay-durable-audit");
    expect(panel).toHaveAttribute("data-network", "owner-relay");
    expect(panel).toHaveTextContent(
      translated("relay.durable_audit.scope"),
    );
    expect(panel).toHaveTextContent(
      translated("relay.durable_audit.persistent"),
    );
    expect(panel).not.toHaveTextContent("AUDIT_PANEL_PAYLOAD_SENTINEL");
    expect(panel).not.toHaveTextContent("2001:db8::audit-panel-secret");

    fireEvent.change(
      screen.getByLabelText(translated("relay.durable_audit.from")),
      { target: { value: datetimeLocal(from) } },
    );
    fireEvent.change(
      screen.getByLabelText(translated("relay.durable_audit.to")),
      { target: { value: datetimeLocal(to) } },
    );
    expect(relayAuditExportButton()).toBeDisabled();
    await user.click(relayAuditCheckbox());
    await user.click(relayAuditExportButton());

    expect(onExport).toHaveBeenCalledOnce();
    expect(onExport).toHaveBeenCalledWith({
      network_id: "net_owner_relay",
      from_unix_ms: from,
      to_unix_ms: to,
    });
    const success = await screen.findByTestId("relay-durable-audit-success");
    expect(success).toHaveTextContent(
      translated("relay.durable_audit.success"),
    );
    expect(
      screen.getByTestId("relay-durable-audit-filename"),
    ).toHaveTextContent("owner-security-audit-export.json");
  });

  it("requires confirmation and a non-inverted time range before export", async () => {
    const user = userEvent.setup();
    const onExport = vi.fn(async () => ({
      filename: "owner-security-audit-export.json",
      record_count: 1,
    }));
    render(relayElement(relayAuditSnapshot(), onExport));

    expect(relayAuditExportButton()).toBeDisabled();
    await user.click(relayAuditCheckbox());
    expect(relayAuditExportButton()).toBeEnabled();

    fireEvent.change(
      screen.getByLabelText(translated("relay.durable_audit.from")),
      { target: { value: datetimeLocal(Date.UTC(2026, 0, 2)) } },
    );
    fireEvent.change(
      screen.getByLabelText(translated("relay.durable_audit.to")),
      { target: { value: datetimeLocal(Date.UTC(2026, 0, 1)) } },
    );
    await user.click(relayAuditExportButton());

    expect(onExport).not.toHaveBeenCalled();
    expect(screen.getByRole("alert")).toHaveTextContent(
      translated("relay.durable_audit.invalid_range"),
    );

    await user.click(relayAuditCheckbox());
    expect(relayAuditExportButton()).toBeDisabled();
  });

  it("renders verifiable empty exports and stable export failures without claiming success", async () => {
    const user = userEvent.setup();
    const onExport = vi
      .fn()
      .mockResolvedValueOnce({
        filename: "empty-owner-security-audit-export.json",
        record_count: 0,
      })
      .mockRejectedValueOnce(new Error("internal URL and payload"));
    render(relayElement(relayAuditSnapshot(), onExport));

    await user.click(relayAuditCheckbox());
    await user.click(relayAuditExportButton());
    const success = await screen.findByTestId("relay-durable-audit-success");
    expect(success).toHaveTextContent(
      translated("relay.durable_audit.empty_success"),
    );
    expect(
      screen.getByTestId("relay-durable-audit-filename"),
    ).toHaveTextContent("empty-owner-security-audit-export.json");

    await user.click(relayAuditCheckbox());
    await user.click(relayAuditExportButton());
    expect(
      await screen.findByTestId("relay-durable-audit-error"),
    ).toHaveTextContent(translated("relay.durable_audit.failed"));
    expect(screen.queryByTestId("relay-durable-audit-success")).not.toBeInTheDocument();
    expect(screen.queryByText(/internal URL and payload/)).not.toBeInTheDocument();
  });

  it("disables durable export when the network ID or callback is unavailable", () => {
    const onExport = vi.fn(async () => ({
      filename: "owner-security-audit-export.json",
      record_count: 0,
    }));
    const missingMembership = relayAuditSnapshot();
    missingMembership.memberships = [];
    const { rerender } = render(relayElement(missingMembership, onExport));
    expect(relayAuditExportButton()).toBeDisabled();
    expect(
      screen.getByTestId("relay-durable-audit-unavailable"),
    ).toHaveTextContent(translated("relay.durable_audit.network_unresolved"));

    rerender(relayElement(relayAuditSnapshot()));
    expect(relayAuditExportButton()).toBeDisabled();
    expect(
      screen.getByTestId("relay-durable-audit-unavailable"),
    ).toHaveTextContent(translated("relay.durable_audit.unavailable"));
  });

  it("does not infer healthy from a running listener without a health snapshot", () => {
    const snapshot: NodeSnapshot = {
      ...baseSnapshot,
      embedded_services: [
        {
          network_name: "listener-only",
          status: "running",
          relay_status: "running",
          relay_addr: "127.0.0.1:6667",
          relay_health: null,
        },
      ],
    };

    render(relayElement(snapshot));

    const missing = screen.getByTestId("relay-health-missing");
    expect(missing).toHaveTextContent(translated("relay.status.no_health"));
    expect(missing).toHaveTextContent(translated("relay.health.unknown"));
    expect(missing).not.toHaveTextContent(translated("relay.health.healthy"));
  });
});
