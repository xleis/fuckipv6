import type {
  CapabilityKind,
  MembershipRole,
  NodeId,
  TopologyBoundary,
  TopologyConnection,
  TopologyConnectionState,
  TopologyNode,
  TopologyNodeHealth,
  TopologySnapshot,
} from "../../contracts/typescript/node-contract-v1";

/// Stable ring order shared with the domain capability model.
export const CAPABILITY_ORDER = [
  "coordinator",
  "relay",
  "exit",
  "file",
] as const satisfies readonly CapabilityKind[];

export const HEALTH_ORDER = [
  "healthy",
  "unknown",
  "degraded",
  "unreachable",
] as const satisfies readonly TopologyNodeHealth[];

export const CONNECTION_ORDER = [
  "active",
  "idle",
  "stale",
] as const satisfies readonly TopologyConnectionState[];

export type TopologyColorMode = "default" | "colorblind";

/// Network colours never reuse capability colours. They are always paired with
/// a distinct dash pattern and a text label, so no meaning depends on hue.
const NETWORK_PALETTE_DEFAULT = [
  "#4f6fd8",
  "#c9721f",
  "#2f8a5b",
  "#9a55c4",
  "#c04f68",
  "#2b8b99",
  "#7a6f24",
  "#6c62c9",
] as const;

/// Okabe-Ito palette: distinguishable for the common colour-vision
/// deficiencies while remaining readable on light and dark backgrounds.
const NETWORK_PALETTE_COLORBLIND = [
  "#0072b2",
  "#e69f00",
  "#009e73",
  "#cc79a7",
  "#d55e00",
  "#56b4e9",
  "#6b6b00",
  "#4a4a9c",
] as const;

const NETWORK_DASHES = [
  "7 4",
  "2 3",
  "10 3 2 3",
  "4 3 1 3",
  "12 4",
  "3 2",
  "8 2 2 2 2 2",
  "5 5",
] as const;

const CAPABILITY_COLORS_DEFAULT: Record<CapabilityKind, string> = {
  coordinator: "#7c5cff",
  relay: "#1f9d8b",
  exit: "#d9822b",
  file: "#3b82f6",
};

const CAPABILITY_COLORS_COLORBLIND: Record<CapabilityKind, string> = {
  coordinator: "#cc79a7",
  relay: "#009e73",
  exit: "#e69f00",
  file: "#56b4e9",
};

export type NetworkVisual = {
  networkId: string;
  color: string;
  dash: string;
};

/** Deterministic, index-based network encoding: same snapshot, same visuals. */
export function networkVisuals(
  snapshot: TopologySnapshot,
  mode: TopologyColorMode = "default",
): Map<string, NetworkVisual> {
  const palette =
    mode === "colorblind" ? NETWORK_PALETTE_COLORBLIND : NETWORK_PALETTE_DEFAULT;
  const visuals = new Map<string, NetworkVisual>();
  snapshot.boundaries.forEach((boundary, index) => {
    visuals.set(boundary.network_id, {
      networkId: boundary.network_id,
      color: palette[index % palette.length],
      dash: NETWORK_DASHES[index % NETWORK_DASHES.length],
    });
  });
  return visuals;
}

export function capabilityColor(
  capability: CapabilityKind,
  mode: TopologyColorMode = "default",
): string {
  return (mode === "colorblind" ? CAPABILITY_COLORS_COLORBLIND : CAPABILITY_COLORS_DEFAULT)[
    capability
  ];
}

export type CapabilitySegment = {
  capability: CapabilityKind;
  enabled: boolean;
  startAngle: number;
  endAngle: number;
  color: string;
};

/// Four fixed quadrants so an empty segment means "not observed", never "no
/// ring". The renderer draws disabled segments faint and dashed.
export function capabilitySegments(
  node: Pick<TopologyNode, "capabilities">,
  mode: TopologyColorMode = "default",
): CapabilitySegment[] {
  const enabled = new Set<CapabilityKind>(node.capabilities ?? []);
  return CAPABILITY_ORDER.map((capability, index) => ({
    capability,
    enabled: enabled.has(capability),
    startAngle: index * 90 - 90 + 4,
    endAngle: (index + 1) * 90 - 90 - 4,
    color: capabilityColor(capability, mode),
  }));
}

export type HealthShape = "circle" | "diamond" | "triangle" | "square";

export type HealthEncoding = {
  shape: HealthShape;
  glyph: string;
  tone: "good" | "warn" | "bad" | "unknown";
};

/// Health is encoded by shape *and* glyph, never by colour alone.
export function healthEncoding(health: TopologyNodeHealth): HealthEncoding {
  switch (health) {
    case "healthy":
      return { shape: "circle", glyph: "✓", tone: "good" };
    case "degraded":
      return { shape: "triangle", glyph: "!", tone: "warn" };
    case "unreachable":
      return { shape: "square", glyph: "×", tone: "bad" };
    case "unknown":
    default:
      return { shape: "diamond", glyph: "?", tone: "unknown" };
  }
}

export type TopologyPlacement = {
  nodeId: NodeId;
  anchorNetworkId: string;
  slot: number;
  x: number;
  y: number;
};

export type TopologyAnchor = {
  networkId: string;
  slot: number;
  x: number;
  y: number;
};

export type TopologyLayout = {
  width: number;
  height: number;
  anchors: TopologyAnchor[];
  placements: TopologyPlacement[];
};

export type TopologyConnectionCounts = Record<TopologyConnectionState, number>;

/// Reusable indexes for paths that used to scan every node × connection or
/// boundary × cluster pair on each render.
export type TopologyIndex = {
  nodesById: Map<NodeId, TopologyNode>;
  boundariesById: Map<string, TopologyBoundary>;
  connectionCountsByNode: Map<NodeId, TopologyConnectionCounts>;
  hiddenMembersByNetwork: Map<string, number>;
  searchFieldsByNode: Map<NodeId, { name: string; nodeId: string }>;
  source: {
    nodes: TopologyNode[];
    boundaries: TopologyBoundary[];
    connections: TopologyConnection[];
    clusters: NonNullable<TopologySnapshot["clusters"]>;
  };
};

function connectionCounts(): TopologyConnectionCounts {
  return { active: 0, idle: 0, stale: 0 };
}

export function createTopologyIndex(
  snapshot: TopologySnapshot,
  previous?: TopologyIndex | null,
): TopologyIndex {
  const cached = previous ?? undefined;
  const previousSource = cached?.source;
  const nodesById = new Map(
    snapshot.nodes.map((node) => [node.node_id, node]),
  );
  const boundariesById =
    cached !== undefined && previousSource?.boundaries === snapshot.boundaries
      ? cached.boundariesById
      : new Map(
          snapshot.boundaries.map((boundary) => [
            boundary.network_id,
            boundary,
          ]),
        );

  let sameNodeIds =
    cached !== undefined &&
    cached.nodesById.size === snapshot.nodes.length;
  if (sameNodeIds && cached !== undefined) {
    for (const node of snapshot.nodes) {
      if (!cached.nodesById.has(node.node_id)) {
        sameNodeIds = false;
        break;
      }
    }
  }
  const connectionCountsByNode =
    cached !== undefined &&
    previousSource?.connections === snapshot.connections &&
    sameNodeIds
      ? cached.connectionCountsByNode
      : (() => {
          const counts = new Map<NodeId, TopologyConnectionCounts>();
          for (const node of snapshot.nodes) {
            counts.set(node.node_id, connectionCounts());
          }
          for (const connection of snapshot.connections) {
            const from = counts.get(connection.from);
            if (from) from[connection.state] += 1;
            if (connection.to !== connection.from) {
              const to = counts.get(connection.to);
              if (to) to[connection.state] += 1;
            }
          }
          return counts;
        })();

  const searchFieldsByNode =
    cached !== undefined && previousSource?.nodes === snapshot.nodes
      ? cached.searchFieldsByNode
      : new Map(
          snapshot.nodes.map((node) => [
            node.node_id,
            {
              name: (node.display_name ?? "").toLocaleLowerCase(),
              nodeId: node.node_id.toLocaleLowerCase(),
            },
          ]),
        );
  const clusters = snapshot.clusters ?? [];
  const hiddenMembersByNetwork =
    cached !== undefined && previousSource?.clusters === clusters
      ? cached.hiddenMembersByNetwork
      : (() => {
          const hidden = new Map<string, number>();
          for (const cluster of clusters) {
            hidden.set(
              cluster.network_id,
              (hidden.get(cluster.network_id) ?? 0) +
                cluster.hidden_member_count,
            );
          }
          return hidden;
        })();

  return {
    nodesById,
    boundariesById,
    connectionCountsByNode,
    hiddenMembersByNetwork,
    searchFieldsByNode,
    source: {
      nodes: snapshot.nodes,
      boundaries: snapshot.boundaries,
      connections: snapshot.connections,
      clusters,
    },
  };
}

/**
 * Snapshot patches preserve unchanged array identities. Keying the last index
 * by the connection array lets a name/health refresh reuse all expensive
 * sub-indexes without reading a React ref during render.
 */
const topologyIndexesByConnections = new WeakMap<
  TopologyConnection[],
  TopologyIndex
>();

export function cachedTopologyIndex(snapshot: TopologySnapshot): TopologyIndex {
  const previous = topologyIndexesByConnections.get(snapshot.connections);
  const index = createTopologyIndex(snapshot, previous);
  topologyIndexesByConnections.set(snapshot.connections, index);
  return index;
}

/** Layout only depends on boundaries and node network membership, never on
 * connection traffic, so refreshes that only update health keep their frame. */
export function topologyLayoutKey(snapshot: TopologySnapshot): string {
  return `${snapshot.boundaries
    .map((boundary) => boundary.network_id)
    .join(",")}|${snapshot.nodes
    .map((node) => `${node.node_id}:${node.networks.map((network) => network.network_id).join("+")}`)
    .join(",")}`;
}

export const MAX_GRAPH_NODES = 80;
export const MAX_GRAPH_CONNECTIONS = 200;

export type TopologyGraphBudget = {
  nodes: TopologyNode[];
  connections: TopologyConnection[];
  hiddenNodes: number;
  hiddenConnections: number;
};

/** Bound SVG work while keeping every object available through the
 * virtualized list. Self and selected nodes stay visible when possible. */
export function topologyGraphBudget(
  snapshot: TopologySnapshot,
  selectedNodeIds: ReadonlySet<string> = new Set(),
  maxNodes = MAX_GRAPH_NODES,
  maxConnections = MAX_GRAPH_CONNECTIONS,
): TopologyGraphBudget {
  if (
    snapshot.nodes.length <= maxNodes &&
    snapshot.connections.length <= maxConnections
  ) {
    return {
      nodes: snapshot.nodes,
      connections: snapshot.connections,
      hiddenNodes: 0,
      hiddenConnections: 0,
    };
  }
  const priorities = new Map<string, number>();
  snapshot.nodes.forEach((node, index) => {
    let priority = index;
    if (node.is_self) priority -= snapshot.nodes.length + 2;
    if (selectedNodeIds.has(node.node_id)) priority -= snapshot.nodes.length + 1;
    priorities.set(node.node_id, priority);
  });
  const nodes = [...snapshot.nodes]
    .sort(
      (left, right) =>
        (priorities.get(left.node_id) ?? 0) -
          (priorities.get(right.node_id) ?? 0) ||
        left.node_id.localeCompare(right.node_id),
    )
    .slice(0, maxNodes);
  const visible = new Set(nodes.map((node) => node.node_id));
  const connections = snapshot.connections
    .filter(
      (connection) =>
        visible.has(connection.from) && visible.has(connection.to),
    )
    .slice(0, maxConnections);
  return {
    nodes,
    connections,
    hiddenNodes: snapshot.nodes.length - nodes.length,
    hiddenConnections: snapshot.connections.length - connections.length,
  };
}

const ANCHOR_SLOTS = 12;
const NODE_SLOTS_PER_RING = 16;

function anchorPosition(
  width: number,
  height: number,
  slot: number,
  boundaryCount: number,
) {
  if (boundaryCount === 1) {
    return { x: width / 2, y: height / 2 };
  }
  if (boundaryCount === 2) {
    return {
      x: width * (slot === 0 ? 0.36 : 0.68),
      y: height * (slot === 0 ? 0.52 : 0.46),
    };
  }
  const ring = Math.floor(slot / ANCHOR_SLOTS);
  const index = slot % ANCHOR_SLOTS;
  const radiusX = width * (0.34 + ring * 0.16);
  const radiusY = height * (0.32 + ring * 0.16);
  const angle = (Math.PI * 2 * index) / ANCHOR_SLOTS - Math.PI / 2;
  return {
    x: width / 2 + radiusX * Math.cos(angle),
    y: height / 2 + radiusY * Math.sin(angle),
  };
}

function nodeOrbitRadius(groupSize: number, ring: number): number {
  if (groupSize <= 1) return 0;
  const base =
    groupSize === 2
      ? 96
      : groupSize === 3
        ? 118
        : groupSize === 4
          ? 138
          : 158;
  return base + ring * 42;
}

function nodePosition(
  anchorX: number,
  anchorY: number,
  slot: number,
  groupSize: number,
) {
  if (groupSize <= 1) return { x: anchorX, y: anchorY };
  const ring = Math.floor(slot / NODE_SLOTS_PER_RING);
  const index = slot % NODE_SLOTS_PER_RING;
  const nodesInRing = Math.min(groupSize - ring * NODE_SLOTS_PER_RING, NODE_SLOTS_PER_RING);
  const radius = nodeOrbitRadius(groupSize, ring);
  // Radially distribute the nodes actually present in this ring. Reusing all
  // sixteen fixed slots packed every small group into the top arc, which made
  // the card-based renderer overlap even for a two-node network.
  const angle = (Math.PI * 2 * index) / nodesInRing - Math.PI / 2;
  return {
    x: anchorX + radius * Math.cos(angle),
    y: anchorY + radius * Math.sin(angle),
  };
}

/// Mirrors the domain layout: anchors sit on fixed slots, each node is placed
/// once around its first sorted network, and repeated refreshes are identical.
export function layoutTopology(
  snapshot: TopologySnapshot,
  width = 1200,
  height = 400,
): TopologyLayout {
  const nodeCountByNetwork = new Map<string, number>();
  for (const node of snapshot.nodes) {
    const networkId = node.networks[0]?.network_id;
    if (!networkId) continue;
    nodeCountByNetwork.set(
      networkId,
      (nodeCountByNetwork.get(networkId) ?? 0) + 1,
    );
  }
  const anchors = snapshot.boundaries.map((boundary, slot) => ({
    networkId: boundary.network_id,
    slot,
    ...anchorPosition(width, height, slot, snapshot.boundaries.length),
  }));
  const anchorByNetwork = new Map(anchors.map((anchor) => [anchor.networkId, anchor]));
  const nextSlot = new Map<string, number>();
  const placements: TopologyPlacement[] = [];
  for (const node of snapshot.nodes) {
    const anchor = node.networks
      .map((network) => anchorByNetwork.get(network.network_id))
      .find((candidate) => candidate !== undefined);
    if (!anchor) continue;
    const slot = nextSlot.get(anchor.networkId) ?? 0;
    nextSlot.set(anchor.networkId, slot + 1);
    placements.push({
      nodeId: node.node_id,
      anchorNetworkId: anchor.networkId,
      slot,
      ...nodePosition(
        anchor.x,
        anchor.y,
        slot,
        nodeCountByNetwork.get(anchor.networkId) ?? 1,
      ),
    });
  }
  return { width, height, anchors, placements };
}

export function activeConnections(
  snapshot: TopologySnapshot,
): TopologyConnection[] {
  return snapshot.connections.filter((connection) => connection.state === "active");
}

export type TopologyListNetwork = {
  networkId: string;
  name: string;
  role: MembershipRole;
  color: string;
  dash: string;
};

export type TopologyListRow = {
  nodeId: NodeId;
  name: string;
  isSelf: boolean;
  health: TopologyNodeHealth;
  healthEncoding: HealthEncoding;
  capabilities: CapabilityKind[];
  networks: TopologyListNetwork[];
  connections: Record<TopologyConnectionState, number>;
};

/// The traditional list is an equivalent view: same objects, same health,
/// same capability and network membership as the graph.
export function topologyListRows(
  snapshot: TopologySnapshot,
  mode: TopologyColorMode = "default",
  index: TopologyIndex = createTopologyIndex(snapshot),
): TopologyListRow[] {
  const visuals = networkVisuals(snapshot, mode);
  return snapshot.nodes.map((node) => {
    const connections = index.connectionCountsByNode.get(node.node_id) ?? connectionCounts();
    return {
      nodeId: node.node_id,
      name: node.display_name?.trim() || node.node_id,
      isSelf: node.is_self,
      health: node.health,
      healthEncoding: healthEncoding(node.health),
      capabilities: CAPABILITY_ORDER.filter((capability) =>
        (node.capabilities ?? []).includes(capability),
      ),
      networks: node.networks.map((network) => ({
        networkId: network.network_id,
        name:
          index.boundariesById.get(network.network_id)?.name ??
          network.network_id,
        role: network.role,
        color: visuals.get(network.network_id)?.color ?? "#6b7280",
        dash: visuals.get(network.network_id)?.dash ?? "6 4",
      })),
      connections,
    };
  });
}

export type LegendItem = {
  id: string;
  labelKey: string;
  color?: string;
  dash?: string;
  shape?: HealthShape;
  glyph?: string;
  line?: "solid" | "dashed" | "dotted";
};

export type LegendSection = {
  id: "networks" | "capabilities" | "health" | "connections";
  titleKey: string;
  items: LegendItem[];
};

/// Legend entries are generated from the same functions that draw the canvas,
/// so the legend cannot drift from the encoding.
export function topologyLegend(
  snapshot: TopologySnapshot,
  mode: TopologyColorMode = "default",
): LegendSection[] {
  const visuals = networkVisuals(snapshot, mode);
  return [
    {
      id: "networks",
      titleKey: "topology.legend.networks",
      items: snapshot.boundaries.map((boundary) => ({
        id: boundary.network_id,
        labelKey: "",
        color: visuals.get(boundary.network_id)?.color,
        dash: visuals.get(boundary.network_id)?.dash,
      })),
    },
    {
      id: "capabilities",
      titleKey: "topology.legend.capabilities",
      items: CAPABILITY_ORDER.map((capability) => ({
        id: capability,
        labelKey: `capability.${capability}`,
        color: capabilityColor(capability, mode),
      })),
    },
    {
      id: "health",
      titleKey: "topology.legend.health",
      items: HEALTH_ORDER.map((health) => {
        const encoding = healthEncoding(health);
        return {
          id: health,
          labelKey: `topology.health.${health}`,
          shape: encoding.shape,
          glyph: encoding.glyph,
        };
      }),
    },
    {
      id: "connections",
      titleKey: "topology.legend.connections",
      items: [
        {
          id: "active",
          labelKey: "topology.connection.active",
          line: "solid" as const,
        },
        {
          id: "idle",
          labelKey: "topology.connection.idle",
          line: "dashed" as const,
        },
        {
          id: "stale",
          labelKey: "topology.connection.stale",
          line: "dotted" as const,
        },
      ],
    },
  ];
}

export function hiddenMemberCount(
  snapshot: TopologySnapshot,
  networkId: string,
  index: TopologyIndex = createTopologyIndex(snapshot),
): number {
  return index.hiddenMembersByNetwork.get(networkId) ?? 0;
}

export function connectionDash(state: TopologyConnectionState): string | undefined {
  switch (state) {
    case "active":
      return undefined;
    case "idle":
      return "6 5";
    case "stale":
      return "2 5";
  }
}
