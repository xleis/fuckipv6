import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import manifestFixture from "../../contracts/typescript/fixtures/node-manifest.json";
import productFixture from "../../contracts/typescript/fixtures/product-visual-snapshot.json";
import type {
  ActivityEntry,
  ConfigurationDocument,
  ConfigurationHistoryEntry,
  NodeApiCommandResponse,
  NodeCommandOutcome,
  NodeDiagnosticsSnapshot,
  NodeDiscoveryIndexRecordSnapshot,
  NodeDiscoveryIndexSnapshot,
  NodeOperationSnapshot,
  NodeRoutingSummarySnapshot,
  NodeSnapshot,
  NodeUiManifest,
} from "../../contracts/typescript/node-contract-v1";
import { App } from "./App";
import {
  AuthenticationError,
  cancelOperation,
  HttpError,
  ReauthenticationRequiredError,
  loadManifest,
  loadOperation,
  loadSnapshot,
  sendCommand,
  waitForOperation,
} from "./api/client";
import {
  loadLanguagePreference,
  loadOwnerSession,
  reauthenticateWithPasskey,
} from "./api/session";
import { installTestI18nRuntime } from "./test/runtime";

vi.mock("./api/client", async (importOriginal) => {
  const original = await importOriginal<typeof import("./api/client")>();
  return {
    ...original,
    cancelOperation: vi.fn(),
    loadManifest: vi.fn(),
    loadOperation: vi.fn(),
    loadSnapshot: vi.fn(),
    sendCommand: vi.fn(),
    waitForOperation: vi.fn(),
  };
});

vi.mock("./api/session", async (importOriginal) => {
  const original = await importOriginal<typeof import("./api/session")>();
  return {
    ...original,
    loadOwnerSession: vi.fn(),
    loadLanguagePreference: vi.fn(),
    saveLanguagePreference: vi.fn(),
    logoutOwnerSession: vi.fn(),
    reauthenticateWithPasskey: vi.fn(),
  };
});

const manifest = manifestFixture as NodeUiManifest;
const populatedSnapshot = productFixture.snapshot as NodeSnapshot;

const snapshot: NodeSnapshot = {
  revision: 11,
  lifecycle: {
    active_networks: 0,
    configured_networks: 0,
    failed_networks: [],
    phase: "stopped",
  },
  networks: [],
  peers: [],
  logs: [],
  embedded_services: [],
  configuration: { networks: [] },
  permissions: {
    access_model: "trusted_control_plane",
    permissions: ["view_state", "manage_networks"],
  },
  tun_capabilities: {
    schema_version: 1,
    platform: "linux",
    supported: true,
    can_rebuild: true,
    reasons: [],
  },
  control_health: "available",
  identity: null,
  last_error: null,
  capabilities: { capabilities: [] },
};

class SilentWebSocket {
  static instances: SilentWebSocket[] = [];
  static readonly CONNECTING = 0;
  static readonly OPEN = 1;
  static readonly CLOSING = 2;
  static readonly CLOSED = 3;
  readonly readyState = SilentWebSocket.CONNECTING;
  private readonly listeners = new Map<string, Array<(event: Event) => void>>();

  constructor() {
    SilentWebSocket.instances.push(this);
  }

  addEventListener(type: string, listener: (event: Event) => void): void {
    const listeners = this.listeners.get(type) ?? [];
    listeners.push(listener);
    this.listeners.set(type, listeners);
  }

  close(): void {}

  emit(type: string): void {
    for (const listener of this.listeners.get(type) ?? []) {
      listener(new Event(type));
    }
  }
}

function renderApp() {
  const queryClient = new QueryClient({
    defaultOptions: {
      queries: { retry: false },
      mutations: { retry: false },
    },
  });
  const view = render(
    <QueryClientProvider client={queryClient}>
      <App />
    </QueryClientProvider>,
  );
  return { ...view, queryClient };
}

function authenticate(snapshotValue: NodeSnapshot = snapshot): void {
  vi.mocked(loadOwnerSession).mockResolvedValue({ authenticated: true });
  vi.mocked(loadLanguagePreference).mockResolvedValue("zh-CN");
  vi.mocked(loadManifest).mockResolvedValue(manifest);
  vi.mocked(loadSnapshot).mockResolvedValue(snapshotValue);
}

function discoveryRecord(
  name: string,
  overrides: {
    acceptedAtUnixMs?: number;
    visibility?: "public" | "unlisted" | "private";
    joinMode?: "open" | "approval_required" | "invite_only";
    language?: string;
    region?: string;
    tags?: string[];
  } = {},
): NodeDiscoveryIndexRecordSnapshot {
  const now = Date.now();
  return {
    accepted_at_unix_ms: overrides.acceptedAtUnixMs ?? now - 1_000,
    digest: `digest-${name}`,
    discoverable: true,
    invalid_reason: null,
    source: "network",
    record: {
      document: {
        schema_version: 3,
        network_id: `network-${name}`,
        public_key: `public-key-${name}`,
        publisher: `publisher-${name}`,
        administrators: [],
        policy_revision: 1,
        privacy_policy_revision: 1,
        member_limit: null,
        member_count: 1,
        exit_contribution: "not_required",
        privacy_changes: [],
        name,
        purpose: `${name} purpose`,
        language: overrides.language ?? "en",
        region: overrides.region ?? "US",
        tags: overrides.tags ?? ["community"],
        visibility: overrides.visibility ?? "public",
        join_mode: overrides.joinMode ?? "open",
        owner: `owner-${name}`,
        revision: 1,
        issued_at_unix_ms: now - 2_000,
        expires_at_unix_ms: now + 86_400_000,
        capability_summary: ["relay"],
        endpoints: [],
      },
      signature: `signature-${name}`,
    },
  };
}

function discoverySnapshot(
  records: NodeDiscoveryIndexRecordSnapshot[],
  overrides: Partial<NodeDiscoveryIndexSnapshot> = {},
): NodeSnapshot {
  return {
    ...snapshot,
    discovery_index: {
      data_plane_affected: false,
      index_url: "https://discovery.example.test/index",
      last_attempt_unix_ms: Date.now() - 500,
      last_error: null,
      last_success_unix_ms: Date.now() - 1_000,
      records,
      rejected_record_count: 0,
      search_history_stored: false,
      source: "network",
      metrics: {
        request_attempts: 1,
        requests_succeeded: 1,
        requests_failed: 0,
        retries_scheduled: 0,
        cancellations: 0,
        records_accepted: records.length,
        records_rejected: 0,
      },
      state: "ready",
      ...overrides,
    },
  };
}

function activityAt(index: number): ActivityEntry {
  return {
    audit_id: `audit-${index}`,
    actor: "node-owner",
    action: "network.create",
    resource: `resource-${index}`,
    outcome: "succeeded",
    occurred_at_unix_ms: index * 1000,
    summary: `raw-summary-${index}`,
    details: {
      command: `command-${index}`,
      token: `secret-${index}`,
    },
  };
}

function configurationDocument(revision: number): ConfigurationDocument {
  return {
    schema_version: 1,
    revision,
    networks: { home: { mtu: 1280 } },
  };
}

function configurationHistory(revision: number): ConfigurationHistoryEntry {
  return {
    transaction_id: `tx-${revision}`,
    revision,
    previous_revision: revision - 1,
    action: "apply",
    target_revision: null,
    actor: "owner",
    source: "webui",
    occurred_at: "2026-11-02T00:00:00Z",
    changed_paths: ["/networks/home/mtu"],
  };
}

function diagnosticsFixture(): NodeDiagnosticsSnapshot {
  return {
    diagnostic_id: "diagnostic-w7-05",
    started_at_unix_ms: 1_700_000_000_000,
    completed_at_unix_ms: 1_700_000_001_250,
    timed_out: false,
    lifecycle: snapshot.lifecycle,
    retained_owner_networks: [],
    cleanup_candidate_networks: ["home"],
    residue: {
      sessions_remaining: 1,
      listeners_remaining: 0,
      peers_remaining: 0,
      embedded_services_remaining: 0,
      configuration_retained: true,
      residual_networks: ["home"],
      cleanup_steps: [],
      issues: ["cleanup_incomplete"],
    },
    checks: [
      {
        layer: "exit",
        state: "failed",
        started_at_unix_ms: 1_700_000_000_900,
        duration_ms: 120,
        error_code: "diagnostic.exit.unreachable",
        message_key: "diagnostic.message.exit_failed",
        suggestion_key: "diagnostic.suggestion.exit_retry",
      },
      {
        layer: "network",
        state: "passed",
        started_at_unix_ms: 1_700_000_000_100,
        duration_ms: 15,
        error_code: null,
        message_key: "diagnostic.message.network_passed",
        suggestion_key: null,
      },
      {
        layer: "coordinator",
        state: "unsupported",
        started_at_unix_ms: 1_700_000_000_500,
        duration_ms: 30,
        error_code: "diagnostic.coordinator.unsupported",
        message_key: "diagnostic.message.coordinator_unsupported",
        suggestion_key: "diagnostic.suggestion.self_hosted",
      },
      {
        layer: "tun",
        state: "no_permission",
        started_at_unix_ms: 1_700_000_000_300,
        duration_ms: 45,
        error_code: "diagnostic.tun.permission_denied",
        message_key: "diagnostic.message.tun_no_permission",
        suggestion_key: "diagnostic.suggestion.tun_permission",
      },
      {
        layer: "relay",
        state: "passed",
        started_at_unix_ms: 1_700_000_000_700,
        duration_ms: 40,
        error_code: null,
        message_key: "diagnostic.message.relay_passed",
        suggestion_key: null,
      },
    ],
    recovery_actions: [
      {
        action_id: "cleanup_residuals",
        impact_key: "diagnostic.recovery.impact.cleanup_residuals",
        confirmation_required: true,
      },
    ],
    sanitized_report:
      "diagnostic_id=diagnostic-w7-05\nnetwork=passed\ndns=not_tested\nexit=failed",
  };
}

function operationFixture(
  overrides: Partial<NodeOperationSnapshot> = {},
): NodeOperationSnapshot {
  return {
    operation_id: "op-w7-05-diagnostics",
    kind: "diagnostics",
    state: "succeeded",
    stage: "complete",
    attempt: 1,
    max_attempts: 1,
    cancellable: false,
    cancel_blocked_reason: "operation.cancel.not_supported",
    ...overrides,
  } as NodeOperationSnapshot;
}

function mockConfigurationCommands(): void {
  vi.mocked(sendCommand).mockImplementation(
    async (request): Promise<NodeApiCommandResponse> => {
      const command = request.request.command.command;
      let outcome: NodeCommandOutcome;
      if (command === "get_configuration") {
        outcome = { status: "configuration", data: configurationDocument(2) };
      } else if (command === "get_configuration_history") {
        outcome = {
          status: "configuration_history",
          data: [configurationHistory(2)],
        };
      } else {
        throw new Error(`unexpected command ${command}`);
      }
      return {
        schema_version: 1,
        request_id: request.request.request_id,
        server_time: "2026-11-02T00:00:00Z",
        result: { request_id: request.request.request_id, outcome },
      };
    },
  );
}

async function expectModuleTabs(module: string, tabs: string[]): Promise<void> {
  const user = userEvent.setup();
  await user.click(
    screen.getByRole("button", { name: `translated:nav.${module}` }),
  );
  const tablist = screen.getByRole("tablist", {
    name: "translated:common.page_sections",
  });
  expect(
    within(tablist)
      .getAllByRole("tab")
      .map((tab) => tab.textContent),
  ).toEqual(tabs.map((tab) => `translated:nav.${tab}`));
}

beforeEach(() => {
  vi.clearAllMocks();
  installTestI18nRuntime();
  SilentWebSocket.instances = [];
  vi.stubGlobal("WebSocket", SilentWebSocket);
});

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

describe("React WebUI shell", () => {
  it("shows the owner Passkey login surface before authentication", async () => {
    vi.mocked(loadOwnerSession).mockResolvedValue({ authenticated: false });
    renderApp();

    expect(
      await screen.findByRole("heading", {
        name: "translated:owner_auth.login.title",
      }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("button", {
        name: "translated:owner_auth.passkey.login_button",
      }),
    ).toBeDisabled();
    expect(loadManifest).not.toHaveBeenCalled();
    expect(loadSnapshot).not.toHaveBeenCalled();
  });

  it("renders five modules and all twelve manifest-backed secondary pages", async () => {
    authenticate();
    renderApp();

    expect(
      await screen.findByRole("heading", { name: "translated:page.overview" }),
    ).toBeInTheDocument();
    expect(
      screen.getAllByRole("button", {
        name: /^translated:nav\.(overview|nodes|networks|activity|settings)$/,
      }),
    ).toHaveLength(5);

    await expectModuleTabs("overview", ["overview"]);
    await expectModuleTabs("nodes", [
      "peers",
      "node_runtime",
      "routing",
      "identity",
    ]);
    await expectModuleTabs("networks", [
      "networks",
      "topology",
      "exit_node",
      "relay",
    ]);
    await expectModuleTabs("activity", ["traffic", "logs"]);
    await expectModuleTabs("settings", ["settings"]);
  });

  it("shows verifiable identity details with rotation impact and a recovery entry", async () => {
    authenticate({
      ...snapshot,
      identity: {
        node_id: "node-current",
        display_name: "Home node",
        public_key_fingerprint:
          "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
        created_at_unix_ms: 1_700_000_000_000,
      },
    });
    const user = userEvent.setup();
    renderApp();

    await screen.findByRole("heading", { name: "translated:page.overview" });
    await user.click(
      screen.getByRole("button", { name: "translated:nav.nodes" }),
    );
    await user.click(
      screen.getByRole("tab", { name: "translated:nav.identity" }),
    );

    expect(
      await screen.findByRole("heading", { name: "translated:page.identity" }),
    ).toBeInTheDocument();
    expect(screen.getByText("Home node")).toBeInTheDocument();
    expect(screen.getByText("node-current")).toBeInTheDocument();
    expect(
      screen.getByText("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"),
    ).toBeInTheDocument();
    expect(
      screen.getByText("translated:identity.rotation.title"),
    ).toBeInTheDocument();
    expect(
      screen.getByText("translated:identity.rotation.impact"),
    ).toBeInTheDocument();
    expect(
      screen.getByText("translated:identity.security.detail"),
    ).toBeInTheDocument();

    await user.click(
      screen.getByRole("button", {
        name: "translated:identity.security.settings_button",
      }),
    );
    expect(
      await screen.findByRole("heading", { name: "translated:page.settings" }),
    ).toBeInTheDocument();
  });

  it("applies a one-POST identity rotation without polling or a stale refresh", async () => {
    const oldFingerprint = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
    const newFingerprint = "BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB";
    authenticate({
      ...snapshot,
      identity: {
        node_id: "node-current",
        display_name: "Home node",
        public_key_fingerprint: oldFingerprint,
        created_at_unix_ms: 1_700_000_000_000,
      },
    });
    const outcome: NodeCommandOutcome = {
      status: "identity_rotated",
      data: {
        old_node_id: "node-current",
        new_node_id: "node-rotated",
        old_fingerprint: oldFingerprint,
        new_fingerprint: newFingerprint,
        created_at_unix_ms: 1_700_000_100_000,
        affected_configured_network_count: 0,
        owner_initialized: true,
        owner_sessions_invalidated: true,
      },
    };
    vi.mocked(sendCommand).mockImplementation(
      async (request): Promise<NodeApiCommandResponse> => ({
        schema_version: 1,
        request_id: request.request.request_id,
        server_time: "2026-11-02T00:00:00Z",
        result: { request_id: request.request.request_id, outcome },
        operation: {
          operation_id: "op-identity-rotation",
          state: "succeeded",
          stage: "complete",
          attempt: 1,
          max_attempts: 1,
          cancellable: false,
          cancel_blocked_reason: "operation.cancel.not_supported",
          result: { request_id: request.request.request_id, outcome },
        } as NodeOperationSnapshot,
      }),
    );
    const user = userEvent.setup();
    renderApp();

    await screen.findByRole("heading", { name: "translated:page.overview" });
    await user.click(
      screen.getByRole("button", { name: "translated:nav.nodes" }),
    );
    await user.click(
      screen.getByRole("tab", { name: "translated:nav.identity" }),
    );
    const panel = await screen.findByRole("complementary", {
      name: "translated:common.shared_actions",
    });
    await user.click(
      within(panel).getByRole("button", {
        name: "translated:action.rotate_node_identity",
      }),
    );
    expect(
      within(panel).getByRole("textbox", {
        name: "translated:field.expected_fingerprint",
      }),
    ).toHaveValue(oldFingerprint);
    await user.click(
      within(panel).getByRole("button", { name: "translated:common.execute" }),
    );
    const dialog = await screen.findByRole("dialog");
    await user.click(
      within(dialog).getByRole("button", {
        name: "translated:common.execute",
      }),
    );

    expect(
      await within(panel).findByText("translated:outcome.identity_rotated"),
    ).toBeInTheDocument();
    expect(screen.getByText("node-rotated")).toBeInTheDocument();
    expect(screen.getByText(newFingerprint)).toBeInTheDocument();
    expect(screen.queryByText("node-current")).not.toBeInTheDocument();
    expect(sendCommand).toHaveBeenCalledTimes(1);
    expect(loadOperation).not.toHaveBeenCalled();
    expect(loadSnapshot).toHaveBeenCalledTimes(1);
    expect(vi.mocked(sendCommand).mock.calls[0][0].request.command).toEqual({
      command: "rotate_node_identity",
      payload: { expected_fingerprint: oldFingerprint },
    });
  });

  it("renders authoritative interfaces and diagnostics on node runtime", async () => {
    if (!window.FuckIPV6I18n) throw new Error("i18n runtime is missing");
    window.FuckIPV6I18n.tr = (_locale, key, params) =>
      `translated:${key}:${JSON.stringify(params ?? {})}`;
    authenticate({
      ...snapshot,
      lifecycle: { ...snapshot.lifecycle, phase: "running" },
      tun_capabilities: {
        schema_version: 1,
        platform: "linux",
        supported: true,
        can_rebuild: false,
        reasons: ["permission_denied", "device_unavailable"],
      },
      interfaces: [
        {
          index: 7,
          name: "fk-home",
          kind: "tun",
          ownership: "product",
          status: "up",
          mtu: 1420,
          addresses: [{ address: "fd00::2", prefix_len: 64 }],
          default_ipv4: false,
          default_ipv6: true,
        },
      ],
      interface_problems: [
        {
          code: "unavailable",
          category: "dependency",
          message: "raw host interface error",
          message_key: "error.operation_unavailable",
          params: {},
          network_name: "home",
          retryable: true,
        },
      ],
    });
    const user = userEvent.setup();
    renderApp();

    await screen.findByRole("heading", { name: "translated:page.overview" });
    await user.click(
      screen.getByRole("button", { name: "translated:nav.nodes" }),
    );
    await user.click(
      screen.getByRole("tab", { name: "translated:nav.node_runtime" }),
    );

    const interfaces = await screen.findByTestId("runtime-interfaces");
    expect(within(interfaces).getByText("fk-home")).toBeInTheDocument();
    expect(interfaces).toHaveTextContent(
      '"kind":"translated:interface.kind.tun"',
    );
    expect(interfaces).toHaveTextContent(
      '"ownership":"translated:interface.ownership.product"',
    );
    expect(interfaces).toHaveTextContent('"addresses":"fd00::2/64"');
    expect(interfaces).toHaveTextContent(
      "translated:runtime.row.interface_defaults",
    );

    const problems = screen.getByTestId("runtime-interface-problems");
    expect(within(problems).getByText("home")).toBeInTheDocument();
    expect(problems).toHaveTextContent(
      "translated:error.operation_unavailable",
    );
    expect(problems).not.toHaveTextContent("raw host interface error");

    const tun = screen.getByTestId("runtime-tun");
    expect(tun).toHaveTextContent("translated:runtime.row.tun_platform");
    expect(tun).toHaveTextContent(
      "translated:runtime.value.tun_platform.linux",
    );
    expect(tun).toHaveTextContent(
      "translated:runtime.value.tun_reason.permission_denied",
    );
    expect(tun).toHaveTextContent(
      "translated:runtime.value.tun_reason.device_unavailable",
    );
  });

  it("renders a fixed, sanitized diagnostic report with explicit recovery impact", async () => {
    const writeText = vi
      .spyOn(navigator.clipboard, "writeText")
      .mockResolvedValue(undefined);
    authenticate({
      ...snapshot,
      lifecycle: { ...snapshot.lifecycle, phase: "running" },
    });
    const diagnostics = diagnosticsFixture();
    vi.mocked(sendCommand).mockImplementation(
      async (request): Promise<NodeApiCommandResponse> => {
        const outcome: NodeCommandOutcome = {
          status: "diagnostics",
          data: diagnostics,
        };
        const operation = operationFixture({
          result: {
            request_id: request.request.request_id,
            outcome,
          },
        });
        return {
          schema_version: 1,
          request_id: request.request.request_id,
          server_time: "2026-11-03T00:00:00Z",
          result: { request_id: request.request.request_id, outcome },
          operation,
        };
      },
    );
    const user = userEvent.setup();
    renderApp();

    await screen.findByRole("heading", { name: "translated:page.overview" });
    await user.click(
      screen.getByRole("button", { name: "translated:nav.nodes" }),
    );
    await user.click(
      screen.getByRole("tab", { name: "translated:nav.node_runtime" }),
    );
    const panel = await screen.findByRole("complementary", {
      name: "translated:common.shared_actions",
    });
    await user.click(
      within(panel).getByRole("button", {
        name: "translated:action.get_node_diagnostics",
      }),
    );
    await user.click(
      within(panel).getByRole("button", { name: "translated:common.execute" }),
    );

    const diagnosticsPanel = await screen.findByTestId("runtime-diagnostics");
    await waitFor(() =>
      expect(diagnosticsPanel).toHaveTextContent("diagnostic-w7-05"),
    );
    expect(
      Array.from(
        diagnosticsPanel.querySelectorAll("[data-diagnostic-layer]"),
      ).map((row) => [
        row.getAttribute("data-diagnostic-layer"),
        row.getAttribute("data-diagnostic-state"),
      ]),
    ).toEqual([
      ["network", "passed"],
      ["dns", "not_tested"],
      ["tun", "no_permission"],
      ["coordinator", "unsupported"],
      ["relay", "passed"],
      ["exit", "failed"],
    ]);
    expect(diagnosticsPanel).toHaveTextContent(
      "translated:diagnostic.message.tun_no_permission",
    );
    expect(diagnosticsPanel).toHaveTextContent(
      "translated:diagnostic.suggestion.exit_retry",
    );

    const recovery = diagnosticsPanel.querySelector(
      '[data-recovery-action-id="cleanup_residuals"]',
    );
    expect(recovery).toHaveTextContent(
      "translated:diagnostic.recovery.impact.cleanup_residuals",
    );
    expect(recovery).toHaveTextContent(
      "translated:diagnostic.recovery.confirmation_required",
    );
    expect(diagnosticsPanel).not.toHaveTextContent(
      /Bearer|secret|password|token|raw stderr|\/home\//i,
    );

    await user.click(
      within(diagnosticsPanel).getByText("translated:diagnostic.report"),
    );
    await user.click(
      within(diagnosticsPanel).getByRole("button", {
        name: "translated:diagnostic.copy_report",
      }),
    );
    await waitFor(() =>
      expect(writeText).toHaveBeenCalledWith(diagnostics.sanitized_report),
    );
    expect(
      within(diagnosticsPanel).getByRole("status"),
    ).toHaveTextContent("translated:diagnostic.report_copied");
    expect(sendCommand).toHaveBeenCalledTimes(1);
  });

  it("keeps cancellation non-terminal until the authoritative operation lookup finishes", async () => {
    authenticate({
      ...snapshot,
      lifecycle: { ...snapshot.lifecycle, phase: "running" },
    });
    const running = operationFixture({
      operation_id: "op-cancel-diagnostics",
      state: "running",
      cancellable: true,
      cancel_blocked_reason: null,
    });
    let resolveLookup!: (operation: NodeOperationSnapshot) => void;
    const lookup = new Promise<NodeOperationSnapshot>((resolve) => {
      resolveLookup = resolve;
    });
    vi.mocked(sendCommand).mockImplementation(
      async (request): Promise<NodeApiCommandResponse> => ({
        schema_version: 1,
        request_id: request.request.request_id,
        server_time: "2026-11-03T00:00:00Z",
        operation: running,
      }),
    );
    vi.mocked(waitForOperation).mockReturnValue(lookup);
    vi.mocked(cancelOperation).mockResolvedValue(running);
    const user = userEvent.setup();
    renderApp();

    await screen.findByRole("heading", { name: "translated:page.overview" });
    await user.click(
      screen.getByRole("button", { name: "translated:nav.nodes" }),
    );
    await user.click(
      screen.getByRole("tab", { name: "translated:nav.node_runtime" }),
    );
    const panel = await screen.findByRole("complementary", {
      name: "translated:common.shared_actions",
    });
    await user.click(
      within(panel).getByRole("button", {
        name: "translated:action.get_node_diagnostics",
      }),
    );
    await user.click(
      within(panel).getByRole("button", { name: "translated:common.execute" }),
    );

    await waitFor(() =>
      expect(waitForOperation).toHaveBeenCalledWith(
        "op-cancel-diagnostics",
        expect.objectContaining({ intervalMs: 1000 }),
      ),
    );
    const status = await screen.findByTestId("operation-status");
    expect(status).toHaveTextContent("translated:operation.state.running");
    await user.click(
      within(status).getByRole("button", {
        name: "translated:operation.cancel",
      }),
    );
    await waitFor(() =>
      expect(cancelOperation).toHaveBeenCalledWith("op-cancel-diagnostics"),
    );
    expect(status).toHaveTextContent("translated:operation.state.running");

    await act(async () => {
      resolveLookup({
        ...running,
        state: "cancelled",
        stage: "cancelled",
      });
      await lookup;
    });

    await waitFor(() =>
      expect(status).toHaveTextContent("translated:operation.state.cancelled"),
    );
    expect(
      within(status).queryByRole("button", {
        name: "translated:operation.cancel",
      }),
    ).not.toBeInTheDocument();
    expect(sendCommand).toHaveBeenCalledTimes(1);
  });

  it("renders authoritative routing summaries and navigates with the route identity", async () => {
    if (!window.FuckIPV6I18n) throw new Error("i18n runtime is missing");
    window.FuckIPV6I18n.tr = (_locale, key, params) =>
      `translated:${key}:${JSON.stringify(params ?? {})}`;

    const sourceNetwork = populatedSnapshot.networks[1];
    const sourceConfig = populatedSnapshot.configuration?.networks[1];
    if (!sourceNetwork || !sourceConfig)
      throw new Error("network fixture is missing");
    const officeNetwork = {
      ...sourceNetwork,
      name: "office",
      tun_name: "fk-office",
    };
    const rejectNetwork = {
      ...sourceNetwork,
      name: "reject",
      tun_name: "fk-reject",
    };
    const officeConfig = {
      ...sourceConfig,
      network_name: "office",
      tun_name: "fk-office",
    };
    const rejectConfig = {
      ...sourceConfig,
      network_name: "reject",
      tun_name: "fk-reject",
    };
    const routingSummaries: NodeRoutingSummarySnapshot[] = [
      {
        network_name: "home",
        default_action: { kind: "direct" },
        effective_target: null,
        enabled_rule_count: 2,
        last_applied_revision: 7,
        last_applied_at_unix_ms: 1_700_000_000_000,
        status: "current",
        problem: null,
      },
      {
        network_name: "studio",
        default_action: { kind: "network" },
        effective_target: { kind: "network", network_name: "studio" },
        enabled_rule_count: 3,
        last_applied_revision: 8,
        last_applied_at_unix_ms: 1_700_000_001_000,
        status: "stale",
        problem: {
          code: "unavailable",
          category: "dependency",
          message: "raw stopped",
          message_key: "runtime.routing.problem.stopped",
          params: {},
          network_name: "studio",
          retryable: true,
        },
      },
      {
        network_name: "office",
        default_action: { kind: "exit", value: "peer-1" },
        effective_target: { kind: "exit", node_id: "peer-1" },
        enabled_rule_count: 4,
        last_applied_revision: 9,
        last_applied_at_unix_ms: 1_700_000_002_000,
        status: "invalid",
        problem: {
          code: "invalid_request",
          category: "validation",
          message: "raw invalid",
          message_key: "runtime.routing.problem.invalid",
          params: {},
          network_name: "office",
          retryable: false,
        },
      },
      {
        network_name: "reject",
        default_action: { kind: "reject" },
        effective_target: null,
        enabled_rule_count: 5,
        last_applied_revision: 10,
        last_applied_at_unix_ms: 1_700_000_003_000,
        status: "apply_failed",
        problem: {
          code: "internal",
          category: "internal",
          message: "raw rollback",
          message_key: "runtime.routing.problem.rollback_failed",
          params: {},
          network_name: "reject",
          retryable: true,
        },
      },
    ];

    authenticate({
      ...populatedSnapshot,
      lifecycle: { ...populatedSnapshot.lifecycle, phase: "running" },
      networks: [...populatedSnapshot.networks, officeNetwork, rejectNetwork],
      configuration: {
        networks: [
          ...(populatedSnapshot.configuration?.networks ?? []),
          officeConfig,
          rejectConfig,
        ],
      },
      routing_summaries: routingSummaries,
    });
    const user = userEvent.setup();
    renderApp();

    await screen.findByRole("heading", { name: "translated:page.overview" });
    await user.click(
      screen.getByRole("button", { name: "translated:nav.nodes" }),
    );
    await user.click(
      screen.getByRole("tab", { name: "translated:nav.node_runtime" }),
    );

    const routing = await screen.findByTestId("runtime-routing");
    expect(routing.querySelectorAll("[data-routing-network]")).toHaveLength(4);
    expect(
      Array.from(routing.querySelectorAll("[data-routing-status]")).map((row) =>
        row.getAttribute("data-routing-status"),
      ),
    ).toEqual(["current", "stale", "invalid", "apply_failed"]);

    const home = routing.querySelector('[data-routing-network="home"]');
    expect(home).toHaveTextContent("translated:routing.status.current");
    expect(home).toHaveTextContent("translated:routing.action.direct");
    expect(home).toHaveTextContent('"target":"translated:common.none"');
    expect(home).toHaveTextContent('"rules":"2"');
    expect(home).toHaveTextContent('"revision":"7"');
    expect(home).toHaveTextContent('"applied":"2023-11-14T22:13:20.000Z"');

    const studio = routing.querySelector('[data-routing-network="studio"]');
    expect(studio).toHaveTextContent("translated:routing.action.network");
    expect(studio).toHaveTextContent(/translated:routing\.target\.network/);
    expect(studio).toHaveTextContent("studio");
    expect(studio).toHaveTextContent(
      "translated:runtime.routing.problem.stopped",
    );

    const office = routing.querySelector('[data-routing-network="office"]');
    expect(office).toHaveTextContent("translated:routing.action.exit");
    expect(office).toHaveTextContent(/translated:routing\.target\.exit/);
    expect(office).toHaveTextContent("peer-1");
    expect(office).toHaveTextContent(
      "translated:runtime.routing.problem.invalid",
    );

    const reject = routing.querySelector('[data-routing-network="reject"]');
    expect(reject).toHaveTextContent("translated:routing.action.reject");
    expect(reject).toHaveTextContent(
      "translated:runtime.routing.problem.rollback_failed",
    );
    expect(routing).not.toContainElement(
      routing.querySelector('[data-testid="configuration-workspace"]'),
    );

    await user.click(
      within(studio as HTMLElement).getByRole("button", {
        name: "translated:routing.manage",
      }),
    );
    expect(
      await screen.findByRole("heading", { name: "translated:page.routing" }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("combobox", {
        name: "translated:common.current_network",
      }),
    ).toHaveValue("studio");
  });

  it("renders one authoritative page state with node failure ahead of control failure", async () => {
    authenticate({
      ...populatedSnapshot,
      control_health: "unavailable",
      lifecycle: {
        ...populatedSnapshot.lifecycle,
        failed_networks: ["home"],
        phase: "failed",
      },
    });
    const { container } = renderApp();

    await screen.findByRole("heading", { name: "translated:page.overview" });
    const notices = container.querySelectorAll("[data-page-state]");
    expect(notices).toHaveLength(1);
    expect(notices[0]).toHaveAttribute("data-page-state", "node_failed");
    expect(
      container.querySelector('[data-page-state="degraded"]'),
    ).not.toBeInTheDocument();
    expect(
      container.querySelector('[data-page-state="control_unavailable"]'),
    ).not.toBeInTheDocument();
  });

  it.each([
    {
      name: "control service failure",
      value: { ...populatedSnapshot, control_health: "unavailable" as const },
      expected: "control_unavailable",
    },
    {
      name: "degraded operation",
      value: {
        ...populatedSnapshot,
        lifecycle: {
          ...populatedSnapshot.lifecycle,
          failed_networks: ["home"],
        },
      },
      expected: "degraded",
    },
    {
      name: "missing view permission",
      value: {
        ...populatedSnapshot,
        permissions: {
          access_model: "trusted_control_plane" as const,
          permissions: [],
        },
      },
      expected: "permission_denied",
    },
  ])(
    "renders the $name as the single primary state",
    async ({ value, expected }) => {
      authenticate(value);
      const { container } = renderApp();

      await screen.findByRole("heading", { name: "translated:page.overview" });
      await waitFor(() => {
        expect(container.querySelectorAll("[data-page-state]")).toHaveLength(1);
      });
      expect(container.querySelector("[data-page-state]")).toHaveAttribute(
        "data-page-state",
        expected,
      );
    },
  );

  it("shows stale state after the event stream disconnects", async () => {
    authenticate(populatedSnapshot);
    const { container } = renderApp();

    await screen.findByRole("heading", { name: "translated:page.overview" });
    const socket = SilentWebSocket.instances.at(-1);
    if (!socket) throw new Error("WebSocket was not created");
    act(() => socket.emit("close"));

    await waitFor(() => {
      expect(container.querySelector("[data-page-state]")).toHaveAttribute(
        "data-page-state",
        "stale",
      );
    });
    expect(container.querySelectorAll("[data-page-state]")).toHaveLength(1);
  });

  it("maps a temporary control rejection to maintenance while keeping the snapshot", async () => {
    const user = userEvent.setup();
    authenticate(populatedSnapshot);
    const { container } = renderApp();

    await screen.findByRole("heading", { name: "translated:page.overview" });
    vi.mocked(loadSnapshot).mockRejectedValueOnce(new HttpError(503, null));
    await user.click(
      screen.getByRole("button", { name: "translated:common.refresh" }),
    );

    await waitFor(() => {
      expect(container.querySelector("[data-page-state]")).toHaveAttribute(
        "data-page-state",
        "maintenance",
      );
    });
    expect(
      screen.getByRole("heading", { name: "translated:page.overview" }),
    ).toBeInTheDocument();
    expect(container.querySelector(".app-banner")).not.toBeInTheDocument();
  });

  it("shows browser offline as the authoritative state", async () => {
    vi.spyOn(navigator, "onLine", "get").mockReturnValue(false);
    authenticate(populatedSnapshot);
    const { container } = renderApp();

    await screen.findByRole("heading", { name: "translated:page.overview" });
    expect(container.querySelector("[data-page-state]")).toHaveAttribute(
      "data-page-state",
      "offline",
    );
  });

  it("shows unsupported state for a disabled platform capability", async () => {
    const user = userEvent.setup();
    authenticate({
      ...populatedSnapshot,
      capabilities: {
        capabilities:
          populatedSnapshot.capabilities?.capabilities.map((capability) =>
            capability.kind === "exit"
              ? { ...capability, state: "unsupported" as const }
              : capability,
          ) ?? [],
      },
    });
    const { container } = renderApp();

    await screen.findByRole("heading", { name: "translated:page.overview" });
    await user.click(
      screen.getByRole("button", { name: "translated:nav.networks" }),
    );
    await user.click(
      screen.getByRole("tab", { name: "translated:nav.exit_node" }),
    );

    expect(
      await screen.findByRole("heading", { name: "translated:page.exit_node" }),
    ).toBeInTheDocument();
    expect(container.querySelector("[data-page-state]")).toHaveAttribute(
      "data-page-state",
      "unsupported",
    );
  });

  it("keeps an unsaved configuration draft across an expired session", async () => {
    const user = userEvent.setup();
    authenticate(populatedSnapshot);
    mockConfigurationCommands();
    const { queryClient } = renderApp();

    await screen.findByRole("heading", { name: "translated:page.overview" });
    await user.click(
      screen.getByRole("button", { name: "translated:nav.settings" }),
    );
    const editor = await screen.findByRole("textbox", {
      name: "translated:configuration.editor.label",
    });
    await waitFor(() => expect(queryClient.isFetching()).toBe(0));
    const draft = JSON.stringify({ home: { mtu: 1400 } }, null, 2);
    fireEvent.change(editor, { target: { value: draft } });
    expect(editor).toHaveValue(draft);

    vi.mocked(loadSnapshot).mockRejectedValueOnce(new AuthenticationError());
    await user.click(
      screen.getByRole("button", { name: "translated:common.refresh" }),
    );
    expect(
      await screen.findByRole("heading", {
        name: "translated:owner_auth.login.title",
      }),
    ).toBeInTheDocument();

    act(() => {
      queryClient.setQueryData(["owner-session"], { authenticated: true });
    });

    expect(
      await screen.findByRole("heading", { name: "translated:page.settings" }),
    ).toBeInTheDocument();
    const restored = await screen.findByRole("textbox", {
      name: "translated:configuration.editor.label",
    });
    await waitFor(() => expect(restored).toHaveValue(draft));
  });

  it("renders basic and advanced read-only settings with explicit capability states", async () => {
    authenticate({
      ...populatedSnapshot,
      ui_mode: {
        display: "headless",
        effective: "web_ui",
        gpui_available: false,
        requested: "auto",
      },
      recent_activity: [activityAt(1)],
    });
    const user = userEvent.setup();
    const { container } = renderApp();

    await screen.findByRole("heading", { name: "translated:page.overview" });
    await user.click(
      screen.getByRole("button", { name: "translated:nav.settings" }),
    );
    await screen.findByRole("heading", { name: "translated:page.settings" });

    const grid = container.querySelector<HTMLElement>(
      ".settings-content > .settings-grid:not(.advanced-settings-grid)",
    );
    expect(grid).toBeInTheDocument();
    const groups = Array.from(
      grid?.querySelectorAll<HTMLElement>("[data-settings-group]") ?? [],
    );
    expect(groups.map((group) => group.dataset.settingsGroup)).toEqual([
      "owner-security",
      "remote-access",
      "services-updates",
      "discovery-index",
      "data-recovery",
    ]);
    expect(
      groups.map((group) => group.querySelector("h3")?.textContent),
    ).toEqual([
      "translated:settings.section.owner_security",
      "translated:settings.section.remote_access",
      "translated:settings.section.services_updates",
      "translated:settings.section.discovery_index",
      "translated:settings.section.data_recovery",
    ]);

    expect(groups[0]).toHaveTextContent("Visual Fixture Node");
    expect(groups[0]).toHaveTextContent("node-visual-01");
    expect(groups[0]).toHaveTextContent(
      "translated:choice.permission.manage_networks",
    );
    expect(groups[1]).toHaveTextContent(
      "translated:settings.webui_configuration.detail",
    );
    expect(groups[2]).toHaveTextContent("translated:capability.file");
    expect(groups[2]).toHaveTextContent(
      "translated:status.capability.unsupported",
    );
    expect(groups[2]).toHaveTextContent(
      "translated:settings.platform_limitations.detail",
    );
    expect(groups[2]).not.toHaveTextContent(
      "translated:settings.platform_limitations.none",
    );
    expect(groups[3]).toHaveTextContent(
      "translated:settings.discovery_index.detail",
    );
    expect(groups[4]).toHaveTextContent("translated:settings.value.read_only");
    expect(groups[4]).toHaveTextContent("translated:settings.read_only.detail");
    expect(grid?.querySelector("button, input, select, textarea")).toBeNull();

    const advanced = container.querySelector<HTMLElement>(".advanced-settings");
    expect(advanced).toBeInTheDocument();
    expect(
      within(advanced as HTMLElement).getByRole("heading", {
        name: "translated:settings.advanced.title",
      }),
    ).toBeInTheDocument();
    expect(
      Array.from(
        advanced?.querySelectorAll<HTMLElement>(
          ".advanced-settings-grid > [data-settings-group]",
        ) ?? [],
      ).map((group) => group.dataset.settingsGroup),
    ).toEqual([
      "network-listeners",
      "dns-privacy",
      "transport-limits",
      "web-security",
      "observability-maintenance",
    ]);
    expect(
      advanced?.querySelectorAll("[data-advanced-setting-id]"),
    ).toHaveLength(21);
    expect(
      Array.from(
        advanced?.querySelectorAll<HTMLElement>("[data-setting-state]") ?? [],
      ).map((setting) => setting.dataset.settingState),
    ).toEqual(
      expect.arrayContaining([
        "managed",
        "read_only",
        "partial",
        "unsupported",
      ]),
    );
    expect(
      advanced?.querySelector('[data-advanced-setting-id="listen-ports"]'),
    ).toHaveTextContent("127.0.0.1:9000");
    expect(
      advanced?.querySelector("button, input, select, textarea"),
    ).toBeNull();
  });

  it("exposes the shared four-region shell with a persistent action panel", async () => {
    authenticate();
    const { container } = renderApp();

    await screen.findByRole("heading", { name: "translated:page.overview" });
    const regions = Array.from(
      container.querySelectorAll<HTMLElement>("[data-ui-region]"),
    ).map((element) => element.dataset.uiRegion);

    expect(regions).toEqual(["sidebar", "topbar", "content", "action-panel"]);
    const workspace = container.querySelector<HTMLElement>(".workspace");
    const content = workspace?.querySelector<HTMLElement>(
      '[data-ui-region="content"]',
    );
    const actionPanel = workspace?.querySelector<HTMLElement>(
      '[data-ui-region="action-panel"]',
    );
    expect(content).toBeInTheDocument();
    expect(actionPanel).toBeInTheDocument();
    expect(content?.nextElementSibling).toBe(actionPanel);
  });

  it("searches objects locally and navigates with network context", async () => {
    authenticate(populatedSnapshot);
    const user = userEvent.setup();
    renderApp();

    const search = await screen.findByRole("textbox", {
      name: "translated:common.search",
    });
    await user.type(search, "home");
    const results = screen.getByRole("list", {
      name: "translated:common.search_results",
    });
    expect(within(results).getAllByRole("listitem").length).toBeGreaterThan(1);

    await user.click(
      within(results).getByRole("button", {
        name: /translated:search\.kind\.network · home/,
      }),
    );
    expect(
      await screen.findByRole("heading", { name: "translated:page.networks" }),
    ).toBeInTheDocument();
    expect(
      screen.getByLabelText("translated:common.current_network"),
    ).toHaveValue("home");
    expect(search).toHaveValue("");

    await user.type(search, "peer-tokyo");
    await user.click(
      screen.getByRole("button", {
        name: /translated:search\.kind\.peer · peer-tokyo/,
      }),
    );
    expect(
      await screen.findByRole("heading", { name: "translated:page.peers" }),
    ).toBeInTheDocument();
    expect(
      screen.getByLabelText("translated:common.current_network"),
    ).toHaveValue("home");
  });

  it("supports global search keyboard focus, empty state, and clearing", async () => {
    authenticate(populatedSnapshot);
    const user = userEvent.setup();
    renderApp();

    const search = await screen.findByRole("textbox", {
      name: "translated:common.search",
    });
    await user.keyboard("{Control>}k{/Control}");
    expect(search).toHaveFocus();

    await user.type(search, "no-such-object");
    expect(screen.getByRole("status")).toHaveTextContent(
      "translated:common.search_no_results",
    );
    await user.click(
      screen.getByRole("button", { name: "translated:common.search_clear" }),
    );
    expect(search).toHaveValue("");
    expect(search).toHaveFocus();
    expect(
      screen.queryByRole("list", { name: "translated:common.search_results" }),
    ).toBeNull();
  });

  it("finds advanced settings and navigates to the settings page", async () => {
    authenticate(populatedSnapshot);
    const user = userEvent.setup();
    renderApp();

    const search = await screen.findByRole("textbox", {
      name: "translated:common.search",
    });
    await user.type(search, "doh");
    await user.click(
      screen.getByRole("button", {
        name: /translated:search\.kind\.setting · translated:settings\.row\.doh_upstreams/,
      }),
    );

    expect(
      await screen.findByRole("heading", { name: "translated:page.settings" }),
    ).toBeInTheDocument();
    expect(search).toHaveValue("");
  });

  it("uses the shared shell and action-panel tokens", () => {
    const styles = readFileSync(
      resolve(process.cwd(), "src/styles.css"),
      "utf8",
    );
    expect(styles).toContain(
      "grid-template-columns: var(--sidebar-w) minmax(0, 1fr)",
    );
    expect(styles).toContain(
      "grid-template-rows: var(--topbar-h) auto minmax(0, 1fr)",
    );
    expect(styles).toMatch(
      /\.workspace\s*\{[^}]*grid-template-columns: minmax\(0, 1fr\) var\(--action-panel-compact-w\)/s,
    );
    expect(styles).toMatch(
      /@media \(min-width: 1280px\)[\s\S]*?\.workspace\s*\{[^}]*var\(--action-panel-w\)/,
    );
    expect(styles).not.toContain("--sidebar-width");
    expect(styles).not.toContain("--topbar-height");
  });

  it.each([
    {
      status: "uninitialized",
      value: snapshot,
      action: "start_node",
      secondaryAction: null,
    },
    {
      status: "running",
      value: {
        ...snapshot,
        lifecycle: { ...snapshot.lifecycle, phase: "running" as const },
        permissions: {
          access_model: "trusted_control_plane",
          permissions: ["view_state", "manage_networks", "stop_node"],
        },
      },
      action: "stop_node",
      secondaryAction: "restart_node",
    },
    {
      status: "degraded",
      value: {
        ...snapshot,
        lifecycle: { ...snapshot.lifecycle, phase: "running" as const },
        last_error: {
          code: "unavailable" as const,
          category: "dependency" as const,
          message: "raw backend summary",
          message_key: "error.operation_unavailable",
          retryable: true,
        },
      },
      action: "get_node_diagnostics",
      secondaryAction: "restart_node",
    },
  ])(
    "navigates from overview to the executable runtime action for $status",
    async ({ value, action, secondaryAction }) => {
      authenticate(value as NodeSnapshot);
      const user = userEvent.setup();
      renderApp();

      await screen.findByRole("heading", { name: "translated:page.overview" });
      await user.click(
        screen.getByRole("button", {
          name: "translated:overview.manage_runtime",
        }),
      );

      expect(
        await screen.findByRole("heading", {
          name: "translated:page.node_runtime",
        }),
      ).toBeInTheDocument();
      const panel = await screen.findByRole("complementary", {
        name: "translated:common.shared_actions",
      });
      await waitFor(() => {
        expect(
          screen
            .getAllByRole("button", { name: `translated:action.${action}` })
            .some((button) =>
              button.classList.contains("runtime-primary-action"),
            ),
        ).toBe(true);
      });
      const primaryAction = screen
        .getAllByRole("button", { name: `translated:action.${action}` })
        .find((button) => button.classList.contains("runtime-primary-action"));
      if (!primaryAction) throw new Error("runtime primary action is missing");
      expect(primaryAction).toHaveClass("runtime-primary-action");
      const secondary = document.querySelector(".runtime-secondary-action");
      if (secondaryAction) {
        expect(secondary).toHaveTextContent(
          `translated:action.${secondaryAction}`,
        );
      } else {
        expect(secondary).not.toBeInTheDocument();
      }
      await user.click(primaryAction);

      expect(
        within(panel).getByRole("button", {
          name: `translated:action.${action}`,
        }),
      ).toHaveAttribute("aria-expanded", "true");
      expect(panel.querySelector(".action-form")).toBeInTheDocument();
    },
  );

  it("reuses the operation idempotency key after a terminal failure", async () => {
    authenticate({
      ...snapshot,
      lifecycle: { ...snapshot.lifecycle, phase: "running" },
      permissions: {
        access_model: "trusted_control_plane",
        permissions: ["view_state", "manage_networks", "stop_node"],
      },
    });
    const failedOperation = {
      operation_id: "op-restart-failed",
      state: "failed",
      stage: "cleanup",
      attempt: 1,
      max_attempts: 3,
      cancellable: false,
      cancel_blocked_reason: "operation.cancel.not_supported",
      error: {
        code: "node.restart_failed",
        message_key: "error.node_restart_failed",
        retryable: true,
        recovery: { kind: "retry", hint_key: "recovery.retry" },
      },
    } as NodeOperationSnapshot;
    vi.mocked(sendCommand).mockImplementation(
      async (request): Promise<NodeApiCommandResponse> => ({
        schema_version: 1,
        request_id: request.request.request_id,
        server_time: "2026-11-02T00:00:00Z",
        operation: failedOperation,
      }),
    );
    const user = userEvent.setup();
    renderApp();

    await screen.findByRole("heading", { name: "translated:page.overview" });
    await user.click(
      screen.getByRole("button", {
        name: "translated:overview.manage_runtime",
      }),
    );
    const secondary = await waitFor(() => {
      const button = document.querySelector<HTMLButtonElement>(
        ".runtime-secondary-action",
      );
      if (!button) throw new Error("runtime restart action is missing");
      return button;
    });
    expect(secondary).toHaveTextContent("translated:action.restart_node");
    await user.click(secondary);

    const panel = screen.getByRole("complementary", {
      name: "translated:common.shared_actions",
    });
    await user.click(
      within(panel).getByRole("button", { name: "translated:common.execute" }),
    );
    const dialog = await screen.findByRole("dialog");
    await user.click(
      within(dialog).getByRole("button", { name: "translated:common.execute" }),
    );
    await waitFor(() => {
      expect(panel.querySelector(".form-error")).toHaveTextContent(
        "translated:error.node_restart_failed",
      );
    });
    const operationStatus = await screen.findByTestId("operation-status");
    expect(operationStatus).toHaveTextContent("op-restart-failed");

    await user.click(
      within(operationStatus).getByRole("button", {
        name: "translated:operation.retry",
      }),
    );
    await waitFor(() => expect(sendCommand).toHaveBeenCalledTimes(2));

    const first = vi.mocked(sendCommand).mock.calls[0][0];
    const second = vi.mocked(sendCommand).mock.calls[1][0];
    expect(second.request.command).toEqual(first.request.command);
    expect(second.idempotency_key).toBe(first.idempotency_key);
  });

  it("reauthenticates a stale WebUI session and retries the original command", async () => {
    authenticate({
      ...snapshot,
      lifecycle: { ...snapshot.lifecycle, phase: "running" },
      permissions: {
        access_model: "trusted_control_plane",
        permissions: ["view_state", "manage_networks", "stop_node"],
      },
    });
    vi.mocked(sendCommand)
      .mockRejectedValueOnce(new ReauthenticationRequiredError())
      .mockImplementationOnce(async (request) => ({
        schema_version: 1,
        request_id: request.request.request_id,
        server_time: "2026-11-02T00:00:00Z",
        result: {
          request_id: request.request.request_id,
          outcome: {
            status: "completed",
            data: { message: "node restarted" },
          },
        },
      }));
    vi.mocked(reauthenticateWithPasskey).mockResolvedValue({
      authenticated: true,
    });
    const user = userEvent.setup();
    renderApp();

    await screen.findByRole("heading", { name: "translated:page.overview" });
    await user.click(
      screen.getByRole("button", {
        name: "translated:overview.manage_runtime",
      }),
    );
    const secondary = await waitFor(() => {
      const button = document.querySelector<HTMLButtonElement>(
        ".runtime-secondary-action",
      );
      if (!button) throw new Error("runtime restart action is missing");
      return button;
    });
    await user.click(secondary);

    const panel = screen.getByRole("complementary", {
      name: "translated:common.shared_actions",
    });
    await user.click(
      within(panel).getByRole("button", { name: "translated:common.execute" }),
    );
    let dialog = await screen.findByRole("dialog");
    await user.click(
      within(dialog).getByRole("button", { name: "translated:common.execute" }),
    );

    dialog = await screen.findByRole("dialog");
    expect(
      within(dialog).getByRole("heading", {
        name: "translated:owner_auth.reauth.title",
      }),
    ).toBeInTheDocument();
    await user.click(
      within(dialog).getByRole("button", {
        name: "translated:owner_auth.passkey.reauth_button",
      }),
    );

    await waitFor(() => expect(sendCommand).toHaveBeenCalledTimes(2));
    expect(reauthenticateWithPasskey).toHaveBeenCalledTimes(1);
    const first = vi.mocked(sendCommand).mock.calls[0][0];
    const second = vi.mocked(sendCommand).mock.calls[1][0];
    expect(second).toEqual(first);
    await waitFor(() =>
      expect(screen.queryByRole("dialog")).not.toBeInTheDocument(),
    );
  });

  it("cancels reauthentication without retrying or creating an operation", async () => {
    authenticate({
      ...snapshot,
      lifecycle: { ...snapshot.lifecycle, phase: "running" },
      permissions: {
        access_model: "trusted_control_plane",
        permissions: ["view_state", "manage_networks", "stop_node"],
      },
    });
    vi.mocked(sendCommand).mockRejectedValueOnce(
      new ReauthenticationRequiredError(),
    );
    const user = userEvent.setup();
    renderApp();

    await screen.findByRole("heading", { name: "translated:page.overview" });
    await user.click(
      screen.getByRole("button", {
        name: "translated:overview.manage_runtime",
      }),
    );
    const secondary = await waitFor(() => {
      const button = document.querySelector<HTMLButtonElement>(
        ".runtime-secondary-action",
      );
      if (!button) throw new Error("runtime restart action is missing");
      return button;
    });
    await user.click(secondary);

    const panel = screen.getByRole("complementary", {
      name: "translated:common.shared_actions",
    });
    await user.click(
      within(panel).getByRole("button", { name: "translated:common.execute" }),
    );
    let dialog = await screen.findByRole("dialog");
    await user.click(
      within(dialog).getByRole("button", { name: "translated:common.execute" }),
    );

    dialog = await screen.findByRole("dialog");
    await user.click(
      within(dialog).getByRole("button", { name: "translated:common.cancel" }),
    );

    await waitFor(() =>
      expect(panel.querySelector(".form-error")).toHaveTextContent(
        "translated:owner_auth.reauth.cancelled",
      ),
    );
    expect(sendCommand).toHaveBeenCalledTimes(1);
    expect(reauthenticateWithPasskey).not.toHaveBeenCalled();
    expect(screen.queryByTestId("operation-status")).not.toBeInTheDocument();
  });

  it("keeps runtime navigation visible but hides an unauthorized primary action", async () => {
    authenticate({
      ...snapshot,
      permissions: {
        access_model: "trusted_control_plane",
        permissions: ["view_state"],
      },
    });
    const user = userEvent.setup();
    renderApp();

    await screen.findByRole("heading", { name: "translated:page.overview" });
    const navigation = screen.getByRole("button", {
      name: "translated:overview.manage_runtime",
    });
    expect(navigation).toHaveClass("overview-primary-action");
    await user.click(navigation);

    await screen.findByRole("heading", {
      name: "translated:page.node_runtime",
    });
    expect(
      document.querySelector(".runtime-primary-action"),
    ).not.toBeInTheDocument();
  });

  it("does not offer a lifecycle action while the node is transitioning", async () => {
    authenticate({
      ...snapshot,
      lifecycle: { ...snapshot.lifecycle, phase: "starting" },
    });
    const user = userEvent.setup();
    renderApp();

    await screen.findByRole("heading", { name: "translated:page.overview" });
    await user.click(
      screen.getByRole("button", {
        name: "translated:overview.manage_runtime",
      }),
    );

    await screen.findByRole("heading", {
      name: "translated:page.node_runtime",
    });
    expect(
      document.querySelector(".runtime-primary-action"),
    ).not.toBeInTheDocument();
  });

  it("shows compact network paths without generic metric cards or live charts", async () => {
    if (!window.FuckIPV6I18n) throw new Error("i18n runtime is missing");
    window.FuckIPV6I18n.tr = (_locale, key, params) =>
      `translated:${key}:${JSON.stringify(params ?? {})}`;
    authenticate(populatedSnapshot);
    const { container } = renderApp();

    const networks = await screen.findByTestId("overview-networks");
    const home = within(networks)
      .getByText("home")
      .closest(".overview-network-row");
    expect(home).toHaveTextContent("translated:overview.network.detail");
    expect(home).toHaveTextContent("translated:overview.network.path");
    expect(home).toHaveTextContent(
      '"routing":"translated:choice.routing.bypass_mainland"',
    );
    expect(home).toHaveTextContent('"exit":"translated:overview.exit.selected');
    expect(
      container.querySelector(".overview-stack .metric-card"),
    ).not.toBeInTheDocument();
    expect(
      container.querySelector(".overview-stack [data-chart]"),
    ).not.toBeInTheDocument();
    expect(
      container.querySelector(".overview-stack canvas"),
    ).not.toBeInTheDocument();
  });

  it("shows at most three deduplicated operational issues", async () => {
    if (!window.FuckIPV6I18n) throw new Error("i18n runtime is missing");
    window.FuckIPV6I18n.tr = (_locale, key, params) =>
      `translated:${key}:${JSON.stringify(params ?? {})}`;
    authenticate({
      ...snapshot,
      lifecycle: {
        ...snapshot.lifecycle,
        failed_networks: ["home", "home", "office", "lab"],
      },
    });
    renderApp();

    const issues = await screen.findByTestId("overview-issues");
    expect(within(issues).getAllByRole("paragraph")).toHaveLength(3);
    expect(issues).toHaveTextContent('"network":"home"');
    expect(issues).toHaveTextContent('"network":"office"');
    expect(issues).toHaveTextContent('"network":"lab"');
  });

  it("sorts, truncates, and sanitizes recent overview activity", async () => {
    authenticate({
      ...snapshot,
      recent_activity: [3, 6, 1, 5, 2, 4].map(activityAt),
    });
    renderApp();

    const activity = await screen.findByTestId("overview-activity");
    const rows = activity.querySelectorAll(".overview-activity-row");
    expect(rows).toHaveLength(5);
    expect(rows[0]).toHaveTextContent("command-6");
    expect(rows[4]).toHaveTextContent("command-2");
    expect(activity).not.toHaveTextContent("raw-summary");
    expect(activity).not.toHaveTextContent("secret-");
  });

  it.each(["create_network", "join_network"])(
    "navigates from the %s shortcut to its real network action",
    async (action) => {
      authenticate();
      const user = userEvent.setup();
      renderApp();

      await user.click(
        await screen.findByRole("button", {
          name: `translated:action.${action}`,
        }),
      );

      expect(
        await screen.findByRole("heading", {
          name: "translated:page.networks",
        }),
      ).toBeInTheDocument();
      expect(
        screen.getByRole("tab", { name: "translated:nav.networks" }),
      ).toHaveAttribute("aria-selected", "true");
      const panel = screen.getByRole("complementary", {
        name: "translated:common.shared_actions",
      });
      expect(
        within(panel).getByRole("button", {
          name: `translated:action.${action}`,
        }),
      ).toHaveAttribute("aria-expanded", "true");
    },
  );

  it("searches discovery locally and separates joined networks from results", async () => {
    authenticate(
      discoverySnapshot([
        discoveryRecord("family", {
          tags: ["smart-home"],
          joinMode: "approval_required",
        }),
        discoveryRecord("studio", {
          language: "zh-CN",
          region: "CN",
          joinMode: "invite_only",
        }),
      ]),
    );
    const user = userEvent.setup();
    renderApp();

    await user.click(
      await screen.findByRole("button", {
        name: /translated:shortcut\.discover_networks/,
      }),
    );

    const joined = await screen.findByTestId("joined-networks-section");
    const results = screen.getByTestId("discovery-results-section");
    expect(joined).toHaveTextContent("translated:discovery.empty.joined");
    expect(
      within(results).getByText("translated:discovery.search.private_note"),
    ).toBeInTheDocument();
    const panel = screen.getByRole("complementary", {
      name: "translated:common.shared_actions",
    });
    expect(
      within(panel).getByRole("button", {
        name: "translated:action.refresh_discovery_index",
      }),
    ).toHaveAttribute("aria-expanded", "true");
    expect(within(results).getByText("family")).toBeInTheDocument();
    expect(within(results).getByText("studio")).toBeInTheDocument();

    const search = within(results).getByRole("searchbox", {
      name: "translated:discovery.search.label",
    });
    await user.type(search, "studio");
    await waitFor(() =>
      expect(within(results).queryByText("family")).not.toBeInTheDocument(),
    );
    expect(within(results).getByText("studio")).toBeInTheDocument();

    await user.clear(search);
    await waitFor(() =>
      expect(within(results).getByText("family")).toBeInTheDocument(),
    );
    await user.selectOptions(
      within(results).getByLabelText("translated:discovery.filter.language"),
      "zh-CN",
    );
    expect(within(results).queryByText("family")).not.toBeInTheDocument();
    expect(within(results).getByText("studio")).toBeInTheDocument();

    await user.clear(search);
    await user.type(search, "does-not-exist");
    await waitFor(() =>
      expect(screen.getByTestId("discovery-state")).toHaveTextContent(
        "translated:discovery.state.no_matches",
      ),
    );
    expect(sendCommand).not.toHaveBeenCalled();
  });

  it("opens the signed join preview with the stable discovery network id", async () => {
    authenticate(discoverySnapshot([discoveryRecord("family")]));
    const user = userEvent.setup();
    renderApp();

    await user.click(
      await screen.findByRole("button", {
        name: /translated:shortcut\.discover_networks/,
      }),
    );
    const result = await screen.findByTestId("discovery-result");
    await user.click(
      within(result).getByText("translated:discovery.result.details"),
    );
    expect(result).toHaveTextContent("network-family");
    expect(result).toHaveTextContent("translated:discovery.result.privacy_summary");

    await user.click(
      within(result).getByRole("button", {
        name: "translated:discovery.result.open_join",
      }),
    );
    const panel = screen.getByRole("complementary", {
      name: "translated:common.shared_actions",
    });
    expect(
      within(panel).getByRole("button", {
        name: "translated:action.preview_join_network",
      }),
    ).toHaveAttribute("aria-expanded", "true");
    expect(
      within(panel).getByLabelText("translated:field.network_id"),
    ).toHaveValue("network-family");
    expect(
      within(panel).queryByLabelText("translated:field.ticket_or_url"),
    ).not.toBeInTheDocument();
    expect(sendCommand).not.toHaveBeenCalled();
  });

  it("leaves the selected network without touching other memberships", async () => {
    authenticate(populatedSnapshot);
    const remainingNetwork = populatedSnapshot.networks.find(
      (network) => network.name === "studio",
    )!;
    const leftSnapshot: NodeSnapshot = {
      ...populatedSnapshot,
      revision: populatedSnapshot.revision + 1,
      networks: [remainingNetwork],
      lifecycle: {
        ...populatedSnapshot.lifecycle,
        active_networks: 1,
        configured_networks: 1,
      },
    };
    vi.mocked(sendCommand).mockImplementation(
      async (request): Promise<NodeApiCommandResponse> => ({
        schema_version: 1,
        request_id: request.request.request_id,
        server_time: "2026-11-04T00:00:00Z",
        result: {
          request_id: request.request.request_id,
          outcome: {
            status: "network_left",
            data: {
              left: true,
              network_name: "home",
              snapshot: leftSnapshot,
            },
          },
        },
      }),
    );
    const user = userEvent.setup();
    renderApp();

    await user.click(
      await screen.findByRole("button", {
        name: /translated:shortcut\.discover_networks/,
      }),
    );
    const joined = await screen.findByTestId("joined-networks-section");
    const homeCard = within(joined).getByText("home").closest("article");
    expect(homeCard).not.toBeNull();
    fireEvent.contextMenu(homeCard!);
    await user.click(
      await screen.findByRole("menuitem", {
        name: "translated:action.leave_network",
      }),
    );

    const panel = screen.getByRole("complementary", {
      name: "translated:common.shared_actions",
    });
    const leaveAction = within(panel)
      .getByRole("button", {
        name: "translated:action.leave_network",
      })
      .closest("article");
    expect(leaveAction).not.toBeNull();
    expect(
      within(leaveAction!).getByRole("button", {
        name: "translated:action.leave_network",
      }),
    ).toHaveAttribute("aria-expanded", "true");
    expect(
      within(leaveAction!).getByLabelText("translated:field.network_name"),
    ).toHaveValue("home");

    await user.click(
      within(leaveAction!).getByRole("button", {
        name: "translated:common.execute",
      }),
    );
    const dialog = await screen.findByRole("dialog");
    await user.click(
      within(dialog).getByRole("button", {
        name: "translated:common.execute",
      }),
    );

    expect(
      await within(panel).findByText("translated:outcome.network_left"),
    ).toBeInTheDocument();
    expect(sendCommand).toHaveBeenCalledTimes(1);
    expect(vi.mocked(sendCommand).mock.calls[0][0].request.command).toEqual({
      command: "leave_network",
      payload: { network_name: "home" },
    });
    await waitFor(() =>
      expect(
        within(screen.getByTestId("joined-networks-section")).queryByText("home"),
      ).not.toBeInTheDocument(),
    );
    expect(
      within(screen.getByTestId("joined-networks-section")).getByText("studio"),
    ).toBeInTheDocument();
  });

  it("opens the joined-network menu from its visible keyboard trigger and restores focus", async () => {
    authenticate(populatedSnapshot);
    const user = userEvent.setup();
    renderApp();

    await user.click(
      await screen.findByRole("button", {
        name: /translated:shortcut\.discover_networks/,
      }),
    );
    const joined = await screen.findByTestId("joined-networks-section");
    const trigger = within(joined).getByRole("button", {
      name: "translated:common.actions · home",
    });

    trigger.focus();
    await user.keyboard("{Enter}");
    expect(
      await screen.findByRole("menuitem", {
        name: "translated:action.leave_network",
      }),
    ).toBeInTheDocument();
    await user.keyboard("{Escape}");
    await waitFor(() => expect(trigger).toHaveFocus());
    expect(
      screen.queryByRole("menuitem", {
        name: "translated:action.leave_network",
      }),
    ).not.toBeInTheDocument();

    await user.keyboard(" ");
    expect(
      await screen.findByRole("menuitem", {
        name: "translated:action.leave_network",
      }),
    ).toBeInTheDocument();
    await user.keyboard("{Escape}");
    await waitFor(() => expect(trigger).toHaveFocus());
  });

  it("opens the joined-network action sheet after a touch long press", async () => {
    authenticate(populatedSnapshot);
    renderApp();

    await screen.findByRole("button", {
      name: /translated:shortcut\.discover_networks/,
    }).then((button) => button.click());
    const joined = await screen.findByTestId("joined-networks-section");
    const homeCard = within(joined).getByText("home").closest("article");
    expect(homeCard).not.toBeNull();

    vi.useFakeTimers();
    try {
      fireEvent.pointerDown(homeCard!, {
        pointerType: "touch",
        clientX: 12,
        clientY: 18,
      });
      act(() => vi.advanceTimersByTime(449));
      expect(
        screen.queryByRole("menuitem", {
          name: "translated:action.leave_network",
        }),
      ).not.toBeInTheDocument();
      act(() => vi.advanceTimersByTime(1));
      expect(
        screen.getByRole("menuitem", {
          name: "translated:action.leave_network",
        }),
      ).toBeInTheDocument();
      expect(window.history.state.networkActionSheet).toBe("home");
    } finally {
      window.history.replaceState(null, "");
      vi.useRealTimers();
    }
  });

  it("cancels a touch long press on scroll or pointer movement", async () => {
    authenticate(populatedSnapshot);
    renderApp();

    await screen.findByRole("button", {
      name: /translated:shortcut\.discover_networks/,
    }).then((button) => button.click());
    const joined = await screen.findByTestId("joined-networks-section");
    const homeCard = within(joined).getByText("home").closest("article");
    expect(homeCard).not.toBeNull();

    vi.useFakeTimers();
    try {
      fireEvent.pointerDown(homeCard!, {
        pointerType: "touch",
        clientX: 12,
        clientY: 18,
      });
      fireEvent.scroll(window);
      act(() => vi.advanceTimersByTime(450));

      fireEvent.pointerDown(homeCard!, {
        pointerType: "touch",
        clientX: 12,
        clientY: 18,
      });
      fireEvent.pointerMove(homeCard!, {
        pointerType: "touch",
        clientX: 24,
        clientY: 18,
      });
      act(() => vi.advanceTimersByTime(450));

      expect(
        screen.queryByRole("menuitem", {
          name: "translated:action.leave_network",
        }),
      ).not.toBeInTheDocument();
    } finally {
      vi.useRealTimers();
    }
  });

  it("closes a long-press action sheet on browser back and restores focus", async () => {
    authenticate(populatedSnapshot);
    renderApp();

    await screen.findByRole("button", {
      name: /translated:shortcut\.discover_networks/,
    }).then((button) => button.click());
    const joined = await screen.findByTestId("joined-networks-section");
    const homeCard = within(joined).getByText("home").closest("article");
    const trigger = within(joined).getByRole("button", {
      name: "translated:common.actions · home",
    });
    expect(homeCard).not.toBeNull();

    fireEvent.pointerDown(homeCard!, {
      pointerType: "touch",
      clientX: 12,
      clientY: 18,
    });
    expect(
      await screen.findByRole("menuitem", {
        name: "translated:action.leave_network",
      }),
    ).toBeInTheDocument();
    expect(window.history.state.networkActionSheet).toBe("home");

    act(() => window.history.back());
    await waitFor(() =>
      expect(
        screen.queryByRole("menuitem", {
          name: "translated:action.leave_network",
        }),
      ).not.toBeInTheDocument(),
    );
    await waitFor(() => expect(trigger).toHaveFocus());
  });

  it("shows degraded partial and stale discovery without calling it empty", async () => {
    authenticate(
      discoverySnapshot(
        [
          discoveryRecord("cached", {
            acceptedAtUnixMs: Date.now() - 11 * 60 * 1000,
          }),
        ],
        { rejected_record_count: 2 },
      ),
    );
    renderApp();

    await screen.findByRole("button", {
      name: /translated:shortcut\.discover_networks/,
    }).then((button) => button.click());

    expect(await screen.findByTestId("discovery-state")).toHaveTextContent(
      "translated:discovery.state.degraded",
    );
    expect(screen.getByTestId("discovery-partial")).toHaveTextContent(
      "translated:discovery.state.rejected",
    );
    expect(screen.getByTestId("discovery-stale")).toHaveTextContent(
      "translated:discovery.state.stale",
    );
    expect(screen.getByTestId("discovery-result")).toHaveTextContent("cached");
    expect(screen.getByTestId("discovery-results-section")).not.toHaveTextContent(
      "translated:discovery.state.empty",
    );
  });

  it("distinguishes discovery failure from an empty result and exposes retry", async () => {
    authenticate(
      discoverySnapshot([], {
        state: "failed",
        last_success_unix_ms: null,
        last_error: { category: "timeout", retryable: true },
      }),
    );
    const user = userEvent.setup();
    renderApp();

    await user.click(
      await screen.findByRole("button", {
        name: /translated:shortcut\.discover_networks/,
      }),
    );
    const state = await screen.findByTestId("discovery-state");
    expect(state).toHaveTextContent("translated:discovery.state.failed");
    expect(state).toHaveTextContent("translated:discovery.error.detail");
    expect(state).toHaveAttribute("role", "alert");

    await user.click(
      screen.getByRole("button", {
        name: "translated:discovery.action.retry",
      }),
    );
    await waitFor(() => expect(sendCommand).toHaveBeenCalled());
    expect(
      vi.mocked(sendCommand).mock.calls.at(-1)![0].request.command,
    ).toEqual({ command: "retry_discovery_index" });
  });
});
