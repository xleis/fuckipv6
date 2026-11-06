import { describe, expect, it } from "vitest";
import type {
  NetworkMemberDirectory,
  TopologySnapshot,
} from "../../contracts/typescript/node-contract-v1";
import {
  EMPTY_TOPOLOGY_FILTERS,
  buildBatchMemberPlan,
  filteredTopologySnapshot,
  planForBatchAction,
  queryTopology,
  retainTopologySelection,
} from "./topology-actions";

function snapshot(): TopologySnapshot {
  return {
    revision: 3,
    viewer: "self",
    generated_at_unix_ms: 1,
    truncated: false,
    boundaries: [
      {
        network_id: "net-alpha",
        name: "Alpha",
        member_count: 3,
        visible_member_count: 3,
        viewer_role: "owner",
      },
      {
        network_id: "net-beta",
        name: "Beta",
        member_count: 1,
        visible_member_count: 1,
        viewer_role: "member",
      },
    ],
    nodes: [
      {
        node_id: "self",
        display_name: "Alice",
        health: "healthy",
        is_self: true,
        capabilities: ["coordinator"],
        networks: [
          {
            network_id: "net-alpha",
            health: "healthy",
            presence: { state: "online", source: "live_session" },
            role: "owner",
            status: "active",
          },
        ],
      },
      {
        node_id: "bob",
        display_name: "Bob",
        health: "degraded",
        is_self: false,
        capabilities: ["exit"],
        networks: [
          {
            network_id: "net-alpha",
            health: "degraded",
            presence: { state: "offline", source: "disconnect_observed" },
            role: "member",
            status: "degraded",
          },
          {
            network_id: "net-beta",
            health: "degraded",
            presence: { state: "offline", source: "disconnect_observed" },
            role: "administrator",
            status: "degraded",
          },
        ],
      },
      {
        node_id: "carol",
        display_name: "Carol",
        health: "unreachable",
        is_self: false,
        capabilities: ["relay"],
        networks: [
          {
            network_id: "net-alpha",
            health: "unreachable",
            presence: { state: "unknown", source: "not_observed" },
            role: "member",
            status: "suspended",
          },
        ],
      },
    ],
    connections: [
      {
        connection_id: "c1",
        from: "self",
        to: "bob",
        state: "active",
        networks: ["net-alpha"],
      },
      {
        connection_id: "c2",
        from: "bob",
        to: "carol",
        state: "idle",
        networks: ["net-alpha", "net-beta"],
      },
    ],
  };
}

function directory(): NetworkMemberDirectory {
  const member = {
    node_id: "bob",
    display_name: "Bob",
    role: "member" as const,
    groups: ["member" as const],
    capabilities: ["exit" as const],
    status: "approved" as const,
    lifecycle_state: "approved" as const,
    revision: 9,
    access_epoch: 4,
    available_actions: ["suspend" as const],
    presence: {
      state: "offline" as const,
      source: "disconnect_observed" as const,
    },
  };
  return {
    network_id: "net-alpha",
    generated_at_unix_ms: 2,
    viewer_role: "owner",
    can_manage_members: true,
    members: [
      member,
      {
        ...member,
        node_id: "carol",
        display_name: "Carol",
        available_actions: ["resume" as const],
      },
    ],
  };
}

describe("topology action planning", () => {
  it("shares search, role, capability, health and network filters with the list", () => {
    const filters = { ...EMPTY_TOPOLOGY_FILTERS, search: "bo" };
    expect(queryTopology(snapshot(), filters).map((node) => node.node_id)).toEqual([
      "bob",
    ]);
    const filtered = filteredTopologySnapshot(snapshot(), {
      ...EMPTY_TOPOLOGY_FILTERS,
      role: "administrator",
      capability: "exit",
      health: "degraded",
      networkId: "net-beta",
    });
    expect(filtered.nodes.map((node) => node.node_id)).toEqual(["bob"]);
    expect(filtered.connections).toEqual([]);
  });

  it("retains only still-present identities after a refresh", () => {
    expect(
      [...retainTopologySelection(snapshot(), new Set(["bob", "gone"]))],
    ).toEqual(["bob"]);
  });

  it("reports per-member permission differences before execution", () => {
    const plan = buildBatchMemberPlan(
      snapshot(),
      new Set(["bob", "carol"]),
      "net-alpha",
      directory(),
    );
    expect(plan.actionOptions).toEqual([
      { action: "suspend", eligibleCount: 1, totalCount: 2 },
      { action: "resume", eligibleCount: 1, totalCount: 2 },
    ]);
    const suspend = planForBatchAction(plan, "suspend");
    expect(suspend.eligible.map((candidate) => candidate.nodeId)).toEqual(["bob"]);
    expect(suspend.skipped).toEqual([
      expect.objectContaining({
        nodeId: "carol",
        skipReason: "action_unavailable",
      }),
    ]);
  });

  it("blocks every member when the server did not grant management", () => {
    const plan = buildBatchMemberPlan(
      snapshot(),
      new Set(["bob"]),
      "net-alpha",
      { ...directory(), can_manage_members: false },
    );
    expect(plan.eligible).toEqual([]);
    expect(plan.skipped[0]).toEqual(
      expect.objectContaining({ skipReason: "not_manageable" }),
    );
  });
});
