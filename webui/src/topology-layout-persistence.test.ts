import { describe, expect, it, vi } from "vitest";
import type { TopologySnapshot } from "../../contracts/typescript/node-contract-v1";
import { layoutTopology } from "./topology";
import {
  MAX_PERSISTED_TOPOLOGY_POSITIONS,
  TOPOLOGY_LAYOUT_STORAGE_KEY,
  applyFixedTopologyPositions,
  clearPersistedTopologyPositions,
  loadPersistedTopologyPositions,
  parsePersistedTopologyPositions,
  savePersistedTopologyPositions,
  serializeTopologyPositions,
} from "./topology-layout-persistence";

function snapshot(): TopologySnapshot {
  return {
    revision: 1,
    viewer: "node-a",
    generated_at_unix_ms: 1_700_000_000_000,
    truncated: false,
    boundaries: [
      {
        network_id: "network-a",
        name: "Private network name",
        member_count: 2,
        visible_member_count: 2,
        viewer_role: "owner",
      },
    ],
    clusters: [],
    nodes: [
      {
        node_id: "node-a",
        display_name: "Private node name",
        health: "healthy",
        is_self: true,
        capabilities: ["relay"],
        networks: [
          {
            network_id: "network-a",
            role: "owner",
            status: "active",
            health: "healthy",
            presence: { source: "live_session", state: "online" },
          },
        ],
      },
      {
        node_id: "node-b",
        display_name: "Another name",
        health: "degraded",
        is_self: false,
        capabilities: [],
        networks: [
          {
            network_id: "network-a",
            role: "member",
            status: "active",
            health: "degraded",
            presence: { source: "live_session", state: "online" },
          },
        ],
      },
    ],
    connections: [],
  };
}

function memoryStorage(initial: Record<string, string> = {}) {
  const values = new Map(Object.entries(initial));
  return {
    getItem: vi.fn((key: string) => values.get(key) ?? null),
    removeItem: vi.fn((key: string) => {
      values.delete(key);
    }),
    setItem: vi.fn((key: string, value: string) => {
      values.set(key, value);
    }),
    values,
  };
}

describe("topology layout persistence", () => {
  it("round-trips a versioned position document without snapshot metadata", () => {
    const positions = new Map([
      ["node-b", { x: 155.5, y: 210 }],
      ["node-a", { x: 100, y: 120 }],
    ]);
    const encoded = serializeTopologyPositions(positions);
    const parsed = parsePersistedTopologyPositions(encoded);

    expect(parsed.status).toBe("loaded");
    expect([...parsed.positions.entries()]).toEqual([
      ["node-a", { x: 100, y: 120 }],
      ["node-b", { x: 155.5, y: 210 }],
    ]);
    expect(encoded).not.toContain("Private node name");
    expect(encoded).not.toContain("Private network name");
    expect(encoded).not.toContain("healthy");
    expect(encoded).not.toContain("member");
  });

  it.each([
    ["malformed JSON", "{"],
    ["wrong version", JSON.stringify({ version: 99, positions: {} })],
    [
      "invalid coordinate",
      JSON.stringify({ version: 1, positions: { "node-a": { x: "1", y: 2 } } }),
    ],
    [
      "non-finite coordinate",
      '{"version":1,"positions":{"node-a":{"x":1e999,"y":2}}}',
    ],
  ])("falls back to automatic layout for %s", (_name, raw) => {
    const parsed = parsePersistedTopologyPositions(raw);
    expect(parsed.positions.size).toBe(0);
    expect(["invalid", "unsupported_version"]).toContain(parsed.status);
  });

  it("rejects an unbounded stored position set", () => {
    const positions = Object.fromEntries(
      Array.from({ length: MAX_PERSISTED_TOPOLOGY_POSITIONS + 1 }, (_, index) => [
        `node-${index}`,
        { x: index, y: index },
      ]),
    );
    const parsed = parsePersistedTopologyPositions(
      JSON.stringify({ version: 1, positions }),
    );
    expect(parsed.status).toBe("invalid");
    expect(parsed.positions.size).toBe(0);
  });

  it("applies fixed coordinates while preserving authoritative layout ownership", () => {
    const automatic = layoutTopology(snapshot());
    const original = automatic.placements.find(
      (placement) => placement.nodeId === "node-b",
    );
    const fixed = applyFixedTopologyPositions(
      automatic,
      new Map([["node-b", { x: 44, y: 300 }]]),
    );
    const moved = fixed.placements.find(
      (placement) => placement.nodeId === "node-b",
    );

    expect(moved).toMatchObject({
      nodeId: "node-b",
      anchorNetworkId: original?.anchorNetworkId,
      slot: original?.slot,
      x: 44,
      y: 300,
    });
    expect(automatic.placements.find((placement) => placement.nodeId === "node-b"))
      .toMatchObject({ x: original?.x, y: original?.y });
  });

  it("clamps restored positions to the current layout bounds", () => {
    const automatic = layoutTopology(snapshot());
    const fixed = applyFixedTopologyPositions(
      automatic,
      new Map([["node-a", { x: -900, y: 9_999 }]]),
    );
    expect(fixed.placements.find((placement) => placement.nodeId === "node-a"))
      .toMatchObject({ x: 32, y: automatic.height - 32 });
  });

  it("loads, saves and clears through storage while containing storage errors", () => {
    const storage = memoryStorage();
    expect(
      savePersistedTopologyPositions(
        new Map([["node-a", { x: 110, y: 130 }]]),
        storage,
      ),
    ).toBe(true);
    expect(storage.values.has(TOPOLOGY_LAYOUT_STORAGE_KEY)).toBe(true);
    expect([...loadPersistedTopologyPositions(storage).positions]).toEqual([
      ["node-a", { x: 110, y: 130 }],
    ]);
    expect(clearPersistedTopologyPositions(storage)).toBe(true);
    expect(storage.values.has(TOPOLOGY_LAYOUT_STORAGE_KEY)).toBe(false);

    const oversized = new Map(
      Array.from({ length: MAX_PERSISTED_TOPOLOGY_POSITIONS + 1 }, (_, index) => [
        `node-${index}`,
        { x: index, y: index },
      ]),
    );
    expect(savePersistedTopologyPositions(oversized, storage)).toBe(false);
    expect(storage.values.has(TOPOLOGY_LAYOUT_STORAGE_KEY)).toBe(false);

    const unavailable = {
      getItem: vi.fn(() => {
        throw new Error("blocked");
      }),
      setItem: vi.fn(() => {
        throw new Error("blocked");
      }),
      removeItem: vi.fn(() => {
        throw new Error("blocked");
      }),
    };
    expect(loadPersistedTopologyPositions(unavailable)).toEqual({
      positions: new Map(),
      status: "unavailable",
    });
    expect(
      savePersistedTopologyPositions(
        new Map([["node-a", { x: 1, y: 2 }]]),
        unavailable,
      ),
    ).toBe(false);
    expect(clearPersistedTopologyPositions(unavailable)).toBe(false);
  });
});
