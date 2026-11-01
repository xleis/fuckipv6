import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import manifestFixture from "../../contracts/typescript/fixtures/node-manifest.json";
import productFixture from "../../contracts/typescript/fixtures/product-visual-snapshot.json";
import type {
  ActivityEntry,
  NodeSnapshot,
  NodeUiManifest,
} from "../../contracts/typescript/node-contract-v1";
import { App } from "./App";
import { loadManifest, loadSnapshot } from "./api/client";
import { loadLanguagePreference, loadOwnerSession } from "./api/session";
import { installTestI18nRuntime } from "./test/runtime";

vi.mock("./api/client", async (importOriginal) => {
  const original = await importOriginal<typeof import("./api/client")>();
  return {
    ...original,
    loadManifest: vi.fn(),
    loadSnapshot: vi.fn(),
    sendCommand: vi.fn(),
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
  control_health: "available",
  identity: null,
  last_error: null,
  capabilities: { capabilities: [] },
};

class SilentWebSocket {
  static readonly CONNECTING = 0;
  static readonly OPEN = 1;
  static readonly CLOSING = 2;
  static readonly CLOSED = 3;
  readonly readyState = SilentWebSocket.CONNECTING;
  addEventListener(): void {}
  close(): void {}
}

function renderApp() {
  const queryClient = new QueryClient({
    defaultOptions: {
      queries: { retry: false },
      mutations: { retry: false },
    },
  });
  return render(
    <QueryClientProvider client={queryClient}>
      <App />
    </QueryClientProvider>,
  );
}

function authenticate(snapshotValue: NodeSnapshot = snapshot): void {
  vi.mocked(loadOwnerSession).mockResolvedValue({ authenticated: true });
  vi.mocked(loadLanguagePreference).mockResolvedValue("zh-CN");
  vi.mocked(loadManifest).mockResolvedValue(manifest);
  vi.mocked(loadSnapshot).mockResolvedValue(snapshotValue);
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

async function expectModuleTabs(module: string, tabs: string[]): Promise<void> {
  const user = userEvent.setup();
  await user.click(screen.getByRole("button", { name: `translated:nav.${module}` }));
  const tablist = screen.getByRole("tablist", { name: "translated:common.page_sections" });
  expect(within(tablist).getAllByRole("tab").map((tab) => tab.textContent)).toEqual(
    tabs.map((tab) => `translated:nav.${tab}`),
  );
}

beforeEach(() => {
  vi.clearAllMocks();
  installTestI18nRuntime();
  vi.stubGlobal("WebSocket", SilentWebSocket);
});

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

describe("React WebUI shell", () => {
  it("shows the owner Passkey login surface before authentication", async () => {
    vi.mocked(loadOwnerSession).mockResolvedValue({ authenticated: false });
    renderApp();

    expect(await screen.findByRole("heading", { name: "translated:owner_auth.login.title" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "translated:owner_auth.passkey.login_button" })).toBeDisabled();
    expect(loadManifest).not.toHaveBeenCalled();
    expect(loadSnapshot).not.toHaveBeenCalled();
  });

  it("renders five modules and all eleven manifest-backed secondary pages", async () => {
    authenticate();
    renderApp();

    expect(await screen.findByRole("heading", { name: "translated:page.overview" })).toBeInTheDocument();
    expect(screen.getAllByRole("button", { name: /^translated:nav\.(overview|nodes|networks|activity|settings)$/ })).toHaveLength(5);

    await expectModuleTabs("overview", ["overview"]);
    await expectModuleTabs("nodes", ["peers", "routing", "identity"]);
    await expectModuleTabs("networks", ["networks", "topology", "exit_node", "relay"]);
    await expectModuleTabs("activity", ["traffic", "logs"]);
    await expectModuleTabs("settings", ["settings"]);
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
    expect(groups.map((group) => group.querySelector("h3")?.textContent)).toEqual([
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
    expect(groups[1]).toHaveTextContent("translated:settings.webui_configuration.detail");
    expect(groups[2]).toHaveTextContent("translated:capability.file");
    expect(groups[2]).toHaveTextContent("translated:status.capability.unsupported");
    expect(groups[2]).toHaveTextContent("translated:settings.platform_limitations.detail");
    expect(groups[2]).not.toHaveTextContent("translated:settings.platform_limitations.none");
    expect(groups[3]).toHaveTextContent("translated:settings.discovery_index.detail");
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
    ).toEqual(expect.arrayContaining(["managed", "read_only", "partial", "unsupported"]));
    expect(
      advanced?.querySelector('[data-advanced-setting-id="listen-ports"]'),
    ).toHaveTextContent("127.0.0.1:9000");
    expect(advanced?.querySelector("button, input, select, textarea")).toBeNull();
  });

  it("exposes the shared four-region shell with a persistent action panel", async () => {
    authenticate();
    const { container } = renderApp();

    await screen.findByRole("heading", { name: "translated:page.overview" });
    const regions = Array.from(container.querySelectorAll<HTMLElement>("[data-ui-region]"))
      .map((element) => element.dataset.uiRegion);

    expect(regions).toEqual(["sidebar", "topbar", "content", "action-panel"]);
    const workspace = container.querySelector<HTMLElement>(".workspace");
    const content = workspace?.querySelector<HTMLElement>('[data-ui-region="content"]');
    const actionPanel = workspace?.querySelector<HTMLElement>('[data-ui-region="action-panel"]');
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
    expect(screen.getByLabelText("translated:common.current_network")).toHaveValue(
      "home",
    );
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
    expect(screen.getByLabelText("translated:common.current_network")).toHaveValue(
      "home",
    );
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
    expect(screen.queryByRole("list", { name: "translated:common.search_results" })).toBeNull();
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
    const styles = readFileSync(resolve(process.cwd(), "src/styles.css"), "utf8");
    expect(styles).toContain("grid-template-columns: var(--sidebar-w) minmax(0, 1fr)");
    expect(styles).toContain("grid-template-rows: var(--topbar-h) auto minmax(0, 1fr)");
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
    },
  ])(
    "opens the real primary action for $status",
    async ({ value, action }) => {
      authenticate(value as NodeSnapshot);
      const user = userEvent.setup();
      renderApp();

      const panel = await screen.findByRole("complementary", {
        name: "translated:common.shared_actions",
      });
      const primaryAction = screen
        .getAllByRole("button", { name: `translated:action.${action}` })
        .find((button) => button.classList.contains("overview-primary-action"));
      if (!primaryAction) throw new Error("overview primary action is missing");
      await user.click(primaryAction);

      expect(
        within(panel).getByRole("button", {
          name: `translated:action.${action}`,
        }),
      ).toHaveAttribute("aria-expanded", "true");
      expect(panel.querySelector(".action-form")).toBeInTheDocument();
    },
  );

  it("hides a primary action that the owner cannot execute", async () => {
    authenticate({
      ...snapshot,
      permissions: {
        access_model: "trusted_control_plane",
        permissions: ["view_state"],
      },
    });
    renderApp();

    await screen.findByRole("heading", { name: "translated:page.overview" });
    expect(
      document.querySelector(".overview-primary-action"),
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
    expect(home).toHaveTextContent('"routing":"translated:choice.routing.bypass_mainland"');
    expect(home).toHaveTextContent('"exit":"translated:overview.exit.selected');
    expect(container.querySelector(".overview-stack .metric-card")).not.toBeInTheDocument();
    expect(container.querySelector(".overview-stack [data-chart]")).not.toBeInTheDocument();
    expect(container.querySelector(".overview-stack canvas")).not.toBeInTheDocument();
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
        await screen.findByRole("heading", { name: "translated:page.networks" }),
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

  it("explains why network discovery is unavailable", async () => {
    authenticate();
    renderApp();

    const discovery = await screen.findByRole("button", {
      name: /translated:shortcut\.discover_networks/,
    });
    expect(discovery).toBeDisabled();
    expect(discovery).toHaveAttribute(
      "title",
      "translated:shortcut.discovery_unavailable",
    );
    expect(discovery).toHaveTextContent(
      "translated:shortcut.discovery_unavailable",
    );
  });
});
