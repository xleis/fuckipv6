import { describe, expect, it } from "vitest";
import productFixture from "../../contracts/typescript/fixtures/product-visual-snapshot.json";
import type { NodeSnapshot } from "../../contracts/typescript/node-contract-v1";
import {
  interactionBlockReason,
  pageHasContent,
  pageStateSignals,
  primaryPageState,
} from "./state-matrix";

const snapshot = productFixture.snapshot as NodeSnapshot;
const pageIds = [
  "overview",
  "networks",
  "peers",
  "node_runtime",
  "topology",
  "traffic",
  "routing",
  "exit_node",
  "relay",
  "logs",
  "identity",
  "settings",
] as const;

describe("page state matrix", () => {
  it("resolves exactly one state using the documented product priority", () => {
    const priorities: Array<[Parameters<typeof primaryPageState>[0], string]> = [
      [{ error: "failed", initialLoading: true }, "error"],
      [{ initialLoading: true, browserOnline: false }, "initial_loading"],
      [{ browserOnline: false, maintenance: true }, "offline"],
      [{ maintenance: true, permissionDenied: true }, "maintenance"],
      [{ permissionDenied: true, unsupported: true }, "permission_denied"],
      [{ unsupported: true, nodePhase: "failed" }, "unsupported"],
      [
        {
          nodePhase: "failed",
          degraded: true,
          controlHealth: "unavailable",
        },
        "node_failed",
      ],
      [{ degraded: true, controlHealth: "unavailable" }, "degraded"],
      [{ controlHealth: "unavailable", stale: true }, "control_unavailable"],
      [{ stale: true, refreshing: true }, "stale"],
      [{ refreshing: true, empty: true }, "refreshing"],
      [{ empty: true }, "empty"],
      [{}, "ready"],
    ];

    for (const [signals, expected] of priorities) {
      const state = primaryPageState(signals);
      expect(state.kind).toBe(expected);
      expect(state).not.toHaveProperty("states");
    }
  });

  it("keeps refreshing non-blocking and distinct from stale data", () => {
    expect(primaryPageState({ refreshing: true, stale: true }).kind).toBe("stale");
    expect(interactionBlockReason({ refreshing: true })).toBeNull();
    expect(interactionBlockReason({ stale: true })).toBe("stale");
  });

  it("keeps an actual error ahead of initial loading", () => {
    expect(primaryPageState({ initialLoading: true, error: "failed" }).kind).toBe(
      "error",
    );
    expect(interactionBlockReason({ initialLoading: true, error: "failed" })).toBe(
      "error",
    );
  });

  it("separates offline, control-plane, and node failures", () => {
    expect(primaryPageState({ browserOnline: false, controlHealth: "unavailable" }).kind).toBe(
      "offline",
    );
    expect(primaryPageState({ controlHealth: "unavailable", nodePhase: "failed" }).kind).toBe(
      "node_failed",
    );
    expect(primaryPageState({ controlHealth: "available", nodePhase: "failed" }).kind).toBe(
      "node_failed",
    );
  });

  it("treats maintenance and unsupported capabilities as explicit blockers", () => {
    expect(primaryPageState({ maintenance: true }).kind).toBe("maintenance");
    expect(primaryPageState({ unsupported: true }).kind).toBe("unsupported");
    expect(interactionBlockReason({ maintenance: true })).toBe("maintenance");
    expect(interactionBlockReason({ unsupported: true })).toBe("unsupported");
  });

  it("resolves empty state for every page without treating it as an error", () => {
    const empty = {
      ...snapshot,
      networks: [],
      peers: [],
      logs: [],
      identity: null,
      configuration: { networks: [] },
    };
    const contents = pageIds.map((pageId) => pageHasContent(empty, pageId));
    expect(contents).toEqual([
      true,
      false,
      false,
      true,
      false,
      false,
      false,
      false,
      false,
      false,
      false,
      true,
    ]);
    for (const pageId of pageIds) {
      const state = primaryPageState(
        pageStateSignals(empty, pageId, { browserOnline: true }),
      );
      expect(state.kind).not.toBe("error");
      expect(state.blocking).toBe(false);
    }
  });

  it("derives a stable state and write gate for every manifest page", () => {
    const healthySnapshot: NodeSnapshot = {
      ...snapshot,
      lifecycle: {
        active_networks: 0,
        configured_networks: 0,
        failed_networks: [],
        phase: "stopped",
      },
      last_error: null,
      networks: [],
      peers: [],
      logs: [],
      embedded_services: [],
      identity: null,
      configuration: { networks: [] },
    };

    for (const pageId of pageIds) {
      const signals = pageStateSignals(healthySnapshot, pageId, {
        browserOnline: true,
      });
      const primary = primaryPageState(signals);
      expect(primary.kind).toBe(pageHasContent(healthySnapshot, pageId) ? "ready" : "empty");
      expect(interactionBlockReason(signals)).toBeNull();

      const staleSignals = pageStateSignals(healthySnapshot, pageId, {
        browserOnline: true,
        stale: true,
      });
      expect(primaryPageState(staleSignals).kind).toBe("stale");
      expect(interactionBlockReason(staleSignals)).toBe("stale");
    }
  });
});
