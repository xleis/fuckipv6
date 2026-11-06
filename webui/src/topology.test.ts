import { describe, expect, it } from "vitest";
import type { TopologySnapshot } from "../../contracts/typescript/node-contract-v1";
import {
  MAX_GRAPH_CONNECTIONS,
  MAX_GRAPH_NODES,
  createTopologyIndex,
  hiddenMemberCount,
  topologyGraphBudget,
  topologyLayoutKey,
  topologyListRows,
} from "./topology";

function capacitySnapshot(): TopologySnapshot {
  const nodeCount = 1_000;
  return {
    revision: 1,
    viewer: "node-0000",
    generated_at_unix_ms: 1_700_000_000_000,
    truncated: false,
    boundaries: [
      {
        network_id: "capacity",
        name: "Capacity",
        member_count: nodeCount,
        visible_member_count: 900,
        viewer_role: "owner",
      },
    ],
    clusters: [
      {
        network_id: "capacity",
        hidden_member_count: 100,
        expandable: false,
        sample: [],
      },
    ],
    nodes: Array.from({ length: nodeCount }, (_, index) => {
      const nodeId = `node-${String(index).padStart(4, "0")}`;
      return {
        node_id: nodeId,
        display_name: `Node ${index}`,
        health: index % 13 === 0 ? "degraded" : "healthy",
        is_self: index === 0,
        capabilities: index % 4 === 0 ? ["relay"] : [],
        networks: [
          {
            network_id: "capacity",
            health: "healthy",
            presence: { state: "online", source: "live_session" },
            role: index === 0 ? "owner" : "member",
            status: "active",
          },
        ],
      } satisfies TopologySnapshot["nodes"][number];
    }),
    connections: Array.from({ length: 5_000 }, (_, index) => ({
      connection_id: `connection-${index}`,
      from: `node-${String(index % nodeCount).padStart(4, "0")}`,
      to: `node-${String((index + 1) % nodeCount).padStart(4, "0")}`,
      networks: ["capacity"],
      observed_at_unix_ms: 1_700_000_000_000 + index,
      state: (["active", "idle", "stale"] as const)[index % 3],
    })),
  };
}

describe("topology capacity indexes", () => {
  it("counts 5,000 relationships once while building the reusable index", () => {
    const snapshot = capacitySnapshot();
    const index = createTopologyIndex(snapshot);

    expect(index.nodesById.size).toBe(1_000);
    expect(index.boundariesById.get("capacity")?.name).toBe("Capacity");
    expect(hiddenMemberCount(snapshot, "capacity", index)).toBe(100);
    const totalCounts = [...index.connectionCountsByNode.values()].reduce(
      (total, counts) =>
        total + counts.active + counts.idle + counts.stale,
      0,
    );
    expect(totalCounts).toBe(10_000);
    expect(topologyListRows(snapshot, "default", index)).toHaveLength(1_000);
  });

  it("bounds SVG nodes and connections while preserving self and selections", () => {
    const snapshot = capacitySnapshot();
    const budget = topologyGraphBudget(
      snapshot,
      new Set(["node-0999"]),
    );

    expect(budget.nodes).toHaveLength(MAX_GRAPH_NODES);
    expect(budget.connections.length).toBeLessThanOrEqual(
      MAX_GRAPH_CONNECTIONS,
    );
    expect(budget.nodes.some((node) => node.is_self)).toBe(true);
    expect(budget.nodes.some((node) => node.node_id === "node-0999")).toBe(
      true,
    );
    expect(budget.hiddenNodes).toBe(1_000 - MAX_GRAPH_NODES);
    expect(budget.hiddenConnections).toBe(5_000 - budget.connections.length);
  });

  it("keeps the layout key stable for connection-only refreshes", () => {
    const snapshot = capacitySnapshot();
    const before = topologyLayoutKey(snapshot);
    const after = topologyLayoutKey({
      ...snapshot,
      revision: 2,
      connections: snapshot.connections.map((connection) => ({
        ...connection,
        state: "active",
      })),
    });

    expect(after).toBe(before);
  });
});
