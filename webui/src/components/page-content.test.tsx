import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import manifestFixture from "../../../contracts/typescript/fixtures/node-manifest.json";
import productFixture from "../../../contracts/typescript/fixtures/product-visual-snapshot.json";
import type {
  ActivityEntry,
  NodeDiagnosticsSnapshot,
  NodeExitSelectionSnapshot,
  NodeProblem,
  NodeSnapshot,
  NodeUiManifest,
  NodeUiPage,
} from "../../../contracts/typescript/node-contract-v1";
import type { ActivityPresentationSignals } from "../activity";
import { PageContent } from "./page-content";
import { installTestI18nRuntime, translated } from "../test/runtime";

const manifest = manifestFixture as NodeUiManifest;
const networksPage = manifest.pages.find((page) => page.id === "networks") as NodeUiPage;
const activityPage = manifest.pages.find((page) => page.id === "activity") as NodeUiPage;
const settingsPage = manifest.pages.find((page) => page.id === "settings") as NodeUiPage;
const exitNodePage = manifest.pages.find((page) => page.id === "exit_node") as NodeUiPage;
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

function exitElement(
  snapshot: NodeSnapshot,
  recentDiagnostics: NodeDiagnosticsSnapshot | null = null,
  onRequestAction = vi.fn(),
) {
  return (
    <PageContent
      page={exitNodePage}
      snapshot={snapshot}
      locale="zh-CN"
      selectedNetwork="home"
      recentDiagnostics={recentDiagnostics}
      onSelectNetwork={() => {}}
      onNavigate={() => {}}
      onRequestAction={onRequestAction}
      onRunAction={async () => {}}
    />
  );
}

function diagnosticsWithEvidence(
  observedAt: number,
  maxAgeMs = 60_000,
): NodeDiagnosticsSnapshot {
  return {
    diagnostic_id: "diagnostic-exit-quality",
    started_at_unix_ms: observedAt - 1_000,
    completed_at_unix_ms: observedAt,
    timed_out: false,
    lifecycle: baseSnapshot.lifecycle,
    retained_owner_networks: [],
    cleanup_candidate_networks: [],
    residue: {
      sessions_remaining: 0,
      listeners_remaining: 0,
      peers_remaining: 0,
      embedded_services_remaining: 0,
      configuration_retained: false,
      residual_networks: [],
      cleanup_steps: [],
      issues: [],
    },
    checks: [],
    exit_health: {
      max_age_ms: maxAgeMs,
      evidence: [],
    },
    exit_health_assessment: {
      state: "available",
      default_candidate: true,
      reason_code: "exit.health.available",
      affected_layers: [],
      affected_targets: [],
      affected_protocols: [],
      fresh_evidence_count: 1,
      observed_at_unix_ms: observedAt,
      source: "controlled_probe",
    },
    leak_detection: {
      generated_at_unix_ms: observedAt,
      max_age_ms: maxAgeMs,
      environment: {},
      evidence: [],
    },
    leak_detection_assessment: {
      state: "leak_free",
      reason_code: "leak_detection.leak_free",
      fail_closed_required: false,
      affected_protocols: [],
      affected_targets: [],
    },
    recovery_actions: [],
    sanitized_report: "diagnostic_id=diagnostic-exit-quality",
  };
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
  vi.useRealTimers();
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
    expect(governance).toHaveTextContent(
      translated("exit_governance.title"),
    );
    expect(governance).toHaveTextContent("net_governance");
    expect(governance).toHaveTextContent(translated("exit_governance.mode.fixed"));
    const selected = within(governance).getByTestId(
      "exit-candidate-net_governance-exit-a",
    );
    expect(selected).toHaveTextContent(translated("exit_governance.selectable"));
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
    expect(within(unavailable).getByTestId("exit-governance-reason")).toHaveTextContent(
      "exit.selection.stale_directory",
    );
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

describe("Exit connection evidence and activity", () => {
  const networkId = "net_exit_evidence";
  const selection: NodeExitSelectionSnapshot = {
    network_id: networkId,
    policy_revision: 7,
    policy: {
      mode: { mode: "fixed", exit: "exit-a" },
      default_exit: "exit-a",
      available_exits: ["exit-a", "exit-b"],
    },
    resolution: {
      policy_revision: 7,
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
      ],
      selectable_exits: ["exit-a"],
      selected_default: "exit-a",
    },
    evaluated_at_unix_ms: Date.UTC(2026, 0, 2, 3, 4, 6),
    reason_code: null,
  };
  const authoritativeSnapshot: NodeSnapshot = {
    ...baseSnapshot,
    memberships: [
      {
        ...policySnapshot(false).memberships![0],
        network_name: "home",
        network_id: networkId,
      },
    ],
    configuration: {
      ...baseSnapshot.configuration!,
      networks: baseSnapshot.configuration!.networks.map((network) =>
        network.network_name === "home"
          ? { ...network, exit_node: "config-fallback-exit" }
          : network,
      ),
    },
    exit_selections: [selection],
  };

  it("separates network Exit configuration from the authoritative selection source", () => {
    render(exitElement(authoritativeSnapshot));
    const summary = screen.getByTestId("exit-connection-summary");
    const globalSession = within(summary).getByTestId(
      "exit-global-traffic-session",
    );
    const home = within(summary).getByTestId("exit-connection-home");
    const value = (key: string) =>
      within(home).getByText(translated(key)).nextElementSibling;

    expect(globalSession).toHaveTextContent(translated("exit_summary.protection"));
    expect(globalSession).toHaveTextContent(
      translated("traffic.session.not_observed"),
    );
    expect(home).not.toHaveTextContent(translated("exit_summary.protection"));
    expect(value("exit_summary.configured_exit")).toHaveTextContent("exit-a");
    expect(value("exit_governance.mode")).toHaveTextContent(
      translated("exit_governance.mode.fixed"),
    );
    expect(value("exit_governance.default_exit")).toHaveTextContent("exit-a");
    expect(value("exit_governance.fixed_exit")).toHaveTextContent("exit-a");
    expect(value("exit_summary.selection_source")).toHaveTextContent(
      translated("exit_governance.source.member"),
    );
    expect(home).not.toHaveTextContent("config-fallback-exit");
  });

  it("renders one authoritative traffic session instead of a state per network", () => {
    render(
      exitElement({
        ...authoritativeSnapshot,
        traffic_session: {
          state: "fail_closed",
          changed_at_unix_ms: 5_000,
          reason_code: "traffic.session.exit_lost",
          failure: "exit_unavailable",
          events: [],
        },
      }),
    );

    const summary = screen.getByTestId("exit-connection-summary");
    const globalSession = within(summary).getByTestId(
      "exit-global-traffic-session",
    );
    const home = within(summary).getByTestId("exit-connection-home");
    const studio = within(summary).getByTestId("exit-connection-studio");

    expect(summary.querySelectorAll("[data-traffic-state]")).toHaveLength(1);
    expect(globalSession).toHaveAttribute("data-traffic-state");
    expect(globalSession).toHaveTextContent(
      translated("traffic.session.state.fail_closed"),
    );
    for (const network of [home, studio]) {
      expect(network).not.toHaveTextContent(
        translated("traffic.session.state.fail_closed"),
      );
      expect(network.querySelector("[data-traffic-state]")).toBeNull();
      expect(network.querySelector("[data-protected]")).toBeNull();
    }
  });

  it("pairs repeated Exit node IDs with peers from the same network", () => {
    const sharedExit = "exit-shared";
    const homeNetworkId = "net_exit_home";
    const studioNetworkId = "net_exit_studio";
    const selectionFor = (
      networkId: string,
    ): NodeExitSelectionSnapshot => ({
      ...selection,
      network_id: networkId,
      resolution: {
        ...selection.resolution!,
        candidates: selection.resolution!.candidates.map((candidate) => ({
          ...candidate,
          exit: sharedExit,
        })),
        selectable_exits: [sharedExit],
        selected_default: sharedExit,
      },
    });
    const membership = policySnapshot(false).memberships![0];
    const snapshot: NodeSnapshot = {
      ...baseSnapshot,
      memberships: [
        { ...membership, network_name: "home", network_id: homeNetworkId },
        {
          ...membership,
          network_name: "studio",
          network_id: studioNetworkId,
        },
      ],
      exit_selections: [
        selectionFor(homeNetworkId),
        selectionFor(studioNetworkId),
      ],
      peers: [
        {
          ...baseSnapshot.peers[0],
          peer_id: sharedExit,
          network_name: "studio",
          rtt_us: 22_000,
          loss_rate: 0.2,
        },
        {
          ...baseSnapshot.peers[1],
          peer_id: sharedExit,
          network_name: "home",
          rtt_us: 11_000,
          loss_rate: 0.1,
        },
      ],
    };

    render(exitElement(snapshot));
    const peers = within(screen.getByTestId("exit-quality")).getAllByTestId(
      `exit-quality-peer-${sharedExit}`,
    );

    expect(peers).toHaveLength(2);
    expect(peers[0]).toHaveTextContent("home / exit-shared");
    expect(
      peers[0].querySelector("[data-rtt-us]"),
    ).toHaveAttribute("data-rtt-us", "11000");
    expect(
      peers[0].querySelector("[data-loss-rate]"),
    ).toHaveAttribute("data-loss-rate", "0.1");
    expect(peers[1]).toHaveTextContent("studio / exit-shared");
    expect(
      peers[1].querySelector("[data-rtt-us]"),
    ).toHaveAttribute("data-rtt-us", "22000");
    expect(
      peers[1].querySelector("[data-loss-rate]"),
    ).toHaveAttribute("data-loss-rate", "0.2");
  });

  it("shows an explicit empty state when no network Exit configuration exists", () => {
    render(
      exitElement({
        ...baseSnapshot,
        configuration: { ...baseSnapshot.configuration!, networks: [] },
      }),
    );
    const summary = screen.getByTestId("exit-connection-summary");
    expect(summary).toHaveTextContent(translated("exit_summary.no_network"));
    expect(within(summary).queryByTestId(/^exit-connection-/)).not.toBeInTheDocument();
  });

  it("does not infer protection or leak freedom without diagnostics", () => {
    const onRequestAction = vi.fn();
    render(exitElement(baseSnapshot, null, onRequestAction));
    const quality = screen.getByTestId("exit-quality");

    expect(quality).toHaveTextContent(translated("exit_quality.not_run"));
    expect(quality).toHaveTextContent(
      translated("exit_quality.throughput_unavailable"),
    );
    expect(quality).not.toHaveTextContent(
      translated("diagnostic.leak_detection.state.leak_free"),
    );
    expect(quality).not.toHaveTextContent(
      translated("traffic.session.state.protected"),
    );

    fireEvent.click(
      within(quality).getByRole("button", {
        name: translated("exit_quality.run"),
      }),
    );
    expect(onRequestAction).toHaveBeenCalledWith("get_node_diagnostics");
  });

  it("distinguishes fresh evidence from stale evidence using server max ages", () => {
    const now = Date.UTC(2026, 0, 2, 3, 5, 0);
    vi.useFakeTimers();
    vi.setSystemTime(now);
    const { rerender } = render(
      exitElement(baseSnapshot, diagnosticsWithEvidence(now - 1_000)),
    );

    let quality = screen.getByTestId("exit-quality-diagnostics");
    for (const labelText of [
      translated("exit_quality.health"),
      translated("exit_quality.leak"),
    ]) {
      const term = within(quality).getByText(labelText);
      expect(term.closest("div")?.querySelector("dd")).toHaveTextContent(
        translated("exit_quality.evidence_fresh"),
      );
    }

    rerender(
      exitElement(
        baseSnapshot,
        diagnosticsWithEvidence(now - 60_001),
      ),
    );
    quality = screen.getByTestId("exit-quality-diagnostics");
    for (const labelText of [
      translated("exit_quality.health"),
      translated("exit_quality.leak"),
    ]) {
      const term = within(quality).getByText(labelText);
      expect(term.closest("div")?.querySelector("dd")).toHaveTextContent(
        translated("exit_quality.evidence_stale"),
      );
    }
  });

  it("orders Exit activity newest-first, deduplicates audit IDs, and keeps recovery events", () => {
    const snapshot: NodeSnapshot = {
      ...baseSnapshot,
      traffic_session: {
        state: "protected",
        changed_at_unix_ms: 5_000,
        events: [
          {
            kind: "failure_detected",
            state: "fail_closed",
            occurred_at_unix_ms: 3_000,
            reason_code: "exit.unavailable",
            failure: "handshake_interrupted",
          },
          {
            kind: "protection_established",
            state: "protected",
            occurred_at_unix_ms: 5_000,
            reason_code: "recovery.complete",
            failure: null,
          },
        ],
      },
      recent_activity: [
        activity({
          audit_id: "audit-exit-role",
          action: "network.role.set",
          occurred_at_unix_ms: 4_000,
        }),
        activity({
          audit_id: "audit-exit-role",
          action: "network.role.set",
          occurred_at_unix_ms: 4_000,
        }),
        activity({
          audit_id: "audit-unrelated",
          action: "network.create",
          occurred_at_unix_ms: 6_000,
        }),
      ],
    };

    render(exitElement(snapshot));
    const timeline = screen.getByTestId("exit-activity");
    const titles = Array.from(
      timeline.querySelectorAll<HTMLElement>(".diagnostic-row strong"),
    ).map((node) => node.textContent);

    expect(titles).toEqual([
      translated("traffic.session.event.protection_established"),
      translated("activity.action.network.role.set"),
      translated("traffic.session.event.failure_detected"),
    ]);
    expect(timeline).toHaveTextContent("recovery.complete");
    expect(
      within(timeline).getAllByText(
        translated("activity.action.network.role.set"),
      ),
    ).toHaveLength(1);
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

    expect(card).toHaveTextContent("translated:membership.policy.confirmation_required");
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
    expect(onRequestAction).toHaveBeenCalledWith("acknowledge_membership_policy", {
      network_id: "net_policy_home",
      expected_terms_hash: currentTermsHash,
      acknowledged_at_unix_ms: "1700000200123",
      exit_contribution_acknowledged: "true",
    });
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

    expect(screen.getByTestId("activity-entry-audit-security")).toHaveTextContent(
      "latest duplicate",
    );
    expect(screen.queryByText("stale duplicate")).not.toBeInTheDocument();
    expect(screen.getAllByTestId(/^activity-entry-/)).toHaveLength(3);

    fireEvent.change(
      screen.getByLabelText("translated:activity.filter.from"),
      { target: { value: datetimeLocal(start) } },
    );
    fireEvent.change(screen.getByLabelText("translated:activity.filter.to"), {
      target: { value: datetimeLocal(end) },
    });

    expect(screen.getByTestId("activity-entry-audit-security")).toBeInTheDocument();
    expect(screen.getByTestId("activity-entry-audit-failure")).toBeInTheDocument();
    expect(screen.queryByTestId("activity-entry-audit-outside")).not.toBeInTheDocument();

    await user.selectOptions(
      screen.getByLabelText("translated:activity.filter.category"),
      "operation_failure",
    );
    expect(screen.queryByTestId("activity-entry-audit-security")).not.toBeInTheDocument();
    expect(screen.getByTestId("activity-entry-audit-failure")).toBeInTheDocument();
  });

  it("preserves the reader scroll position when a live snapshot revision arrives", () => {
    const snapshot: NodeSnapshot = {
      ...baseSnapshot,
      revision: 20,
      recent_activity: [
        activity({ audit_id: "audit-a", occurred_at_unix_ms: Date.UTC(2026, 0, 1, 12, 0, 1) }),
        activity({ audit_id: "audit-b", occurred_at_unix_ms: Date.UTC(2026, 0, 1, 12, 0, 0) }),
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
    expect(screen.getByTestId("activity-entry-audit-preserved")).toBeInTheDocument();

    rerender(activityElement(snapshot, { stale: false, refreshing: true }));
    expect(screen.queryByTestId("activity-sync-stale")).not.toBeInTheDocument();
    expect(screen.getByTestId("activity-sync-refreshing")).toHaveTextContent(
      "translated:activity.sync.refreshing.title",
    );
    expect(screen.getByTestId("activity-entry-audit-preserved")).toBeInTheDocument();
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
    expect(screen.getByTestId("activity-object-link-audit-node-link")).toBeInTheDocument();

    rerender(
      activityElement({
        ...snapshot,
        permissions: { ...snapshot.permissions!, permissions: [] },
      }),
    );
    expect(screen.queryByTestId("activity-object-link-audit-node-link")).not.toBeInTheDocument();

    rerender(activityElement({ ...snapshot, identity: null }));
    expect(screen.queryByTestId("activity-object-link-audit-node-link")).not.toBeInTheDocument();
  });
});
