import type { TopologyLayout, TopologyPlacement } from "./topology";

export const TOPOLOGY_LAYOUT_STORAGE_KEY = "fuckipv6.topology.layout.v1";
export const TOPOLOGY_LAYOUT_STORAGE_VERSION = 1;
export const MAX_PERSISTED_TOPOLOGY_POSITIONS = 2_000;

export type TopologyNodePosition = {
  x: number;
  y: number;
};

export type TopologyFixedPositions = ReadonlyMap<string, TopologyNodePosition>;

export type TopologyLayoutLoadStatus =
  | "empty"
  | "loaded"
  | "invalid"
  | "unsupported_version"
  | "unavailable";

export type TopologyLayoutLoadResult = {
  positions: TopologyFixedPositions;
  status: TopologyLayoutLoadStatus;
};

type StorageLike = Pick<Storage, "getItem" | "removeItem" | "setItem">;

const EMPTY_POSITIONS: TopologyFixedPositions = new Map();

function browserStorage(): StorageLike | null {
  if (typeof window === "undefined") return null;
  try {
    return window.localStorage;
  } catch {
    return null;
  }
}

function validNodeId(value: string): boolean {
  return value.length > 0 && value.length <= 512 && !value.includes("\0");
}

function validPosition(value: unknown): value is TopologyNodePosition {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    return false;
  }
  const position = value as Record<string, unknown>;
  return (
    typeof position.x === "number" &&
    Number.isFinite(position.x) &&
    Math.abs(position.x) <= 10_000 &&
    typeof position.y === "number" &&
    Number.isFinite(position.y) &&
    Math.abs(position.y) <= 10_000
  );
}

/** Only stable node IDs and coordinates are persisted. Names, roles,
 * addresses, health and connection data never enter browser storage. */
export function parsePersistedTopologyPositions(
  raw: string | null,
): { positions: TopologyFixedPositions; status: TopologyLayoutLoadStatus } {
  if (raw === null) return { positions: EMPTY_POSITIONS, status: "empty" };
  let decoded: unknown;
  try {
    decoded = JSON.parse(raw);
  } catch {
    return { positions: EMPTY_POSITIONS, status: "invalid" };
  }
  if (typeof decoded !== "object" || decoded === null || Array.isArray(decoded)) {
    return { positions: EMPTY_POSITIONS, status: "invalid" };
  }
  const document = decoded as Record<string, unknown>;
  if (document.version !== TOPOLOGY_LAYOUT_STORAGE_VERSION) {
    return { positions: EMPTY_POSITIONS, status: "unsupported_version" };
  }
  const storedPositions = document.positions;
  if (
    typeof storedPositions !== "object" ||
    storedPositions === null ||
    Array.isArray(storedPositions)
  ) {
    return { positions: EMPTY_POSITIONS, status: "invalid" };
  }
  const entries = Object.entries(storedPositions);
  if (entries.length > MAX_PERSISTED_TOPOLOGY_POSITIONS) {
    return { positions: EMPTY_POSITIONS, status: "invalid" };
  }
  const positions = new Map<string, TopologyNodePosition>();
  for (const [nodeId, position] of entries) {
    if (!validNodeId(nodeId) || !validPosition(position)) {
      return { positions: EMPTY_POSITIONS, status: "invalid" };
    }
    positions.set(nodeId, { x: position.x, y: position.y });
  }
  return { positions, status: positions.size > 0 ? "loaded" : "empty" };
}

export function loadPersistedTopologyPositions(
  storage: StorageLike | null = browserStorage(),
): TopologyLayoutLoadResult {
  if (!storage) return { positions: EMPTY_POSITIONS, status: "unavailable" };
  try {
    return parsePersistedTopologyPositions(
      storage.getItem(TOPOLOGY_LAYOUT_STORAGE_KEY),
    );
  } catch {
    return { positions: EMPTY_POSITIONS, status: "unavailable" };
  }
}

export function serializeTopologyPositions(
  positions: TopologyFixedPositions,
): string {
  return JSON.stringify({
    version: TOPOLOGY_LAYOUT_STORAGE_VERSION,
    positions: Object.fromEntries(
      [...positions.entries()]
        .sort(([left], [right]) => left.localeCompare(right))
        .map(([nodeId, position]) => [
          nodeId,
          { x: position.x, y: position.y },
        ]),
    ),
  });
}

export function savePersistedTopologyPositions(
  positions: TopologyFixedPositions,
  storage: StorageLike | null = browserStorage(),
): boolean {
  if (!storage || positions.size > MAX_PERSISTED_TOPOLOGY_POSITIONS) return false;
  try {
    storage.setItem(
      TOPOLOGY_LAYOUT_STORAGE_KEY,
      serializeTopologyPositions(positions),
    );
    return true;
  } catch {
    return false;
  }
}

export function clearPersistedTopologyPositions(
  storage: StorageLike | null = browserStorage(),
): boolean {
  if (!storage) return false;
  try {
    storage.removeItem(TOPOLOGY_LAYOUT_STORAGE_KEY);
    return true;
  } catch {
    return false;
  }
}

export function clampTopologyNodePosition(
  position: TopologyNodePosition,
  layout: Pick<TopologyLayout, "width" | "height">,
): TopologyNodePosition {
  return {
    x: Math.max(32, Math.min(layout.width - 32, position.x)),
    y: Math.max(32, Math.min(layout.height - 32, position.y)),
  };
}

/** Fixed positions only override coordinates. Their network ownership, list
 * order and anchor remain derived from the current authoritative snapshot. */
export function applyFixedTopologyPositions(
  layout: TopologyLayout,
  positions: TopologyFixedPositions,
): TopologyLayout {
  if (positions.size === 0) return layout;
  let changed = false;
  const placements = layout.placements.map((placement): TopologyPlacement => {
    const fixed = positions.get(placement.nodeId);
    if (!fixed) return placement;
    const clamped = clampTopologyNodePosition(fixed, layout);
    if (clamped.x === placement.x && clamped.y === placement.y) {
      return placement;
    }
    changed = true;
    return { ...placement, ...clamped };
  });
  return changed ? { ...layout, placements } : layout;
}
