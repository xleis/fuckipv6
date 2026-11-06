import type {
  CapabilityKind,
  MembershipRole,
  NodeId,
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

const ANCHOR_SLOTS = 12;
const NODE_SLOTS_PER_RING = 16;

function anchorPosition(width: number, height: number, slot: number) {
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

function nodePosition(anchorX: number, anchorY: number, slot: number) {
  const ring = Math.floor(slot / NODE_SLOTS_PER_RING);
  const index = slot % NODE_SLOTS_PER_RING;
  const radius = 56 + ring * 30;
  const angle = (Math.PI * 2 * index) / NODE_SLOTS_PER_RING - Math.PI / 2;
  return {
    x: anchorX + radius * Math.cos(angle),
    y: anchorY + radius * Math.sin(angle),
  };
}

/// Mirrors the domain layout: anchors sit on fixed slots, each node is placed
/// once around its first sorted network, and repeated refreshes are identical.
export function layoutTopology(
  snapshot: TopologySnapshot,
  width = 720,
  height = 360,
): TopologyLayout {
  const anchors = snapshot.boundaries.map((boundary, slot) => ({
    networkId: boundary.network_id,
    slot,
    ...anchorPosition(width, height, slot),
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
      ...nodePosition(anchor.x, anchor.y, slot),
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
): TopologyListRow[] {
  const visuals = networkVisuals(snapshot, mode);
  const boundaryNames = new Map(
    snapshot.boundaries.map((boundary) => [boundary.network_id, boundary.name]),
  );
  return snapshot.nodes.map((node) => {
    const connections: Record<TopologyConnectionState, number> = {
      active: 0,
      idle: 0,
      stale: 0,
    };
    for (const connection of snapshot.connections) {
      if (connection.from === node.node_id || connection.to === node.node_id) {
        connections[connection.state] += 1;
      }
    }
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
        name: boundaryNames.get(network.network_id) ?? network.network_id,
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

export function hiddenMemberCount(snapshot: TopologySnapshot, networkId: string): number {
  return (snapshot.clusters ?? [])
    .filter((cluster) => cluster.network_id === networkId)
    .reduce((total, cluster) => total + cluster.hidden_member_count, 0);
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
