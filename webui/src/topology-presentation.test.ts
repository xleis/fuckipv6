import { describe, expect, it } from "vitest";
import type { TopologySnapshot } from "../../contracts/typescript/node-contract-v1";
import { topologyPresentation } from "./topology-presentation";

function topology(
  overrides: Partial<TopologySnapshot> = {},
): TopologySnapshot {
  return {
    revision: 7,
    generated_at_unix_ms: 1_700_000_000_000,
    viewer: "node-self",
    truncated: false,
    boundaries: [
      {
        network_id: "net-alpha",
        name: "Alpha",
        member_count: 1,
        visible_member_count: 1,
        viewer_role: "member",
      },
    ],
    nodes: [],
    connections: [],
    ...overrides,
  };
}

describe("topology presentation", () => {
  it("distinguishes ready, empty and unavailable authoritative states", () => {
    expect(topologyPresentation(topology()).kind).toBe("ready");
    expect(topologyPresentation(topology({ boundaries: [] }))).toMatchObject({
      kind: "empty",
      hasContent: false,
      canRender: false,
    });
    expect(topologyPresentation(null)).toMatchObject({
      kind: "unavailable",
      hasContent: false,
      canRender: false,
    });
  });

  it("surfaces truncated data without discarding renderable objects", () => {
    const presentation = topologyPresentation(topology({ truncated: true }));
    expect(presentation).toMatchObject({
      kind: "partial",
      conditions: ["partial"],
      hasContent: true,
      canRender: true,
    });
    expect(presentation.titleKey).toBe("topology.state.partial.title");
  });

  it("retains content for stale, offline and refreshing states", () => {
    for (const signals of [
      { stale: true },
      { offline: true },
      { refreshing: true },
    ]) {
      const presentation = topologyPresentation(topology(), signals);
      expect(presentation.hasContent).toBe(true);
      expect(presentation.canRender).toBe(true);
    }
    expect(topologyPresentation(topology(), { stale: true }).kind).toBe(
      "stale",
    );
    expect(topologyPresentation(topology(), { offline: true }).kind).toBe(
      "offline",
    );
  });

  it("keeps every simultaneous condition while using the highest priority", () => {
    const presentation = topologyPresentation(topology({ truncated: true }), {
      refreshing: true,
      stale: true,
      offline: true,
    });
    expect(presentation.kind).toBe("offline");
    expect(presentation.conditions).toEqual([
      "offline",
      "stale",
      "refreshing",
      "partial",
    ]);
  });

  it("blocks error, loading and permission denied content explicitly", () => {
    expect(
      topologyPresentation(topology(), { error: "snapshot unavailable" }),
    ).toMatchObject({
      kind: "error",
      canRender: true,
      conditions: ["error"],
    });
    expect(
      topologyPresentation(topology(), { initialLoading: true }),
    ).toMatchObject({
      kind: "initial_loading",
      canRender: false,
    });
    expect(
      topologyPresentation(topology(), { permissionDenied: true }),
    ).toMatchObject({
      kind: "permission_denied",
      canRender: false,
    });
  });
});
