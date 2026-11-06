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
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import manifestFixture from "../../../contracts/typescript/fixtures/node-manifest.json";
import productFixture from "../../../contracts/typescript/fixtures/product-visual-snapshot.json";
import type {
  NodeSnapshot,
  NodeUiManifest,
  NodeUiPage,
  TopologyNode,
  TopologyNodeHealth,
  TopologySnapshot,
} from "../../../contracts/typescript/node-contract-v1";
import { installTestI18nRuntime } from "../test/runtime";
import { PageContent } from "./page-content";
import { TopologyView } from "./topology-view";

const translated = (key: string) => `translated:${key}`;

function membership(
  networkId: string,
  role: "owner" | "administrator" | "member",
  health: TopologyNodeHealth,
) {
  return {
    network_id: networkId,
    role,
    status: "active" as const,
    health,
    presence: {
      source: "live_session" as const,
      state: "online" as const,
      observed_at_unix_ms: 1_700_000_000_000,
    },
  };
}

function node(
  nodeId: string,
  displayName: string,
  health: TopologyNodeHealth,
  networks: Array<{ networkId: string; role: "owner" | "administrator" | "member" }>,
  capabilities: TopologyNode["capabilities"] = [],
  isSelf = false,
): TopologyNode {
  return {
    node_id: nodeId,
    display_name: displayName,
    health,
    is_self: isSelf,
    capabilities,
    networks: networks.map(({ networkId, role }) =>
      membership(networkId, role, health),
    ),
  };
}

function topologySnapshot(): TopologySnapshot {
  return {
    revision: 7,
    viewer: "node-self",
    generated_at_unix_ms: 1_700_000_000_000,
    truncated: false,
    boundaries: [
      {
        network_id: "net_alpha",
        name: "Alpha",
        member_count: 3,
        visible_member_count: 3,
        viewer_role: "owner",
      },
      {
        network_id: "net_beta",
        name: "Beta",
        member_count: 6,
        visible_member_count: 1,
        viewer_role: "member",
      },
    ],
    clusters: [
      {
        network_id: "net_beta",
        hidden_member_count: 5,
        expandable: false,
        sample: ["node-hidden-a", "node-hidden-b"],
      },
    ],
    nodes: [
      node(
        "node-self",
        "Self Node",
        "healthy",
        [{ networkId: "net_alpha", role: "owner" }],
        ["coordinator", "relay"],
        true,
      ),
      node(
        "node-shared",
        "Shared Node",
        "degraded",
        [
          { networkId: "net_alpha", role: "member" },
          { networkId: "net_beta", role: "administrator" },
        ],
        ["coordinator", "relay", "exit", "file"],
      ),
      node(
        "node-unreachable",
        "Unreachable Node",
        "unreachable",
        [{ networkId: "net_beta", role: "member" }],
        ["exit"],
      ),
      node(
        "node-unknown",
        "Unknown Node",
        "unknown",
        [{ networkId: "net_alpha", role: "member" }],
      ),
    ],
    connections: [
      {
        connection_id: "connection-active",
        from: "node-self",
        to: "node-shared",
        networks: ["net_alpha"],
        state: "active",
      },
      {
        connection_id: "connection-idle",
        from: "node-shared",
        to: "node-unreachable",
        networks: ["net_beta"],
        state: "idle",
      },
      {
        connection_id: "connection-stale",
        from: "node-self",
        to: "node-unknown",
        networks: ["net_alpha"],
        state: "stale",
      },
    ],
  };
}

function largeTopologySnapshot(nodeCount = 1_000): TopologySnapshot {
  return {
    revision: 100,
    viewer: "node-0000",
    generated_at_unix_ms: 1_700_000_000_000,
    truncated: false,
    boundaries: [
      {
        network_id: "capacity",
        name: "Capacity",
        member_count: nodeCount,
        visible_member_count: nodeCount,
        viewer_role: "owner",
      },
    ],
    clusters: [],
    nodes: Array.from({ length: nodeCount }, (_, index) => {
      const nodeId = `node-${String(index).padStart(4, "0")}`;
      return node(
        nodeId,
        `Node ${index}`,
        index % 11 === 0 ? "degraded" : "healthy",
        [{ networkId: "capacity", role: index === 0 ? "owner" : "member" }],
        index % 3 === 0 ? ["relay"] : [],
        index === 0,
      );
    }),
    connections: Array.from({ length: 5_000 }, (_, index) => ({
      connection_id: `capacity-connection-${index}`,
      from: `node-${String(index % nodeCount).padStart(4, "0")}`,
      to: `node-${String((index + 1) % nodeCount).padStart(4, "0")}`,
      networks: ["capacity"],
      state: (["active", "idle", "stale"] as const)[index % 3],
    })),
  };
}

beforeEach(() => {
  installTestI18nRuntime();
  Object.defineProperty(Element.prototype, "scrollIntoView", {
    configurable: true,
    value: vi.fn(),
  });
  vi.spyOn(Element.prototype, "getBoundingClientRect").mockReturnValue({
    x: 0,
    y: 0,
    top: 0,
    left: 0,
    right: 120,
    bottom: 120,
    width: 120,
    height: 120,
    toJSON: () => ({}),
  });
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

describe("TopologyView visual encodings", () => {
  it("draws each network with a distinct dashed boundary and visible name", () => {
    const { container } = render(
      <TopologyView topology={topologySnapshot()} locale="zh-CN" />,
    );
    const boundaries = [
      ...container.querySelectorAll<SVGEllipseElement>(".topology-boundary ellipse"),
    ];
    const labels = [
      ...container.querySelectorAll<SVGTextElement>(".topology-boundary-label"),
    ];

    expect(boundaries).toHaveLength(2);
    expect(new Set(boundaries.map((boundary) => boundary.getAttribute("stroke"))).size).toBe(2);
    expect(new Set(boundaries.map((boundary) => boundary.getAttribute("stroke-dasharray"))).size).toBe(2);
    expect(boundaries.every((boundary) => boundary.getAttribute("stroke-dasharray"))).toBe(true);
    expect(labels.map((label) => label.textContent)).toEqual(["Alpha", "Beta"]);
  });

  it("keeps network, capability and health encodings in separate visual channels", () => {
    const { container } = render(
      <TopologyView topology={topologySnapshot()} locale="zh-CN" />,
    );
    const shared = container.querySelector('[data-node-id="node-shared"]') as SVGGElement;
    const networkStroke = shared
      .querySelector(".topology-node-halo")
      ?.getAttribute("stroke");
    const capabilityStrokes = [
      ...shared.querySelectorAll<SVGPathElement>(".capability-enabled"),
    ].map((segment) => segment.getAttribute("stroke"));

    expect(capabilityStrokes).toHaveLength(4);
    expect(new Set(capabilityStrokes).size).toBe(4);
    expect(capabilityStrokes).not.toContain(networkStroke);
    expect(shared.querySelector(".topology-node-core")).toHaveClass("warn");
  });

  it("encodes all four capability segments and each health state by shape and glyph", () => {
    const { container } = render(
      <TopologyView topology={topologySnapshot()} locale="zh-CN" />,
    );
    const shared = container.querySelector('[data-node-id="node-shared"]') as SVGGElement;
    expect(shared.querySelectorAll(".capability-enabled")).toHaveLength(4);
    expect(shared.querySelectorAll(".capability-disabled")).toHaveLength(0);

    const expected = {
      "node-self": { glyph: "✓", shape: "circle", tone: "good" },
      "node-shared": { glyph: "!", shape: "path", tone: "warn" },
      "node-unreachable": { glyph: "×", shape: "rect", tone: "bad" },
      "node-unknown": { glyph: "?", shape: "path", tone: "unknown" },
    } as const;
    for (const [nodeId, encoding] of Object.entries(expected)) {
      const badge = container.querySelector(
        `[data-node-id="${nodeId}"] .topology-health-badge`,
      ) as SVGGElement;
      expect(badge.firstElementChild?.tagName.toLowerCase()).toBe(encoding.shape);
      expect(badge.querySelector("text")?.textContent).toBe(encoding.glyph);
      expect(badge).toHaveClass(encoding.tone);
    }
  });

  it("draws a shared node once and exposes all memberships and overlap counts", () => {
    const { container } = render(
      <TopologyView topology={topologySnapshot()} locale="zh-CN" />,
    );
    const sharedNodes = container.querySelectorAll('[data-node-id="node-shared"]');
    const shared = sharedNodes[0] as SVGGElement;

    expect(sharedNodes).toHaveLength(1);
    expect(shared.querySelector(".topology-node-shared")?.textContent).toContain("×2");
    expect(shared.getAttribute("aria-label")).toContain("Alpha");
    expect(shared.getAttribute("aria-label")).toContain("Beta");
    expect(shared.getAttribute("aria-label")).toContain(
      translated("join_request.role.administrator"),
    );
    expect(container.querySelector(".topology-cluster")?.textContent).toContain("+5");
  });

  it("switches to an equivalent list with every node, network and connection state", async () => {
    const user = userEvent.setup();
    const { container } = render(
      <TopologyView topology={topologySnapshot()} locale="zh-CN" />,
    );

    await user.click(
      screen.getByRole("button", { name: translated("topology.view.list") }),
    );
    const table = screen.getByRole("table");
    expect(container.querySelector(".topology-canvas")).not.toBeInTheDocument();
    expect(within(table).getAllByRole("row")).toHaveLength(5);

    const shared = table.querySelector('tr[data-node-id="node-shared"]') as HTMLElement;
    expect(shared).toHaveTextContent("Shared Node");
    expect(shared).toHaveTextContent("Alpha");
    expect(shared).toHaveTextContent("Beta");
    expect(shared).toHaveTextContent(translated("join_request.role.administrator"));
    expect(shared).toHaveTextContent(translated("capability.coordinator"));
    expect(shared).toHaveTextContent(translated("capability.relay"));
    expect(shared).toHaveTextContent(translated("capability.exit"));
    expect(shared).toHaveTextContent(translated("capability.file"));
    expect(shared).toHaveTextContent(translated("topology.health.degraded"));
    for (const state of ["active", "idle", "stale"] as const) {
      expect(shared).toHaveTextContent(translated(`topology.connection.${state}`));
    }
    expect(shared.querySelector(".topology-list-connections")?.textContent).toContain("1");

    await user.click(
      screen.getByRole("button", { name: translated("topology.view.graph") }),
    );
    expect(container.querySelectorAll("[data-node-id]")).toHaveLength(4);
  });

  it("collapses the legend by default and keeps it aligned with the canvas encodings", async () => {
    const user = userEvent.setup();
    const { container } = render(
      <TopologyView topology={topologySnapshot()} locale="zh-CN" />,
    );
    const summary = screen.getByText(translated("topology.legend.title"));
    const legend = summary.closest("details") as HTMLDetailsElement;
    expect(legend.open).toBe(false);

    await user.click(summary);
    expect(legend.open).toBe(true);
    const networkLines = [
      ...legend.querySelectorAll<SVGLineElement>("section:first-of-type line"),
    ];
    const boundaryStrokes = [
      ...container.querySelectorAll<SVGEllipseElement>(".topology-boundary ellipse"),
    ].map((boundary) => boundary.getAttribute("stroke"));
    expect(networkLines.map((line) => line.getAttribute("stroke"))).toEqual(
      boundaryStrokes,
    );

    for (const state of ["active", "idle", "stale"] as const) {
      const canvasLine = container.querySelector(
        `.topology-connection.${state} line`,
      );
      const legendLine = legend.querySelector(
        `section:nth-of-type(4) li:nth-child(${state === "active" ? 1 : state === "idle" ? 2 : 3}) line`,
      );
      expect(legendLine?.getAttribute("stroke-dasharray")).toBe(
        canvasLine?.getAttribute("stroke-dasharray"),
      );
    }
    expect(legend).toHaveTextContent(translated("topology.legend.overlap"));
  });

  it("changes the colour palette and applies an explicit high-contrast mode", async () => {
    const user = userEvent.setup();
    const { container } = render(
      <TopologyView topology={topologySnapshot()} locale="zh-CN" />,
    );
    const root = container.querySelector(".topology-view") as HTMLElement;
    const capabilityBefore = container
      .querySelector('[data-node-id="node-self"] .capability-enabled')
      ?.getAttribute("stroke");

    expect(root).toHaveAttribute("data-color-mode", "default");
    await user.click(
      screen.getByRole("button", { name: translated("topology.mode.colorblind") }),
    );
    expect(root).toHaveAttribute("data-color-mode", "colorblind");
    expect(
      container
        .querySelector('[data-node-id="node-self"] .capability-enabled')
        ?.getAttribute("stroke"),
    ).not.toBe(capabilityBefore);

    await user.click(
      screen.getByRole("checkbox", {
        name: translated("topology.mode.high_contrast"),
      }),
    );
    expect(root).toHaveClass("topology-high-contrast");
  });

  it("shows explicit empty states when topology or network boundaries are unavailable", () => {
    const { rerender } = render(<TopologyView topology={null} locale="zh-CN" />);
    expect(screen.getByText(translated("topology.empty.unavailable"))).toBeInTheDocument();

    rerender(
      <TopologyView
        topology={{ ...topologySnapshot(), boundaries: [], nodes: [], connections: [] }}
        locale="zh-CN"
      />,
    );
    expect(screen.getByText(translated("topology.empty.no_networks"))).toBeInTheDocument();
  });

  it("renders loading, partial, stale, error and permission states without inventing data", () => {
    const { rerender } = render(
      <TopologyView
        topology={topologySnapshot()}
        locale="zh-CN"
        presentationSignals={{ initialLoading: true }}
      />,
    );
    expect(screen.getByTestId("topology-state-initial_loading")).toBeInTheDocument();
    expect(
      screen.queryByRole("group", {
        name: translated("topology.aria.graph_title"),
      }),
    ).not.toBeInTheDocument();

    rerender(
      <TopologyView
        topology={{ ...topologySnapshot(), truncated: true }}
        locale="zh-CN"
        presentationSignals={{ stale: true }}
      />,
    );
    expect(screen.getByTestId("topology-state-stale")).toBeInTheDocument();
    expect(screen.getByTestId("topology-state-partial")).toBeInTheDocument();
    expect(
      screen.getByRole("group", {
        name: translated("topology.aria.graph_title"),
      }),
    ).toBeInTheDocument();

    rerender(
      <TopologyView
        topology={topologySnapshot()}
        locale="zh-CN"
        presentationSignals={{ error: "snapshot failed" }}
      />,
    );
    expect(screen.getByTestId("topology-state-error")).toBeInTheDocument();
    expect(
      screen.getByRole("group", {
        name: translated("topology.aria.graph_title"),
      }),
    ).toBeInTheDocument();

    rerender(
      <TopologyView
        topology={topologySnapshot()}
        locale="zh-CN"
        presentationSignals={{ permissionDenied: true }}
      />,
    );
    expect(screen.getByTestId("topology-state-permission_denied")).toBeInTheDocument();
    expect(
      screen.queryByRole("group", {
        name: translated("topology.aria.graph_title"),
      }),
    ).not.toBeInTheDocument();
  });
});

describe("Topology keyboard, touch and accessibility behavior", () => {
  it("navigates graph nodes and opens the action menu without a mouse", async () => {
    const user = userEvent.setup();
    const onOpenNode = vi.fn();
    const onToggleNode = vi.fn();
    const { container } = render(
      <TopologyView
        topology={topologySnapshot()}
        locale="zh-CN"
        onOpenNode={onOpenNode}
        onToggleNode={onToggleNode}
      />,
    );
    const self = container.querySelector(
      '[data-node-id="node-self"]',
    ) as SVGGElement;
    const shared = container.querySelector(
      '[data-node-id="node-shared"]',
    ) as SVGGElement;

    self.focus();
    await user.keyboard("{ArrowRight}");
    expect(shared).toHaveFocus();
    await user.keyboard("{Enter}");
    expect(onOpenNode).toHaveBeenCalledWith("node-shared");
    await user.keyboard(" ");
    expect(onToggleNode).toHaveBeenCalledWith("node-shared");

    await user.keyboard("{Shift>}{F10}{/Shift}");
    const menu = await screen.findByRole("menu");
    const openAction = within(menu).getByRole("menuitem", {
      name: translated("topology.menu.open"),
    });
    await waitFor(() => expect(openAction).toHaveFocus());
    const selectAction = within(menu).getByRole("menuitem", {
      name: translated("topology.menu.select"),
    });
    await user.keyboard("{ArrowDown}");
    expect(selectAction).toHaveFocus();
    await user.keyboard("{ArrowUp}");
    expect(openAction).toHaveFocus();
    await user.keyboard("{ArrowUp}");
    expect(selectAction).toHaveFocus();
    await user.keyboard("{End}");
    expect(selectAction).toHaveFocus();
    await user.keyboard("{Home}");
    expect(openAction).toHaveFocus();

    await user.keyboard("{Escape}");
    await waitFor(() => expect(shared).toHaveFocus());
    expect(screen.queryByRole("menu")).not.toBeInTheDocument();
  });

  it("uses the list by default on a narrow viewport and supports row navigation", async () => {
    vi.stubGlobal(
      "matchMedia",
      vi.fn().mockImplementation((query: string) => ({
        matches: query === "(max-width: 600px)",
        media: query,
        onchange: null,
        addEventListener: vi.fn(),
        removeEventListener: vi.fn(),
        addListener: vi.fn(),
        removeListener: vi.fn(),
        dispatchEvent: vi.fn(),
      })),
    );
    const user = userEvent.setup();
    const onOpenNode = vi.fn();
    const { container } = render(
      <TopologyView
        topology={topologySnapshot()}
        locale="zh-CN"
        onOpenNode={onOpenNode}
      />,
    );

    expect(container.querySelector(".topology-view")).toHaveAttribute(
      "data-view",
      "list",
    );
    const firstRow = container.querySelector(
      'tr[data-node-id="node-self"]',
    ) as HTMLTableRowElement;
    firstRow.focus();
    await user.keyboard("{ArrowDown}");
    expect(document.activeElement).not.toBe(firstRow);
    await user.keyboard("{Enter}");
    expect(onOpenNode).toHaveBeenCalled();
  });

  it("bounds virtual list rows for 1,000 nodes while preserving the full row count", async () => {
    vi.stubGlobal(
      "matchMedia",
      vi.fn().mockImplementation((query: string) => ({
        matches: query === "(max-width: 600px)",
        media: query,
        onchange: null,
        addEventListener: vi.fn(),
        removeEventListener: vi.fn(),
        addListener: vi.fn(),
        removeListener: vi.fn(),
        dispatchEvent: vi.fn(),
      })),
    );
    const { container } = render(
      <TopologyView topology={largeTopologySnapshot()} locale="zh-CN" />,
    );

    const firstRow = await waitFor(() => {
      const row = container.querySelector(
        'tr[data-node-id="node-0000"]',
      ) as HTMLTableRowElement | null;
      expect(row).not.toBeNull();
      return row!;
    });
    expect(
      container.querySelectorAll("tbody tr[data-node-id]").length,
    ).toBeLessThanOrEqual(40);
    expect(screen.getByRole("table")).toHaveAttribute("aria-rowcount", "1001");
    expect(firstRow).toHaveAttribute("aria-rowindex", "2");
  });

  it("opens the equivalent context menu after a touch long press and cancels on movement", async () => {
    const { container } = render(
      <TopologyView topology={topologySnapshot()} locale="zh-CN" />,
    );
    const self = container.querySelector(
      '[data-node-id="node-self"]',
    ) as SVGGElement;

    fireEvent.pointerDown(self, {
      pointerType: "touch",
      button: 0,
      clientX: 20,
      clientY: 20,
    });
    await act(async () => {
      await new Promise((resolve) => window.setTimeout(resolve, 500));
    });
    const menu = await screen.findByRole("menu");
    await waitFor(() =>
      expect(
        within(menu).getByRole("menuitem", {
          name: translated("topology.menu.open"),
        }),
      ).toHaveFocus(),
    );
    fireEvent.pointerUp(self);
    fireEvent.click(self);
    expect(menu).toBeInTheDocument();

    fireEvent.keyDown(document, { key: "Escape" });
    await waitFor(() => expect(screen.queryByRole("menu")).not.toBeInTheDocument());

    const shared = container.querySelector(
      '[data-node-id="node-shared"]',
    ) as SVGGElement;
    fireEvent.pointerDown(shared, {
      pointerType: "touch",
      button: 0,
      clientX: 20,
      clientY: 20,
    });
    fireEvent.pointerMove(shared, {
      pointerType: "touch",
      clientX: 40,
      clientY: 20,
    });
    await act(async () => {
      await new Promise((resolve) => window.setTimeout(resolve, 500));
    });
    expect(screen.queryByRole("menu")).not.toBeInTheDocument();
  });

  it("supports keyboard zoom, reset, locate and freeze without moving the frozen layout", async () => {
    const user = userEvent.setup();
    const scrollIntoView = vi.mocked(Element.prototype.scrollIntoView);
    const first = topologySnapshot();
    const { container, rerender } = render(
      <TopologyView topology={first} locale="zh-CN" />,
    );
    const initialTransform = container
      .querySelector('[data-node-id="node-self"]')
      ?.getAttribute("transform");

    await user.click(
      screen.getByRole("button", { name: translated("topology.zoom.in") }),
    );
    expect(container.querySelector(".topology-canvas svg")).toHaveAttribute(
      "data-zoom",
      "1.25",
    );
    await user.click(
      screen.getByRole("button", { name: translated("topology.zoom.reset") }),
    );
    expect(container.querySelector(".topology-canvas svg")).toHaveAttribute(
      "data-zoom",
      "1",
    );

    await user.click(
      screen.getByRole("button", {
        name: translated("topology.locate.current"),
      }),
    );
    expect(container.querySelector('[data-node-id="node-self"]')).toHaveFocus();
    expect(scrollIntoView).toHaveBeenCalled();

    await user.click(
      screen.getByRole("button", {
        name: translated("topology.freeze.freeze"),
      }),
    );
    expect(container.querySelector(".topology-view")).toHaveAttribute(
      "data-frozen",
      "true",
    );
    const next = {
      ...first,
      revision: first.revision + 1,
      nodes: [
        ...first.nodes,
        node("node-new", "New Node", "healthy", [
          { networkId: "net_beta", role: "member" },
        ]),
      ],
    };
    rerender(<TopologyView topology={next} locale="zh-CN" />);
    expect(
      container.querySelector('[data-node-id="node-self"]')?.getAttribute(
        "transform",
      ),
    ).toBe(initialTransform);
    expect(container.querySelector('[data-node-id="node-new"]')).toBeNull();

    await user.click(
      screen.getByRole("button", {
        name: translated("topology.freeze.unfreeze"),
      }),
    );
    expect(container.querySelector('[data-node-id="node-new"]')).toBeInTheDocument();
  });

  it("exposes capabilities, networks and relationship counts in the accessible name", () => {
    const { container } = render(
      <TopologyView topology={topologySnapshot()} locale="zh-CN" />,
    );
    const label = container
      .querySelector('[data-node-id="node-shared"]')
      ?.getAttribute("aria-label");

    expect(label).toContain(translated("capability.coordinator"));
    expect(label).toContain(translated("capability.file"));
    expect(label).toContain("Alpha");
    expect(label).toContain("Beta");
    expect(label).toContain(translated("topology.aria.relationships"));
    expect(label).toContain(translated("topology.connection.active"));
    expect(label).toContain(translated("topology.connection.idle"));
  });

  it("keeps focus across live refreshes and moves it to a fallback when the object disappears", async () => {
    const first = topologySnapshot();
    const { container, rerender } = render(
      <TopologyView topology={first} locale="zh-CN" />,
    );
    const shared = container.querySelector(
      '[data-node-id="node-shared"]',
    ) as SVGGElement;
    shared.focus();

    rerender(
      <TopologyView
        topology={{ ...first, revision: first.revision + 1 }}
        locale="zh-CN"
      />,
    );
    expect(
      container.querySelector('[data-node-id="node-shared"]'),
    ).toHaveFocus();

    rerender(
      <TopologyView
        topology={{
          ...first,
          revision: first.revision + 2,
          nodes: first.nodes.filter((item) => item.node_id !== "node-shared"),
          connections: first.connections.filter(
            (item) =>
              item.from !== "node-shared" && item.to !== "node-shared",
          ),
        }}
        locale="zh-CN"
      />,
    );
    await waitFor(() =>
      expect(container.querySelector('[data-node-id="node-self"]')).toHaveFocus(),
    );
  });
});

describe("Topology page integration", () => {
  it("renders the snapshot topology through PageContent", () => {
    const manifest = manifestFixture as NodeUiManifest;
    const topologyPage = manifest.pages.find((page) => page.id === "topology") as NodeUiPage;
    const snapshot = {
      ...(productFixture.snapshot as NodeSnapshot),
      topology: topologySnapshot(),
    };

    render(
      <PageContent
        page={topologyPage}
        snapshot={snapshot}
        locale="zh-CN"
        selectedNetwork="home"
        recentDiagnostics={null}
        onSelectNetwork={() => {}}
        onNavigate={() => {}}
        onRequestAction={() => {}}
        onRunAction={async () => {}}
      />,
    );

    expect(
      screen.getByRole("group", {
        name: translated("topology.aria.graph_title"),
      }),
    ).toBeInTheDocument();
    expect(screen.getByText("Shared Node")).toBeInTheDocument();
  });
});
