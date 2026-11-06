import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  useSyncExternalStore,
  type JSX,
  type KeyboardEvent as ReactKeyboardEvent,
  type MouseEvent as ReactMouseEvent,
  type PointerEvent as ReactPointerEvent,
} from "react";
import {
  LocateFixed,
  Minus,
  Pause,
  Play,
  Plus,
  RotateCcw,
  Search,
  ShieldCheck,
  X,
} from "lucide-react";
import type {
  CapabilityKind,
  MemberLifecycleAction,
  NetworkMember,
  TopologyConnection,
  TopologyNode,
  TopologySnapshot,
} from "../../../contracts/typescript/node-contract-v1";
import { label } from "../contract/invariants";
import { tr } from "../i18n";
import {
  CAPABILITY_ORDER,
  CONNECTION_ORDER,
  HEALTH_ORDER,
  activeConnections,
  capabilityColor,
  capabilitySegments,
  connectionDash,
  healthEncoding,
  hiddenMemberCount,
  layoutTopology,
  networkVisuals,
  topologyListRows,
  type HealthEncoding,
  type TopologyColorMode,
} from "../topology";
import {
  EMPTY_TOPOLOGY_FILTERS,
  filteredTopologySnapshot,
  type BatchMemberAction,
  type TopologyFilters,
} from "../topology-actions";

type Props = {
  topology: TopologySnapshot | null;
  locale: Locale;
  filters?: TopologyFilters;
  onFiltersChange?: (filters: TopologyFilters) => void;
  selectedNodeIds?: ReadonlySet<string>;
  onToggleNode?: (nodeId: string) => void;
  onOpenNode?: (nodeId: string) => void;
  networkId?: string | null;
  membersByNodeId?: ReadonlyMap<string, NetworkMember>;
  lifecycleActions?: (nodeId: string) => MemberLifecycleAction[];
  onPrepareLifecycle?: (nodeId: string, action: BatchMemberAction) => void;
  onClearSelection?: () => void;
  selectionNotice?: string;
};

type HoverState = {
  nodeId: string;
  x: number;
  y: number;
};

type NodeMenuState = {
  nodeId: string;
  x: number;
  y: number;
};

const NODE_RING_RADIUS = 17;
const MEMBERSHIP_ROLES = ["owner", "administrator", "member"] as const;
const NARROW_VIEWPORT_QUERY = "(max-width: 600px)";
const TOUCH_LONG_PRESS_MS = 450;
const TOUCH_MOVE_TOLERANCE_PX = 8;
const ZOOM_LEVELS = [0.75, 1, 1.25, 1.5, 2] as const;

type TopologyViewMode = "graph" | "list";

function subscribeNarrowViewport(onChange: () => void): () => void {
  if (typeof window.matchMedia !== "function") return () => {};
  const media = window.matchMedia(NARROW_VIEWPORT_QUERY);
  if (typeof media.addEventListener === "function") {
    media.addEventListener("change", onChange);
    return () => media.removeEventListener("change", onChange);
  }
  media.addListener(onChange);
  return () => media.removeListener(onChange);
}

function narrowViewportSnapshot(): boolean {
  return typeof window.matchMedia === "function"
    ? window.matchMedia(NARROW_VIEWPORT_QUERY).matches
    : false;
}

function useNarrowViewport(): boolean {
  return useSyncExternalStore(
    subscribeNarrowViewport,
    narrowViewportSnapshot,
    () => false,
  );
}

function useTouchLongPress(
  onLongPress: (nodeId: string, x: number, y: number) => void,
) {
  const timerRef = useRef<number | null>(null);
  const originRef = useRef<{
    nodeId: string;
    x: number;
    y: number;
  } | null>(null);
  const suppressedNodeRef = useRef<string | null>(null);

  const cancel = useCallback(function cancelLongPress(): void {
    if (timerRef.current !== null) window.clearTimeout(timerRef.current);
    timerRef.current = null;
    originRef.current = null;
    document.removeEventListener("scroll", cancelLongPress, true);
  }, []);

  const start = useCallback(
    (event: ReactPointerEvent<Element>, nodeId: string): void => {
      if (event.pointerType === "mouse" || event.button !== 0) return;
      cancel();
      const target = event.currentTarget;
      originRef.current = { nodeId, x: event.clientX, y: event.clientY };
      document.addEventListener("scroll", cancel, true);
      timerRef.current = window.setTimeout(() => {
        const origin = originRef.current;
        if (!origin || origin.nodeId !== nodeId) return;
        suppressedNodeRef.current = nodeId;
        (target as HTMLElement).focus({ preventScroll: true });
        onLongPress(nodeId, origin.x, origin.y);
        cancel();
      }, TOUCH_LONG_PRESS_MS);
    },
    [cancel, onLongPress],
  );

  const move = useCallback(
    (event: ReactPointerEvent<Element>): void => {
      const origin = originRef.current;
      if (!origin) return;
      if (
        Math.hypot(event.clientX - origin.x, event.clientY - origin.y) >
        TOUCH_MOVE_TOLERANCE_PX
      ) {
        cancel();
      }
    },
    [cancel],
  );

  const consumeActivation = useCallback((nodeId: string): boolean => {
    if (suppressedNodeRef.current !== nodeId) return false;
    suppressedNodeRef.current = null;
    return true;
  }, []);

  useEffect(() => () => cancel(), [cancel]);

  return { start, move, end: cancel, consumeActivation };
}

function polar(cx: number, cy: number, radius: number, angleDegrees: number) {
  const angle = (angleDegrees * Math.PI) / 180;
  return { x: cx + radius * Math.cos(angle), y: cy + radius * Math.sin(angle) };
}

function arcPath(
  cx: number,
  cy: number,
  radius: number,
  startAngle: number,
  endAngle: number,
): string {
  const start = polar(cx, cy, radius, startAngle);
  const end = polar(cx, cy, radius, endAngle);
  const largeArc = endAngle - startAngle > 180 ? 1 : 0;
  return `M ${start.x} ${start.y} A ${radius} ${radius} 0 ${largeArc} 1 ${end.x} ${end.y}`;
}

function nodeName(node: TopologyNode): string {
  return node.display_name?.trim() || node.node_id;
}

function membershipFor(node: TopologyNode, networkId: string | null | undefined) {
  return (
    node.networks.find((network) => network.network_id === networkId) ??
    node.networks[0]
  );
}

function nodeRoleText(
  locale: Locale,
  node: TopologyNode,
  networkId: string | null | undefined,
): string {
  const membership = membershipFor(node, networkId);
  return membership
    ? label(locale, `join_request.role.${membership.role}`)
    : label(locale, "common.not_available");
}

function nodeCapabilityText(
  locale: Locale,
  node: TopologyNode,
  member: NetworkMember | undefined,
): string {
  const capabilities = node.capabilities ?? member?.capabilities;
  if (capabilities == null) {
    return label(locale, "member_directory.capabilities_unknown");
  }
  if (capabilities.length === 0) return label(locale, "common.none");
  return capabilities
    .map((capability) => label(locale, `capability.${capability}`))
    .join(label(locale, "common.list_separator"));
}

function healthLabel(locale: Locale, health: TopologyNode["health"]): string {
  return label(locale, `topology.health.${health}`);
}

function nodeRelationshipText(
  locale: Locale,
  nodeId: string,
  topology: TopologySnapshot,
): string {
  const counts = new Map<
    (typeof CONNECTION_ORDER)[number],
    number
  >(CONNECTION_ORDER.map((state) => [state, 0]));
  for (const connection of topology.connections) {
    if (connection.from !== nodeId && connection.to !== nodeId) continue;
    counts.set(connection.state, (counts.get(connection.state) ?? 0) + 1);
  }
  const summary = CONNECTION_ORDER.filter((state) => (counts.get(state) ?? 0) > 0)
    .map(
      (state) =>
        `${label(locale, `topology.connection.${state}`)} ${counts.get(state)}`,
    )
    .join(label(locale, "common.list_separator"));
  return summary || label(locale, "topology.aria.no_connections");
}

function nodeAriaLabel(
  locale: Locale,
  node: TopologyNode,
  networkNames: Map<string, string>,
  topology: TopologySnapshot,
): string {
  const capabilities = nodeCapabilityText(locale, node, undefined);
  const networks = node.networks
    .map(
      (network) =>
        `${networkNames.get(network.network_id) ?? network.network_id} (${label(
          locale,
          `join_request.role.${network.role}`,
        )})`,
    )
    .join(label(locale, "common.list_separator"));
  const relationships = nodeRelationshipText(locale, node.node_id, topology);
  return `${nodeName(node)} · ${healthLabel(locale, node.health)} · ${capabilities} · ${networks} · ${label(locale, "topology.aria.relationships")}: ${relationships}`;
}

function rectPosition(rect: DOMRect): { x: number; y: number } {
  return {
    x: rect.left + Math.max(8, rect.width / 2),
    y: rect.top + Math.max(8, rect.height / 2),
  };
}

function clampOverlayPosition(
  x: number,
  y: number,
  width: number,
  height: number,
): { x: number; y: number } {
  const margin = 12;
  const viewportWidth = window.innerWidth || 1024;
  const viewportHeight = window.innerHeight || 768;
  return {
    x: Math.max(margin, Math.min(x, viewportWidth - width - margin)),
    y: Math.max(margin, Math.min(y, viewportHeight - height - margin)),
  };
}

function HealthBadge({
  encoding,
  x,
  y,
}: {
  encoding: HealthEncoding;
  x: number;
  y: number;
}) {
  const common = {
    className: `topology-health-badge ${encoding.tone}`,
    "aria-hidden": true,
  } as const;
  return (
    <g transform={`translate(${x} ${y})`} {...common}>
      {encoding.shape === "circle" && <circle r="7" />}
      {encoding.shape === "square" && (
        <rect x="-6" y="-6" width="12" height="12" rx="2" />
      )}
      {encoding.shape === "diamond" && (
        <path d="M 0 -7 L 7 0 L 0 7 L -7 0 Z" />
      )}
      {encoding.shape === "triangle" && <path d="M 0 -7 L 7 6 L -7 6 Z" />}
      <text x="0" y="0.5" textAnchor="middle" dominantBaseline="middle">
        {encoding.glyph}
      </text>
    </g>
  );
}

function HealthBadgeInline({
  health,
}: {
  health: TopologyNode["health"];
}): JSX.Element {
  const encoding = healthEncoding(health);
  return (
    <span className={`topology-health-inline ${encoding.tone}`}>
      <svg viewBox="-8 -8 16 16" aria-hidden="true">
        {encoding.shape === "circle" && <circle r="7" />}
        {encoding.shape === "square" && (
          <rect x="-6" y="-6" width="12" height="12" rx="2" />
        )}
        {encoding.shape === "diamond" && (
          <path d="M 0 -7 L 7 0 L 0 7 L -7 0 Z" />
        )}
        {encoding.shape === "triangle" && <path d="M 0 -7 L 7 6 L -7 6 Z" />}
        <text x="0" y="0.5" textAnchor="middle" dominantBaseline="middle">
          {encoding.glyph}
        </text>
      </svg>
    </span>
  );
}

function NodeHoverCard({
  node,
  member,
  locale,
  networkId,
  x,
  y,
}: {
  node: TopologyNode;
  member: NetworkMember | undefined;
  locale: Locale;
  networkId: string | null;
  x: number;
  y: number;
}) {
  const membership = membershipFor(node, networkId);
  const presence = membership?.presence ?? member?.presence;
  return (
    <section
      className="topology-node-hover"
      role="tooltip"
      data-testid="topology-node-hover"
      style={{ left: x, top: y }}
    >
      <div className="topology-node-hover-heading">
        <strong>{nodeName(node)}</strong>
        <span className={`status-pill ${healthEncoding(node.health).tone}`}>
          {healthLabel(locale, node.health)}
        </span>
      </div>
      <dl>
        <div>
          <dt>{label(locale, "topology.hover.role")}</dt>
          <dd>{nodeRoleText(locale, node, networkId)}</dd>
        </div>
        <div>
          <dt>{label(locale, "topology.hover.capabilities")}</dt>
          <dd>{nodeCapabilityText(locale, node, member)}</dd>
        </div>
        <div>
          <dt>{label(locale, "topology.hover.health")}</dt>
          <dd>
            {healthLabel(locale, node.health)}
            {presence
              ? ` · ${label(locale, `member_directory.presence.${presence.state}`)}`
              : ""}
          </dd>
        </div>
      </dl>
      <p>{label(locale, "topology.hover.hint")}</p>
    </section>
  );
}

function NodeActionMenu({
  node,
  locale,
  selected,
  lifecycleActions,
  x,
  y,
  menuRef,
  onOpen,
  onToggle,
  onPrepareLifecycle,
  onClose,
}: {
  node: TopologyNode;
  locale: Locale;
  selected: boolean;
  lifecycleActions: MemberLifecycleAction[];
  x: number;
  y: number;
  menuRef: React.RefObject<HTMLDivElement | null>;
  onOpen(nodeId: string): void;
  onToggle(nodeId: string): void;
  onPrepareLifecycle(nodeId: string, action: BatchMemberAction): void;
  onClose(restoreFocus?: boolean): void;
}) {
  function menuItems(menu: HTMLDivElement): HTMLButtonElement[] {
    return [
      ...menu.querySelectorAll<HTMLButtonElement>(
        'button[role="menuitem"]:not([aria-disabled="true"])',
      ),
    ];
  }

  function handleMenuKey(event: ReactKeyboardEvent<HTMLDivElement>): void {
    if (!["ArrowDown", "ArrowUp", "Home", "End"].includes(event.key)) return;
    const items = menuItems(event.currentTarget);
    if (items.length === 0) return;
    event.preventDefault();
    const currentIndex = items.indexOf(document.activeElement as HTMLButtonElement);
    let nextIndex = 0;
    if (event.key === "End") nextIndex = items.length - 1;
    else if (event.key === "ArrowUp") {
      nextIndex = currentIndex <= 0 ? items.length - 1 : currentIndex - 1;
    } else if (event.key === "ArrowDown") {
      nextIndex = currentIndex < 0 || currentIndex === items.length - 1
        ? 0
        : currentIndex + 1;
    }
    items[nextIndex]?.focus();
  }

  return (
    <div
      ref={menuRef}
      className="topology-node-menu"
      role="menu"
      aria-orientation="vertical"
      aria-label={tr(locale, "topology.menu.label", { name: nodeName(node) })}
      style={{ left: x, top: y }}
      data-testid="topology-node-menu"
      tabIndex={-1}
      onKeyDown={handleMenuKey}
      onContextMenu={(event) => event.preventDefault()}
    >
      <div className="topology-node-menu-heading">
        <strong>{nodeName(node)}</strong>
        <code>{node.node_id}</code>
      </div>
      <button
        type="button"
        role="menuitem"
        onClick={() => {
          onClose(false);
          onOpen(node.node_id);
        }}
      >
        {label(locale, "topology.menu.open")}
      </button>
      <button
        type="button"
        role="menuitem"
        onClick={() => {
          onClose(false);
          onToggle(node.node_id);
        }}
      >
        {label(
          locale,
          selected ? "topology.menu.unselect" : "topology.menu.select",
        )}
      </button>
      <div className="topology-node-menu-separator" role="separator" />
      <div className="topology-node-menu-label">
        <ShieldCheck size={13} />
        {label(locale, "topology.menu.govern")}
      </div>
      {lifecycleActions.length === 0 ? (
        <div
          className="topology-node-menu-unavailable"
          role="menuitem"
          aria-disabled="true"
        >
          {label(locale, "topology.menu.no_actions")}
        </div>
      ) : (
        lifecycleActions.map((action) => (
          <button
            key={action}
            type="button"
            role="menuitem"
            onClick={() => {
              onClose(false);
              onPrepareLifecycle(node.node_id, action as BatchMemberAction);
            }}
          >
            {label(locale, `choice.member_lifecycle.${action}`)}
          </button>
        ))
      )}
    </div>
  );
}

function FilterToolbar({
  topology,
  locale,
  filters,
  visibleCount,
  onChange,
}: {
  topology: TopologySnapshot;
  locale: Locale;
  filters: TopologyFilters;
  visibleCount: number;
  onChange(filters: TopologyFilters): void;
}) {
  const filtered =
    filters.search.trim() !== "" ||
    filters.networkId !== "all" ||
    filters.role !== "all" ||
    filters.capability !== "all" ||
    filters.health !== "all";
  return (
    <section
      className="topology-filters"
      aria-label={label(locale, "topology.filters.label")}
    >
      <label className="topology-filter-search">
        <Search size={15} aria-hidden="true" />
        <span className="sr-only">{label(locale, "topology.filters.search")}</span>
        <input
          type="search"
          value={filters.search}
          aria-controls="topology-object-navigation"
          placeholder={label(locale, "topology.filters.search_placeholder")}
          onChange={(event) =>
            onChange({ ...filters, search: event.currentTarget.value })
          }
        />
      </label>
      <label>
        <span>{label(locale, "topology.filters.network")}</span>
        <select
          value={filters.networkId}
          onChange={(event) =>
            onChange({ ...filters, networkId: event.currentTarget.value })
          }
        >
          <option value="all">{label(locale, "topology.filters.all")}</option>
          {topology.boundaries.map((boundary) => (
            <option key={boundary.network_id} value={boundary.network_id}>
              {boundary.name}
            </option>
          ))}
        </select>
      </label>
      <label>
        <span>{label(locale, "topology.filters.role")}</span>
        <select
          value={filters.role}
          onChange={(event) =>
            onChange({ ...filters, role: event.currentTarget.value })
          }
        >
          <option value="all">{label(locale, "topology.filters.all")}</option>
          {MEMBERSHIP_ROLES.map((role) => (
            <option key={role} value={role}>
              {label(locale, `join_request.role.${role}`)}
            </option>
          ))}
        </select>
      </label>
      <label>
        <span>{label(locale, "topology.filters.capability")}</span>
        <select
          value={filters.capability}
          onChange={(event) =>
            onChange({ ...filters, capability: event.currentTarget.value })
          }
        >
          <option value="all">{label(locale, "topology.filters.all")}</option>
          {CAPABILITY_ORDER.map((capability) => (
            <option key={capability} value={capability}>
              {label(locale, `capability.${capability}`)}
            </option>
          ))}
        </select>
      </label>
      <label>
        <span>{label(locale, "topology.filters.health")}</span>
        <select
          value={filters.health}
          onChange={(event) =>
            onChange({
              ...filters,
              health: event.currentTarget.value as TopologyFilters["health"],
            })
          }
        >
          <option value="all">{label(locale, "topology.filters.all")}</option>
          {HEALTH_ORDER.map((health) => (
            <option key={health} value={health}>
              {healthLabel(locale, health)}
            </option>
          ))}
        </select>
      </label>
      <span className="topology-filter-count" role="status" aria-live="polite">
        {tr(locale, "topology.filters.count", {
          visible: String(visibleCount),
          total: String(topology.nodes.length),
        })}
      </span>
      <button
        type="button"
        className="secondary-button topology-filter-clear"
        disabled={!filtered}
        onClick={() => onChange({ ...EMPTY_TOPOLOGY_FILTERS })}
      >
        <X size={14} />
        {label(locale, "topology.filters.clear")}
      </button>
    </section>
  );
}

function GraphView({
  topology,
  locale,
  mode,
  zoom,
  focusRequest,
  focusTargetNodeId,
  selectedNodeIds,
  onOpenNode,
  onToggleNode,
  onShowHover,
  onHideHover,
  onShowMenu,
}: {
  topology: TopologySnapshot;
  locale: Locale;
  mode: TopologyColorMode;
  zoom: number;
  focusRequest: number;
  focusTargetNodeId: string | null;
  selectedNodeIds: ReadonlySet<string>;
  onOpenNode?: (nodeId: string) => void;
  onToggleNode?: (nodeId: string) => void;
  onShowHover(nodeId: string, rect: DOMRect): void;
  onHideHover(): void;
  onShowMenu(nodeId: string, x: number, y: number): void;
}): JSX.Element {
  const layout = useMemo(() => layoutTopology(topology), [topology]);
  const visuals = useMemo(() => networkVisuals(topology, mode), [topology, mode]);
  const placements = useMemo(
    () => new Map(layout.placements.map((placement) => [placement.nodeId, placement])),
    [layout],
  );
  const nodeById = useMemo(
    () => new Map(topology.nodes.map((node) => [node.node_id, node])),
    [topology],
  );
  const boundaryNames = useMemo(
    () =>
      new Map(
        topology.boundaries.map((boundary) => [boundary.network_id, boundary.name]),
      ),
    [topology],
  );
  const connections = topology.connections.filter((connection) =>
    placements.has(connection.from) && placements.has(connection.to),
  );
  const nodeRefs = useRef(new Map<string, SVGGElement>());
  const focusedNodeIdRef = useRef<string | null>(null);
  const [activeNodeId, setActiveNodeId] = useState<string | null>(
    () =>
      topology.nodes.find((node) => node.is_self)?.node_id ??
      topology.nodes[0]?.node_id ??
      null,
  );
  const fallbackNodeId =
    topology.nodes.find((node) => node.is_self)?.node_id ??
    topology.nodes[0]?.node_id ??
    null;
  const resolvedActiveNodeId = topology.nodes.some(
    (node) => node.node_id === activeNodeId,
  )
    ? activeNodeId
    : fallbackNodeId;
  const longPress = useTouchLongPress(onShowMenu);
  const focusTargetNodeRef = useRef(focusTargetNodeId);

  useEffect(() => {
    focusTargetNodeRef.current = focusTargetNodeId;
  }, [focusTargetNodeId]);

  useEffect(() => {
    const focusedNodeId = focusedNodeIdRef.current;
    const activeElement = document.activeElement;
    const focusLostToDocument =
      activeElement === document.body ||
      activeElement == null ||
      !activeElement.isConnected;
    if (
      !focusedNodeId ||
      !focusLostToDocument ||
      topology.nodes.some((node) => node.node_id === focusedNodeId)
    ) {
      return;
    }
    focusedNodeIdRef.current = resolvedActiveNodeId;
    const fallback = resolvedActiveNodeId
      ? nodeRefs.current.get(resolvedActiveNodeId)
      : undefined;
    fallback?.focus();
    fallback?.scrollIntoView?.({ block: "center", inline: "center" });
  }, [resolvedActiveNodeId, topology.nodes]);

  useEffect(() => {
    if (focusRequest === 0) return;
    const nodeId = focusTargetNodeRef.current;
    const element = nodeId ? nodeRefs.current.get(nodeId) : undefined;
    if (!nodeId || !element) return;
    setActiveNodeId(nodeId);
    focusedNodeIdRef.current = nodeId;
    element.focus({ preventScroll: true });
    element.scrollIntoView?.({ block: "center", inline: "center" });
  }, [focusRequest]);

  function navigateNode(
    event: ReactKeyboardEvent<SVGGElement>,
    nodeId: string,
  ): void {
    if (!["ArrowLeft", "ArrowRight", "ArrowUp", "ArrowDown", "Home", "End"].includes(event.key)) {
      return;
    }
    event.preventDefault();
    const currentIndex = topology.nodes.findIndex(
      (node) => node.node_id === nodeId,
    );
    let nextIndex = 0;
    if (event.key === "End") nextIndex = topology.nodes.length - 1;
    else if (event.key === "ArrowRight" || event.key === "ArrowDown") {
      nextIndex = (currentIndex + 1) % topology.nodes.length;
    } else if (event.key === "ArrowLeft" || event.key === "ArrowUp") {
      nextIndex =
        (currentIndex - 1 + topology.nodes.length) % topology.nodes.length;
    }
    const nextNodeId = topology.nodes[nextIndex]?.node_id;
    if (!nextNodeId) return;
    setActiveNodeId(nextNodeId);
    focusedNodeIdRef.current = nextNodeId;
    nodeRefs.current.get(nextNodeId)?.focus();
  }

  function menuFromFocus(
    event: ReactKeyboardEvent<SVGGElement>,
    nodeId: string,
  ): void {
    if (event.key !== "ContextMenu" && !(event.shiftKey && event.key === "F10")) {
      return;
    }
    event.preventDefault();
    const position = rectPosition(event.currentTarget.getBoundingClientRect());
    onShowMenu(nodeId, position.x, position.y);
  }

  return (
    <div className="topology-canvas" id="topology-object-navigation">
      <svg
        viewBox={`0 0 ${layout.width} ${layout.height}`}
        role="group"
        aria-labelledby="topology-svg-title"
        aria-describedby="topology-svg-desc"
        aria-keyshortcuts="ArrowLeft ArrowRight ArrowUp ArrowDown Home End Enter Space Shift+F10 ContextMenu"
        data-color-mode={mode}
        data-zoom={zoom}
        style={{ width: `${zoom * 100}%`, minWidth: `${620 * zoom}px` }}
      >
        <title id="topology-svg-title">
          {label(locale, "topology.aria.graph_title")}
        </title>
        <desc id="topology-svg-desc">
          {label(locale, "topology.aria.graph_description")
            .replace("{networks}", String(topology.boundaries.length))
            .replace("{nodes}", String(topology.nodes.length))
            .replace("{links}", String(activeConnections(topology).length))}
        </desc>
        {layout.anchors.map((anchor) => {
          const boundary = topology.boundaries.find(
            (candidate) => candidate.network_id === anchor.networkId,
          );
          const visual = visuals.get(anchor.networkId);
          if (!boundary || !visual) return null;
          const memberSlots = layout.placements.filter(
            (placement) => placement.anchorNetworkId === anchor.networkId,
          );
          const ring = memberSlots.reduce(
            (maximum, placement) => Math.max(maximum, placement.slot),
            0,
          );
          const ringCount = Math.floor(ring / 16);
          const rx = 64 + ringCount * 30 + 22;
          const ry = 58 + ringCount * 30 + 18;
          const hidden = hiddenMemberCount(topology, anchor.networkId);
          return (
            <g key={`boundary-${anchor.networkId}`} className="topology-boundary">
              <ellipse
                cx={anchor.x}
                cy={anchor.y}
                rx={rx}
                ry={ry}
                fill="none"
                stroke={visual.color}
                strokeWidth={1.5}
                strokeDasharray={visual.dash}
              />
              <title>
                {boundary.name} · {tr(locale, "topology.boundary.members", {
                  count: boundary.member_count,
                })}
                {hidden > 0
                  ? ` · ${label(locale, "topology.cluster.hidden").replace("{count}", String(hidden))}`
                  : ""}
              </title>
              <text
                x={anchor.x}
                y={anchor.y - ry - 6}
                textAnchor="middle"
                className="topology-boundary-label"
                fill={visual.color}
              >
                {boundary.name}
              </text>
            </g>
          );
        })}
        {connections.map((connection) => (
          <ConnectionLine
            key={connection.connection_id}
            connection={connection}
            placements={placements}
            locale={locale}
            nodeById={nodeById}
            boundaryNames={boundaryNames}
          />
        ))}
        <g
          role="listbox"
          aria-label={label(locale, "topology.aria.nodes")}
          aria-multiselectable="true"
          aria-keyshortcuts="ArrowLeft ArrowRight ArrowUp ArrowDown Home End"
        >
          {topology.nodes.map((node, index) => {
            const placement = placements.get(node.node_id);
            if (!placement) return null;
            const visual = visuals.get(placement.anchorNetworkId);
            return (
              <TopologyNodeGlyph
                key={node.node_id}
                itemIndex={index}
                itemCount={topology.nodes.length}
                node={node}
                x={placement.x}
                y={placement.y}
                mode={mode}
                locale={locale}
                networkNames={boundaryNames}
                topology={topology}
                networkColor={visual?.color ?? "#6b7280"}
                selected={selectedNodeIds.has(node.node_id)}
                tabIndex={resolvedActiveNodeId === node.node_id ? 0 : -1}
                onOpenNode={onOpenNode}
                onToggleNode={onToggleNode}
                onShowHover={onShowHover}
                onHideHover={onHideHover}
                onShowMenu={onShowMenu}
                onMenuKey={menuFromFocus}
                onNavigate={navigateNode}
                onFocusNode={(nodeId) => {
                  setActiveNodeId(nodeId);
                  focusedNodeIdRef.current = nodeId;
                }}
                onLongPressStart={(event, nodeId) =>
                  longPress.start(event, nodeId)
                }
                onLongPressMove={longPress.move}
                onLongPressEnd={longPress.end}
                consumeLongPress={longPress.consumeActivation}
                nodeRef={(element) => {
                  if (element) nodeRefs.current.set(node.node_id, element);
                  else nodeRefs.current.delete(node.node_id);
                }}
              />
            );
          })}
        </g>
        {(topology.clusters ?? []).map((cluster) => {
          const anchor = layout.anchors.find(
            (candidate) => candidate.networkId === cluster.network_id,
          );
          if (!anchor) return null;
          return (
            <g
              key={`cluster-${cluster.network_id}`}
              transform={`translate(${anchor.x} ${anchor.y})`}
              className="topology-cluster"
            >
              <title>
                {label(locale, "topology.cluster.hidden").replace(
                  "{count}",
                  String(cluster.hidden_member_count),
                )}
              </title>
              <rect x="-30" y="6" width="60" height="18" rx="9" />
              <text x="0" y="19" textAnchor="middle">
                +{cluster.hidden_member_count}
              </text>
            </g>
          );
        })}
      </svg>
    </div>
  );
}

function ConnectionLine({
  connection,
  placements,
  locale,
  nodeById,
  boundaryNames,
}: {
  connection: TopologyConnection;
  placements: Map<string, { x: number; y: number }>;
  locale: Locale;
  nodeById: Map<string, TopologyNode>;
  boundaryNames: Map<string, string>;
}): JSX.Element | null {
  const from = placements.get(connection.from);
  const to = placements.get(connection.to);
  if (!from || !to) return null;
  const stateLabel = label(locale, `topology.connection.${connection.state}`);
  const networks = (connection.networks ?? [])
    .map((networkId) => boundaryNames.get(networkId) ?? networkId)
    .join(", ");
  return (
    <g className={`topology-connection ${connection.state}`}>
      <title>
        {`${nodeById.get(connection.from)?.display_name ?? connection.from} → ${
          nodeById.get(connection.to)?.display_name ?? connection.to
        } · ${stateLabel}${networks ? ` · ${networks}` : ""}`}
      </title>
      <line
        x1={from.x}
        y1={from.y}
        x2={to.x}
        y2={to.y}
        strokeDasharray={connectionDash(connection.state)}
      />
    </g>
  );
}

function TopologyNodeGlyph({
  itemIndex,
  itemCount,
  node,
  x,
  y,
  mode,
  locale,
  networkNames,
  topology,
  networkColor,
  selected,
  tabIndex,
  onOpenNode,
  onToggleNode,
  onShowHover,
  onHideHover,
  onShowMenu,
  onMenuKey,
  onNavigate,
  onFocusNode,
  onLongPressStart,
  onLongPressMove,
  onLongPressEnd,
  consumeLongPress,
  nodeRef,
}: {
  itemIndex: number;
  itemCount: number;
  node: TopologyNode;
  x: number;
  y: number;
  mode: TopologyColorMode;
  locale: Locale;
  networkNames: Map<string, string>;
  topology: TopologySnapshot;
  networkColor: string;
  selected: boolean;
  tabIndex: number;
  onOpenNode?: (nodeId: string) => void;
  onToggleNode?: (nodeId: string) => void;
  onShowHover(nodeId: string, rect: DOMRect): void;
  onHideHover(): void;
  onShowMenu(nodeId: string, x: number, y: number): void;
  onMenuKey(event: ReactKeyboardEvent<SVGGElement>, nodeId: string): void;
  onNavigate(event: ReactKeyboardEvent<SVGGElement>, nodeId: string): void;
  onFocusNode(nodeId: string): void;
  onLongPressStart(
    event: ReactPointerEvent<SVGGElement>,
    nodeId: string,
  ): void;
  onLongPressMove(event: ReactPointerEvent<SVGGElement>): void;
  onLongPressEnd(): void;
  consumeLongPress(nodeId: string): boolean;
  nodeRef(element: SVGGElement | null): void;
}): JSX.Element {
  const segments = capabilitySegments(node, mode);
  const health = healthEncoding(node.health);
  const shared = node.networks.length > 1;

  function openOrToggle(event: ReactMouseEvent<SVGGElement>): void {
    if (consumeLongPress(node.node_id)) return;
    if (event.ctrlKey || event.metaKey || event.shiftKey) {
      event.preventDefault();
      onToggleNode?.(node.node_id);
      return;
    }
    onOpenNode?.(node.node_id);
  }

  function onKeyDown(event: ReactKeyboardEvent<SVGGElement>): void {
    if (
      ["ArrowLeft", "ArrowRight", "ArrowUp", "ArrowDown", "Home", "End"].includes(
        event.key,
      )
    ) {
      onNavigate(event, node.node_id);
      return;
    }
    if (event.key === "Enter") {
      event.preventDefault();
      onOpenNode?.(node.node_id);
      return;
    }
    if (event.key === " ") {
      event.preventDefault();
      onToggleNode?.(node.node_id);
      return;
    }
    onMenuKey(event, node.node_id);
  }

  return (
    <g
      ref={nodeRef}
      transform={`translate(${x} ${y})`}
      className={`topology-node ${node.is_self ? "self" : ""} ${node.health} ${selected ? "selected" : ""}`}
      data-node-id={node.node_id}
      data-selected={selected ? "true" : "false"}
      tabIndex={tabIndex}
      role="option"
      aria-selected={selected}
      aria-posinset={itemIndex + 1}
      aria-setsize={itemCount}
      aria-keyshortcuts="Enter Space Shift+F10 ContextMenu"
      aria-label={nodeAriaLabel(locale, node, networkNames, topology)}
      onClick={openOrToggle}
      onKeyDown={onKeyDown}
      onMouseEnter={(event) =>
        onShowHover(node.node_id, event.currentTarget.getBoundingClientRect())
      }
      onMouseLeave={onHideHover}
      onFocus={(event) => {
        onFocusNode(node.node_id);
        onShowHover(node.node_id, event.currentTarget.getBoundingClientRect());
      }}
      onBlur={onHideHover}
      onPointerDown={(event) => onLongPressStart(event, node.node_id)}
      onPointerMove={onLongPressMove}
      onPointerUp={onLongPressEnd}
      onPointerCancel={onLongPressEnd}
      onPointerLeave={onLongPressEnd}
      onContextMenu={(event) => {
        event.preventDefault();
        onShowMenu(node.node_id, event.clientX, event.clientY);
      }}
    >
      <title>{nodeAriaLabel(locale, node, networkNames, topology)}</title>
      <circle r="24" className="topology-node-halo" stroke={networkColor} />
      {selected ? (
        <g className="topology-node-selection-mark" transform="translate(-17 17)">
          <circle r="7" />
          <text x="0" y="2.4" textAnchor="middle">
            ✓
          </text>
        </g>
      ) : null}
      {segments.map((segment) => (
        <path
          key={segment.capability}
          d={arcPath(0, 0, NODE_RING_RADIUS, segment.startAngle, segment.endAngle)}
          fill="none"
          stroke={segment.enabled ? segment.color : "var(--border)"}
          strokeWidth={segment.enabled ? 4 : 2}
          strokeDasharray={segment.enabled ? undefined : "2 2"}
          strokeLinecap="round"
          className={segment.enabled ? "capability-enabled" : "capability-disabled"}
        />
      ))}
      <circle r="11" className={`topology-node-core ${health.tone}`} />
      {node.is_self && (
        <text x="0" y="2.5" textAnchor="middle" className="topology-node-self">
          ●
        </text>
      )}
      <HealthBadge encoding={health} x={14} y={-14} />
      {shared && (
        <g className="topology-node-shared" transform="translate(-15 -14)">
          <title>
            {label(locale, "topology.node.networks_count").replace(
              "{count}",
              String(node.networks.length),
            )}
          </title>
          <circle r="7" />
          <text x="0" y="2.5" textAnchor="middle">
            ×{node.networks.length}
          </text>
        </g>
      )}
      <text x="0" y="38" textAnchor="middle" className="topology-node-label">
        {node.display_name?.trim() || node.node_id.slice(0, 12)}
      </text>
    </g>
  );
}

function ListView({
  topology,
  locale,
  mode,
  focusRequest,
  focusTargetNodeId,
  selectedNodeIds,
  onOpenNode,
  onToggleNode,
  onShowHover,
  onHideHover,
  onShowMenu,
}: {
  topology: TopologySnapshot;
  locale: Locale;
  mode: TopologyColorMode;
  focusRequest: number;
  focusTargetNodeId: string | null;
  selectedNodeIds: ReadonlySet<string>;
  onOpenNode?: (nodeId: string) => void;
  onToggleNode?: (nodeId: string) => void;
  onShowHover(nodeId: string, rect: DOMRect): void;
  onHideHover(): void;
  onShowMenu(nodeId: string, x: number, y: number): void;
}): JSX.Element {
  const rows = useMemo(() => topologyListRows(topology, mode), [topology, mode]);
  const nodes = useMemo(
    () => new Map(topology.nodes.map((node) => [node.node_id, node])),
    [topology],
  );
  const networkNames = useMemo(
    () =>
      new Map(
        topology.boundaries.map((boundary) => [
          boundary.network_id,
          boundary.name,
        ]),
      ),
    [topology.boundaries],
  );
  const rowRefs = useRef(new Map<string, HTMLTableRowElement>());
  const focusedRowIdRef = useRef<string | null>(null);
  const [activeRowId, setActiveRowId] = useState<string | null>(
    () => rows[0]?.nodeId ?? null,
  );
  const resolvedActiveRowId = rows.some((row) => row.nodeId === activeRowId)
    ? activeRowId
    : rows[0]?.nodeId ?? null;
  const longPress = useTouchLongPress(onShowMenu);
  const focusTargetNodeRef = useRef(focusTargetNodeId);

  useEffect(() => {
    focusTargetNodeRef.current = focusTargetNodeId;
  }, [focusTargetNodeId]);

  useEffect(() => {
    const focusedRowId = focusedRowIdRef.current;
    const activeElement = document.activeElement;
    const focusLostToDocument =
      activeElement === document.body ||
      activeElement == null ||
      !activeElement.isConnected;
    if (
      !focusedRowId ||
      !focusLostToDocument ||
      rows.some((row) => row.nodeId === focusedRowId)
    ) {
      return;
    }
    focusedRowIdRef.current = resolvedActiveRowId;
    const fallback = resolvedActiveRowId
      ? rowRefs.current.get(resolvedActiveRowId)
      : undefined;
    fallback?.focus();
    fallback?.scrollIntoView?.({ block: "center", inline: "center" });
  }, [resolvedActiveRowId, rows]);

  useEffect(() => {
    if (focusRequest === 0) return;
    const nodeId = focusTargetNodeRef.current;
    const element = nodeId ? rowRefs.current.get(nodeId) : undefined;
    if (!nodeId || !element) return;
    setActiveRowId(nodeId);
    focusedRowIdRef.current = nodeId;
    element.focus({ preventScroll: true });
    element.scrollIntoView?.({ block: "center", inline: "center" });
  }, [focusRequest]);

  function handleRowKey(
    event: ReactKeyboardEvent<HTMLTableRowElement>,
    nodeId: string,
  ): void {
    if (event.target !== event.currentTarget) return;
    if (
      ["ArrowLeft", "ArrowRight", "ArrowUp", "ArrowDown", "Home", "End"].includes(
        event.key,
      )
    ) {
      event.preventDefault();
      const currentIndex = rows.findIndex((row) => row.nodeId === nodeId);
      let nextIndex = 0;
      if (event.key === "End") nextIndex = rows.length - 1;
      else if (event.key === "ArrowRight" || event.key === "ArrowDown") {
        nextIndex = (currentIndex + 1) % rows.length;
      } else if (event.key === "ArrowLeft" || event.key === "ArrowUp") {
        nextIndex = (currentIndex - 1 + rows.length) % rows.length;
      }
      const nextNodeId = rows[nextIndex]?.nodeId;
      if (!nextNodeId) return;
      setActiveRowId(nextNodeId);
      focusedRowIdRef.current = nextNodeId;
      rowRefs.current.get(nextNodeId)?.focus();
      return;
    }
    if (event.key === "Enter") {
      event.preventDefault();
      onOpenNode?.(nodeId);
      return;
    }
    if (event.key === " ") {
      event.preventDefault();
      onToggleNode?.(nodeId);
      return;
    }
    if (event.key === "ContextMenu" || (event.shiftKey && event.key === "F10")) {
      event.preventDefault();
      const position = rectPosition(event.currentTarget.getBoundingClientRect());
      onShowMenu(nodeId, position.x, position.y);
    }
  }

  return (
    <div
      className="table-shell topology-list-table"
      id="topology-object-navigation"
    >
      <table>
        <caption className="sr-only">
          {tr(locale, "topology.list.caption", {
            count: String(rows.length),
          })}
        </caption>
        <thead>
          <tr>
            <th scope="col">
              <span className="sr-only">
                {label(locale, "topology.selection.column")}
              </span>
            </th>
            <th scope="col">{label(locale, "topology.list.name")}</th>
            <th scope="col">{label(locale, "topology.list.networks")}</th>
            <th scope="col">{label(locale, "topology.list.capabilities")}</th>
            <th scope="col">{label(locale, "topology.list.health")}</th>
            <th scope="col">{label(locale, "topology.list.connections")}</th>
          </tr>
        </thead>
        <tbody>
          {rows.map((row) => {
            const node = nodes.get(row.nodeId);
            if (!node) return null;
            const selected = selectedNodeIds.has(row.nodeId);
            return (
              <tr
                key={row.nodeId}
                ref={(element) => {
                  if (element) rowRefs.current.set(row.nodeId, element);
                  else rowRefs.current.delete(row.nodeId);
                }}
                data-node-id={row.nodeId}
                data-selected={selected ? "true" : "false"}
                className={selected ? "selected" : undefined}
                aria-selected={selected}
                aria-keyshortcuts="ArrowLeft ArrowRight ArrowUp ArrowDown Home End Enter Space Shift+F10 ContextMenu"
                aria-label={nodeAriaLabel(locale, node, networkNames, topology)}
                tabIndex={resolvedActiveRowId === row.nodeId ? 0 : -1}
                onClick={(event) => {
                  if (longPress.consumeActivation(row.nodeId)) return;
                  const target = event.target as HTMLElement;
                  if (target.closest("button, input, a, select")) return;
                  if (event.ctrlKey || event.metaKey || event.shiftKey) {
                    onToggleNode?.(row.nodeId);
                  } else {
                    onOpenNode?.(row.nodeId);
                  }
                }}
                onKeyDown={(event) => handleRowKey(event, row.nodeId)}
                onMouseEnter={(event) =>
                  onShowHover(row.nodeId, event.currentTarget.getBoundingClientRect())
                }
                onMouseLeave={onHideHover}
                onFocus={(event) => {
                  setActiveRowId(row.nodeId);
                  focusedRowIdRef.current = row.nodeId;
                  onShowHover(
                    row.nodeId,
                    event.currentTarget.getBoundingClientRect(),
                  );
                }}
                onBlur={onHideHover}
                onPointerDown={(event) => longPress.start(event, row.nodeId)}
                onPointerMove={longPress.move}
                onPointerUp={longPress.end}
                onPointerCancel={longPress.end}
                onPointerLeave={longPress.end}
                onContextMenu={(event) => {
                  event.preventDefault();
                  onShowMenu(row.nodeId, event.clientX, event.clientY);
                }}
              >
                <td className="topology-list-select">
                  <input
                    type="checkbox"
                    checked={selected}
                    aria-label={tr(locale, "topology.selection.toggle", {
                      name: row.name,
                    })}
                    onChange={() => onToggleNode?.(row.nodeId)}
                    onClick={(event) => event.stopPropagation()}
                    onKeyDown={(event) => event.stopPropagation()}
                  />
                </td>
                <td>
                  <strong>{row.name}</strong>
                  {row.isSelf && (
                    <span className="topology-list-self">
                      {label(locale, "topology.list.self")}
                    </span>
                  )}
                </td>
                <td>
                  <ul className="topology-list-networks">
                    {row.networks.map((network) => (
                      <li key={network.networkId}>
                        <svg
                          viewBox="0 0 24 12"
                          className="topology-network-swatch"
                          aria-hidden="true"
                        >
                          <line
                            x1="1"
                            y1="6"
                            x2="23"
                            y2="6"
                            stroke={network.color}
                            strokeDasharray={network.dash}
                            strokeWidth="2"
                          />
                        </svg>
                        {network.name}
                        <span className="topology-list-role">
                          {label(locale, `join_request.role.${network.role}`)}
                        </span>
                      </li>
                    ))}
                  </ul>
                </td>
                <td>
                  {row.capabilities.length === 0
                    ? label(locale, "status.capability.unknown")
                    : row.capabilities
                        .map((capability) =>
                          label(locale, `capability.${capability}`),
                        )
                        .join(" · ")}
                </td>
                <td>
                  <span className="topology-health-text">
                    <HealthBadgeInline health={row.health} />
                    {healthLabel(locale, row.health)}
                  </span>
                </td>
                <td>
                  <ul className="topology-list-connections">
                    {CONNECTION_ORDER.map((state) => (
                      <li key={state} className={state}>
                        <span>{label(locale, `topology.connection.${state}`)}</span>
                        <strong>{row.connections[state]}</strong>
                      </li>
                    ))}
                  </ul>
                </td>
              </tr>
            );
          })}
        </tbody>
      </table>
    </div>
  );
}

function Legend({
  topology,
  locale,
  mode,
}: {
  topology: TopologySnapshot;
  locale: Locale;
  mode: TopologyColorMode;
}): JSX.Element {
  const visuals = networkVisuals(topology, mode);
  return (
    <details className="topology-legend">
      <summary>{label(locale, "topology.legend.title")}</summary>
      <div className="topology-legend-grid">
        <section aria-label={label(locale, "topology.legend.networks")}>
          <h3>{label(locale, "topology.legend.networks")}</h3>
          <ul>
            {topology.boundaries.map((boundary) => {
              const visual = visuals.get(boundary.network_id);
              return (
                <li key={boundary.network_id}>
                  <svg
                    viewBox="0 0 24 12"
                    aria-hidden="true"
                    className="topology-legend-line"
                  >
                    <line
                      x1="1"
                      y1="6"
                      x2="23"
                      y2="6"
                      stroke={visual?.color}
                      strokeDasharray={visual?.dash}
                      strokeWidth="2"
                    />
                  </svg>
                  <span>{boundary.name}</span>
                </li>
              );
            })}
          </ul>
        </section>
        <section aria-label={label(locale, "topology.legend.capabilities")}>
          <h3>{label(locale, "topology.legend.capabilities")}</h3>
          <ul>
            {CAPABILITY_ORDER.map((capability: CapabilityKind) => (
              <li key={capability}>
                <span
                  className="topology-capability-swatch"
                  style={{ background: capabilityColor(capability, mode) }}
                  aria-hidden="true"
                />
                <span>{label(locale, `capability.${capability}`)}</span>
              </li>
            ))}
          </ul>
        </section>
        <section aria-label={label(locale, "topology.legend.health")}>
          <h3>{label(locale, "topology.legend.health")}</h3>
          <ul>
            {HEALTH_ORDER.map((health) => (
              <li key={health}>
                <HealthBadgeInline health={health} />
                <span>{healthLabel(locale, health)}</span>
              </li>
            ))}
          </ul>
        </section>
        <section aria-label={label(locale, "topology.legend.connections")}>
          <h3>{label(locale, "topology.legend.connections")}</h3>
          <ul>
            {(["active", "idle", "stale"] as const).map((state) => (
              <li key={state}>
                <svg
                  viewBox="0 0 24 12"
                  aria-hidden="true"
                  className="topology-legend-line"
                >
                  <line
                    x1="1"
                    y1="6"
                    x2="23"
                    y2="6"
                    stroke="currentColor"
                    strokeDasharray={connectionDash(state)}
                    strokeWidth="2"
                  />
                </svg>
                <span>{label(locale, `topology.connection.${state}`)}</span>
              </li>
            ))}
          </ul>
        </section>
      </div>
      <p className="topology-legend-note">
        {label(locale, "topology.legend.overlap")}
      </p>
    </details>
  );
}

export function TopologyView({
  topology,
  locale,
  filters,
  onFiltersChange,
  selectedNodeIds = new Set<string>(),
  onToggleNode,
  onOpenNode,
  networkId = null,
  membersByNodeId = new Map<string, NetworkMember>(),
  lifecycleActions,
  onPrepareLifecycle,
  onClearSelection,
  selectionNotice = "",
}: Props): JSX.Element {
  const narrowViewport = useNarrowViewport();
  const [view, setView] = useState<TopologyViewMode>(() =>
    narrowViewport ? "list" : "graph",
  );
  const viewWasExplicitRef = useRef(false);
  const [mode, setMode] = useState<TopologyColorMode>("default");
  const [highContrast, setHighContrast] = useState(false);
  const [zoom, setZoom] = useState<number>(1);
  const [frozenTopology, setFrozenTopology] = useState<TopologySnapshot | null>(
    null,
  );
  const [focusRequest, setFocusRequest] = useState(0);
  const [internalFilters, setInternalFilters] = useState<TopologyFilters>({
    ...EMPTY_TOPOLOGY_FILTERS,
  });
  const [hover, setHover] = useState<HoverState | null>(null);
  const [menu, setMenu] = useState<NodeMenuState | null>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  const menuNodeIdRef = useRef<string | null>(null);
  const activeFilters = filters ?? internalFilters;
  const sourceTopology = frozenTopology ?? topology;
  const filtered = useMemo(
    () =>
      sourceTopology
        ? filteredTopologySnapshot(sourceTopology, activeFilters)
        : null,
    [sourceTopology, activeFilters],
  );
  const visible = filtered ?? sourceTopology;
  const focusTargetNodeId =
    visible?.nodes.find((node) => node.is_self)?.node_id ??
    visible?.nodes[0]?.node_id ??
    null;
  const visibleTopologyRef = useRef(visible);

  useEffect(() => {
    visibleTopologyRef.current = visible;
  }, [visible]);

  const restoreTopologyFocus = useCallback((nodeId: string): void => {
    window.setTimeout(() => {
      const nodes = [
        ...document.querySelectorAll<HTMLElement>("[data-node-id]"),
      ];
      const preferred = nodes.find(
        (element) => element.dataset.nodeId === nodeId,
      );
      const fallback =
        visibleTopologyRef.current?.nodes.find((node) => node.is_self)
          ?.node_id ??
        visibleTopologyRef.current?.nodes[0]?.node_id ??
        null;
      const target =
        preferred ??
        nodes.find((element) => element.dataset.nodeId === fallback);
      target?.focus();
      target?.scrollIntoView?.({ block: "center", inline: "center" });
    }, 0);
  }, []);

  const closeMenu = useCallback(
    (restoreFocus = true): void => {
      const nodeId = menuNodeIdRef.current;
      menuNodeIdRef.current = null;
      setMenu(null);
      if (restoreFocus && nodeId) restoreTopologyFocus(nodeId);
    },
    [restoreTopologyFocus],
  );

  useEffect(() => {
    if (!viewWasExplicitRef.current) {
      setView(narrowViewport ? "list" : "graph");
    }
  }, [narrowViewport]);

  useEffect(() => {
    if (!menu) return;
    const focusTimer = window.setTimeout(() => {
      const firstAction = menuRef.current?.querySelector<HTMLButtonElement>(
        'button[role="menuitem"]:not([aria-disabled="true"])',
      );
      (firstAction ?? menuRef.current)?.focus();
    }, 0);
    function closeOnOutside(event: PointerEvent): void {
      if (!menuRef.current?.contains(event.target as Node)) closeMenu(false);
    }
    function closeOnEscape(event: KeyboardEvent): void {
      if (event.key !== "Escape") return;
      event.preventDefault();
      event.stopPropagation();
      closeMenu(true);
    }
    document.addEventListener("pointerdown", closeOnOutside);
    document.addEventListener("keydown", closeOnEscape);
    return () => {
      window.clearTimeout(focusTimer);
      document.removeEventListener("pointerdown", closeOnOutside);
      document.removeEventListener("keydown", closeOnEscape);
    };
  }, [closeMenu, menu]);

  function changeView(nextView: TopologyViewMode): void {
    viewWasExplicitRef.current = true;
    setView(nextView);
  }

  function changeFilters(next: TopologyFilters): void {
    if (onFiltersChange) onFiltersChange(next);
    else setInternalFilters(next);
  }

  function showHover(nodeId: string, rect: DOMRect): void {
    if (menu) return;
    const position = rectPosition(rect);
    const clamped = clampOverlayPosition(
      position.x + 14,
      position.y + 14,
      340,
      190,
    );
    setHover({ nodeId, x: clamped.x, y: clamped.y });
  }

  const showMenu = useCallback(
    (nodeId: string, x: number, y: number): void => {
      setHover(null);
      const clamped = clampOverlayPosition(x, y, 270, 360);
      menuNodeIdRef.current = nodeId;
      setMenu({ nodeId, x: clamped.x, y: clamped.y });
    },
    [],
  );

  function zoomBy(direction: -1 | 1): void {
    setZoom((current) => {
      const currentIndex = ZOOM_LEVELS.indexOf(
        current as (typeof ZOOM_LEVELS)[number],
      );
      const nextIndex = Math.max(
        0,
        Math.min(ZOOM_LEVELS.length - 1, currentIndex + direction),
      );
      return ZOOM_LEVELS[nextIndex] ?? current;
    });
  }

  function toggleFrozen(): void {
    setFrozenTopology((current) => (current ? null : topology));
  }

  function locateCurrentNode(): void {
    if (!focusTargetNodeId) return;
    setFocusRequest((current) => current + 1);
  }

  if (!sourceTopology) {
    return (
      <div className="empty-state">
        {label(locale, "topology.empty.unavailable")}
      </div>
    );
  }
  if (sourceTopology.boundaries.length === 0) {
    return (
      <div className="empty-state">
        {label(locale, "topology.empty.no_networks")}
      </div>
    );
  }

  const displayed = visible ?? sourceTopology;
  const hoverNode = hover
    ? sourceTopology.nodes.find((node) => node.node_id === hover.nodeId)
    : null;
  const menuNode = menu
    ? sourceTopology.nodes.find((node) => node.node_id === menu.nodeId)
    : null;
  const selectionCount = [...selectedNodeIds].filter((nodeId) =>
    sourceTopology.nodes.some((node) => node.node_id === nodeId),
  ).length;
  const zoomIndex = ZOOM_LEVELS.indexOf(
    zoom as (typeof ZOOM_LEVELS)[number],
  );
  const percentage = Math.round(zoom * 100);

  return (
    <div
      className={`topology-view ${highContrast ? "topology-high-contrast" : ""}`}
      data-color-mode={mode}
      data-frozen={frozenTopology ? "true" : "false"}
      data-view={view}
    >
      <div
        className="topology-toolbar"
        role="group"
        aria-label={label(locale, "topology.view.label")}
      >
        <div
          className="topology-view-switch"
          role="group"
          aria-label={label(locale, "topology.view.label")}
        >
          <button
            type="button"
            aria-pressed={view === "graph"}
            onClick={() => changeView("graph")}
          >
            {label(locale, "topology.view.graph")}
          </button>
          <button
            type="button"
            aria-pressed={view === "list"}
            onClick={() => changeView("list")}
          >
            {label(locale, "topology.view.list")}
          </button>
        </div>
        <div
          className="topology-color-switch"
          role="group"
          aria-label={label(locale, "topology.mode.label")}
        >
          <button
            type="button"
            aria-pressed={mode === "default"}
            onClick={() => setMode("default")}
          >
            {label(locale, "topology.mode.default")}
          </button>
          <button
            type="button"
            aria-pressed={mode === "colorblind"}
            onClick={() => setMode("colorblind")}
          >
            {label(locale, "topology.mode.colorblind")}
          </button>
          <label className="topology-contrast-toggle">
            <input
              type="checkbox"
              checked={highContrast}
              onChange={(event) => setHighContrast(event.currentTarget.checked)}
            />
            <span>{label(locale, "topology.mode.high_contrast")}</span>
          </label>
        </div>
        <div
          className="topology-navigation-controls"
          role="group"
          aria-label={label(locale, "topology.navigation.label")}
        >
          <button
            type="button"
            className="topology-control-button topology-zoom-button"
            aria-label={label(locale, "topology.zoom.out")}
            title={label(locale, "topology.zoom.out")}
            disabled={view !== "graph" || zoomIndex <= 0}
            onClick={() => zoomBy(-1)}
          >
            <Minus size={15} aria-hidden="true" />
          </button>
          <span
            className="topology-zoom-status"
            role="status"
            aria-live="polite"
            data-testid="topology-zoom-status"
          >
            {tr(locale, "topology.zoom.level", {
              percent: String(percentage),
            })}
          </span>
          <button
            type="button"
            className="topology-control-button topology-zoom-button"
            aria-label={label(locale, "topology.zoom.in")}
            title={label(locale, "topology.zoom.in")}
            disabled={
              view !== "graph" || zoomIndex === ZOOM_LEVELS.length - 1
            }
            onClick={() => zoomBy(1)}
          >
            <Plus size={15} aria-hidden="true" />
          </button>
          <button
            type="button"
            className="topology-control-button"
            aria-label={label(locale, "topology.zoom.reset")}
            title={label(locale, "topology.zoom.reset")}
            disabled={view !== "graph" || zoom === 1}
            onClick={() => setZoom(1)}
          >
            <RotateCcw size={15} aria-hidden="true" />
          </button>
          <button
            type="button"
            className="topology-control-button topology-control-wide"
            aria-label={label(
              locale,
              frozenTopology
                ? "topology.freeze.unfreeze"
                : "topology.freeze.freeze",
            )}
            aria-pressed={Boolean(frozenTopology)}
            onClick={toggleFrozen}
          >
            {frozenTopology ? (
              <Play size={15} aria-hidden="true" />
            ) : (
              <Pause size={15} aria-hidden="true" />
            )}
            <span>
              {label(
                locale,
                frozenTopology
                  ? "topology.freeze.unfreeze"
                  : "topology.freeze.freeze",
              )}
            </span>
          </button>
          <button
            type="button"
            className="topology-control-button topology-control-wide"
            aria-label={label(locale, "topology.locate.current")}
            disabled={!focusTargetNodeId}
            onClick={locateCurrentNode}
          >
            <LocateFixed size={15} aria-hidden="true" />
            <span>{label(locale, "topology.locate.current")}</span>
          </button>
        </div>
      </div>
      <FilterToolbar
        topology={sourceTopology}
        locale={locale}
        filters={activeFilters}
        visibleCount={displayed.nodes.length}
        onChange={changeFilters}
      />
      {selectionCount > 0 ? (
        <section className="topology-selection-bar" aria-live="polite">
          <div>
            <strong>
              {tr(locale, "topology.selection.count", {
                count: String(selectionCount),
              })}
            </strong>
            <span>{label(locale, "topology.selection.hint")}</span>
          </div>
          <button
            type="button"
            className="secondary-button"
            onClick={() => {
              setHover(null);
              onClearSelection?.();
            }}
          >
            <X size={14} />
            {label(locale, "topology.selection.clear")}
          </button>
        </section>
      ) : null}
      {selectionNotice ? (
        <p className="topology-selection-notice" role="status">
          {selectionNotice}
        </p>
      ) : null}
      <p className="topology-selection-refresh-hint">
        {label(locale, "topology.selection.refresh_hint")}
      </p>
      {displayed.nodes.length === 0 ? (
        <div className="empty-state">{label(locale, "topology.filters.empty")}</div>
      ) : view === "graph" ? (
        <GraphView
          topology={displayed}
          locale={locale}
          mode={mode}
          zoom={zoom}
          focusRequest={focusRequest}
          focusTargetNodeId={focusTargetNodeId}
          selectedNodeIds={selectedNodeIds}
          onOpenNode={onOpenNode}
          onToggleNode={onToggleNode}
          onShowHover={showHover}
          onHideHover={() => setHover(null)}
          onShowMenu={showMenu}
        />
      ) : (
        <ListView
          topology={displayed}
          locale={locale}
          mode={mode}
          focusRequest={focusRequest}
          focusTargetNodeId={focusTargetNodeId}
          selectedNodeIds={selectedNodeIds}
          onOpenNode={onOpenNode}
          onToggleNode={onToggleNode}
          onShowHover={showHover}
          onHideHover={() => setHover(null)}
          onShowMenu={showMenu}
        />
      )}
      <Legend topology={displayed} locale={locale} mode={mode} />
      {hoverNode ? (
        <NodeHoverCard
          node={hoverNode}
          member={membersByNodeId.get(hoverNode.node_id)}
          locale={locale}
          networkId={networkId}
          x={hover?.x ?? 0}
          y={hover?.y ?? 0}
        />
      ) : null}
      {menu && menuNode ? (
        <NodeActionMenu
          node={menuNode}
          locale={locale}
          selected={selectedNodeIds.has(menuNode.node_id)}
          lifecycleActions={lifecycleActions?.(menuNode.node_id) ?? []}
          x={menu.x}
          y={menu.y}
          menuRef={menuRef}
          onOpen={(nodeId) => onOpenNode?.(nodeId)}
          onToggle={(nodeId) => onToggleNode?.(nodeId)}
          onPrepareLifecycle={(nodeId, action) =>
            onPrepareLifecycle?.(nodeId, action)
          }
          onClose={closeMenu}
        />
      ) : null}
    </div>
  );
}
