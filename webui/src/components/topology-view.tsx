import {
  startTransition,
  useCallback,
  useDeferredValue,
  useEffect,
  memo,
  useMemo,
  useRef,
  useState,
  useSyncExternalStore,
  type JSX,
  type KeyboardEvent as ReactKeyboardEvent,
  type MouseEvent as ReactMouseEvent,
  type PointerEvent as ReactPointerEvent,
} from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import {
  AlertTriangle,
  Info,
  List,
  LoaderCircle,
  LocateFixed,
  Minus,
  MoreHorizontal,
  Network,
  Pause,
  Play,
  Plus,
  Pin,
  PinOff,
  RefreshCw,
  RotateCcw,
  Search,
  ShieldAlert,
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
  capabilityColor,
  capabilitySegments,
  connectionDash,
  cachedTopologyIndex,
  createTopologyIndex,
  healthEncoding,
  hiddenMemberCount,
  layoutTopology,
  networkVisuals,
  topologyGraphBudget,
  topologyLayoutKey,
  topologyListRows,
  type HealthEncoding,
  type TopologyColorMode,
  type TopologyConnectionCounts,
  type TopologyIndex,
  type TopologyLayout,
  type TopologyPlacement,
} from "../topology";
import {
  applyFixedTopologyPositions,
  clampTopologyNodePosition,
  clearPersistedTopologyPositions,
  loadPersistedTopologyPositions,
  savePersistedTopologyPositions,
  type TopologyFixedPositions,
  type TopologyNodePosition,
} from "../topology-layout-persistence";
import {
  EMPTY_TOPOLOGY_FILTERS,
  filteredTopologySnapshot,
  type BatchMemberAction,
  type TopologyFilters,
} from "../topology-actions";
import {
  topologyConditionCopy,
  topologyPresentation,
  type TopologyPresentation,
  type TopologyPresentationSignals,
} from "../topology-presentation";

type Props = {
  topology: TopologySnapshot | null;
  locale: Locale;
  presentationSignals?: TopologyPresentationSignals;
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
  onSelectNetwork?: (networkId: string) => void;
  selectionNotice?: string;
};

function TopologyPresentationNotices({
  presentation,
  locale,
}: {
  presentation: TopologyPresentation;
  locale: Locale;
}) {
  return (
    <div className="topology-state-notices">
      {presentation.conditions
        .filter((condition) => condition !== "ready")
        .map((condition) => {
          const copy = topologyConditionCopy(condition);
          const Icon =
            copy.tone === "danger"
              ? AlertTriangle
              : condition === "refreshing" || condition === "initial_loading"
                ? LoaderCircle
                : copy.tone === "warn"
                  ? ShieldAlert
                  : condition === "partial"
                    ? RefreshCw
                    : Info;
          return (
            <div
              className={`state-notice ${copy.tone}`}
              data-topology-state={condition}
              data-testid={`topology-state-${condition}`}
              key={condition}
              role={copy.tone === "danger" ? "alert" : "status"}
            >
              <Icon
                className={
                  condition === "refreshing" || condition === "initial_loading"
                    ? "spin"
                    : undefined
                }
                size={17}
                aria-hidden="true"
              />
              <div>
                <strong>{label(locale, copy.titleKey)}</strong>
                <p>{label(locale, copy.detailKey)}</p>
              </div>
            </div>
          );
        })}
    </div>
  );
}

type HoverState = {
  nodeId: string;
  x: number;
  y: number;
};

type NodeMenuState = {
  nodeId: string;
  x: number;
  y: number;
  layoutPosition?: TopologyNodePosition;
};

const NODE_STATUS_RADIUS = 10;
const MEMBERSHIP_ROLES = ["owner", "administrator", "member"] as const;
const NARROW_VIEWPORT_QUERY = "(max-width: 600px)";
const TOUCH_LONG_PRESS_MS = 450;
const TOUCH_MOVE_TOLERANCE_PX = 8;
const ZOOM_LEVELS = [0.75, 1, 1.25, 1.5, 2] as const;
const EMPTY_SELECTED_NODE_IDS: ReadonlySet<string> = new Set();

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
  index: TopologyIndex,
): string {
  const counts: TopologyConnectionCounts = index.connectionCountsByNode.get(
    nodeId,
  ) ?? { active: 0, idle: 0, stale: 0 };
  const summary = CONNECTION_ORDER.filter((state) => counts[state] > 0)
    .map(
      (state) =>
        `${label(locale, `topology.connection.${state}`)} ${counts[state]}`,
    )
    .join(label(locale, "common.list_separator"));
  return summary || label(locale, "topology.aria.no_connections");
}

function nodeAriaLabel(
  locale: Locale,
  node: TopologyNode,
  networkNames: Map<string, string>,
  index: TopologyIndex,
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
  const relationships = nodeRelationshipText(locale, node.node_id, index);
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
  layoutPinned,
  layoutPosition,
  x,
  y,
  menuRef,
  onOpen,
  onToggle,
  onPrepareLifecycle,
  onPinLayout,
  onClose,
}: {
  node: TopologyNode;
  locale: Locale;
  selected: boolean;
  lifecycleActions: MemberLifecycleAction[];
  layoutPinned: boolean;
  layoutPosition?: TopologyNodePosition;
  x: number;
  y: number;
  menuRef: React.RefObject<HTMLDivElement | null>;
  onOpen(nodeId: string): void;
  onToggle(nodeId: string): void;
  onPrepareLifecycle(nodeId: string, action: BatchMemberAction): void;
  onPinLayout(nodeId: string, position: TopologyNodePosition): void;
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
      {layoutPosition ? (
        <button
          type="button"
          role="menuitem"
          onClick={() => {
            onClose(false);
            onPinLayout(node.node_id, layoutPosition);
          }}
        >
          <Pin size={13} aria-hidden="true" />
          {label(
            locale,
            layoutPinned
              ? "topology.menu.update_layout"
              : "topology.menu.pin_layout",
          )}
        </button>
      ) : null}
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

type TopologyLayoutState = {
  layout: TopologyLayout | null;
  pending: boolean;
};

const MAX_SYNC_LAYOUT_NODES = 80;

/** Large layouts are computed in a worker; small layouts stay synchronous so
 * tests and first paint never depend on an asynchronous round trip. */
function useTopologyLayout(topology: TopologySnapshot): TopologyLayoutState {
  const layoutKey = useMemo(() => topologyLayoutKey(topology), [topology]);
  const synchronousLayout = useMemo(
    () =>
      topology.nodes.length <= MAX_SYNC_LAYOUT_NODES
        ? layoutTopology(topology)
        : null,
    [topology],
  );
  const topologyForLayoutRef = useRef(topology);
  const layoutRequestSequence = useRef(0);
  const [workerLayout, setWorkerLayout] = useState<{
    key: string;
    layout: TopologyLayout;
  } | null>(null);

  useEffect(() => {
    topologyForLayoutRef.current = topology;
  }, [topology]);

  useEffect(() => {
    if (topology.nodes.length <= MAX_SYNC_LAYOUT_NODES) {
      return;
    }
    const layoutTopologySnapshot = topologyForLayoutRef.current;
    const requestId = ++layoutRequestSequence.current;
    let active = true;
    const commitLayout = (layout: TopologyLayout): void => {
      if (!active || layoutRequestSequence.current !== requestId) return;
      startTransition(() => setWorkerLayout({ key: layoutKey, layout }));
    };
    if (typeof Worker === "undefined") {
      queueMicrotask(() => commitLayout(layoutTopology(layoutTopologySnapshot)));
      return;
    }
    const worker = new Worker(
      new URL("../topology-layout-worker.ts", import.meta.url),
      { type: "module" },
    );
    worker.onmessage = (
      event: MessageEvent<{
        requestId: number;
        layout: TopologyLayout;
      }>,
    ) => {
      if (event.data.requestId !== requestId) return;
      commitLayout(event.data.layout);
    };
    worker.onerror = () => {
      commitLayout(layoutTopology(layoutTopologySnapshot));
    };
    worker.postMessage({
      requestId,
      snapshot: { ...layoutTopologySnapshot, connections: [] },
      width: 720,
      height: 360,
    });
    return () => {
      active = false;
      worker.terminate();
    };
  }, [layoutKey, topology.nodes.length]);

  if (synchronousLayout) return { layout: synchronousLayout, pending: false };
  const currentWorkerLayout =
    workerLayout?.key === layoutKey ? workerLayout.layout : null;
  return { layout: currentWorkerLayout, pending: currentWorkerLayout === null };
}

const GraphView = memo(function GraphView({
  topology,
  index,
  locale,
  mode,
  zoom,
  fixedPositions,
  focusRequest,
  focusTargetNodeId,
  selectedNodeIds,
  onOpenNode,
  onToggleNode,
  onPinNode,
  onShowHover,
  onHideHover,
  onShowMenu,
}: {
  topology: TopologySnapshot;
  index: TopologyIndex;
  locale: Locale;
  mode: TopologyColorMode;
  zoom: number;
  fixedPositions: TopologyFixedPositions;
  focusRequest: number;
  focusTargetNodeId: string | null;
  selectedNodeIds: ReadonlySet<string>;
  onOpenNode?: (nodeId: string) => void;
  onToggleNode?: (nodeId: string) => void;
  onPinNode(nodeId: string, position: TopologyNodePosition): void;
  onShowHover(nodeId: string, rect: DOMRect): void;
  onHideHover(): void;
  onShowMenu(
    nodeId: string,
    x: number,
    y: number,
    layoutPosition?: TopologyNodePosition,
  ): void;
}): JSX.Element {
  const budget = useMemo(
    () => topologyGraphBudget(topology, selectedNodeIds),
    [selectedNodeIds, topology],
  );
  const graphTopology = useMemo(
    () => ({
      ...topology,
      nodes: budget.nodes,
      connections: budget.connections,
    }),
    [budget.connections, budget.nodes, topology],
  );
  const { layout: automaticLayout, pending: layoutPending } =
    useTopologyLayout(graphTopology);
  const [dragPreview, setDragPreview] = useState<{
    nodeId: string;
    position: TopologyNodePosition;
  } | null>(null);
  const effectiveFixedPositions = useMemo(() => {
    if (!dragPreview) return fixedPositions;
    const positions = new Map(fixedPositions);
    positions.set(dragPreview.nodeId, dragPreview.position);
    return positions;
  }, [dragPreview, fixedPositions]);
  const layout = useMemo(
    () =>
      automaticLayout
        ? applyFixedTopologyPositions(automaticLayout, effectiveFixedPositions)
        : null,
    [automaticLayout, effectiveFixedPositions],
  );
  const previewNodeDrag = useCallback(
    (nodeId: string, position: TopologyNodePosition): void => {
      if (!layout) return;
      setDragPreview({
        nodeId,
        position: clampTopologyNodePosition(position, layout),
      });
    },
    [layout],
  );
  const commitNodeDrag = useCallback(
    (nodeId: string, position: TopologyNodePosition): void => {
      setDragPreview(null);
      if (!layout) return;
      onPinNode(nodeId, clampTopologyNodePosition(position, layout));
    },
    [layout, onPinNode],
  );
  const cancelNodeDrag = useCallback((): void => setDragPreview(null), []);
  const visuals = useMemo(
    () => networkVisuals(topology, mode),
    [mode, topology],
  );
  const placements = useMemo(
    () =>
      new Map(
        (layout?.placements ?? []).map((placement) => [
          placement.nodeId,
          placement,
        ]),
      ),
    [layout],
  );
  const placementsByNetwork = useMemo(() => {
    const grouped = new Map<string, TopologyPlacement[]>();
    for (const placement of layout?.placements ?? []) {
      const networkPlacements = grouped.get(placement.anchorNetworkId) ?? [];
      networkPlacements.push(placement);
      grouped.set(placement.anchorNetworkId, networkPlacements);
    }
    return grouped;
  }, [layout]);
  const renderedNodes = useMemo(
    () => graphTopology.nodes.filter((node) => placements.has(node.node_id)),
    [graphTopology.nodes, placements],
  );
  const renderedConnections = useMemo(
    () =>
      graphTopology.connections.filter(
        (connection) =>
          placements.has(connection.from) && placements.has(connection.to),
      ),
    [graphTopology.connections, placements],
  );
  const boundaryNames = useMemo(
    () =>
      new Map(
        topology.boundaries.map((boundary) => [
          boundary.network_id,
          boundary.name,
        ]),
      ),
    [topology.boundaries],
  );
  const nodeLabels = useMemo(
    () =>
      new Map(
        graphTopology.nodes.map((node) => [
          node.node_id,
          nodeAriaLabel(locale, node, boundaryNames, index),
        ]),
      ),
    [boundaryNames, graphTopology.nodes, index, locale],
  );
  const nodeIds = graphTopology.nodes.map((node) => node.node_id);
  const nodeIdsRef = useRef(nodeIds);
  const nodeRefs = useRef(new Map<string, SVGGElement>());
  const focusedNodeIdRef = useRef<string | null>(null);
  const [activeNodeId, setActiveNodeId] = useState<string | null>(
    () =>
      graphTopology.nodes.find((node) => node.is_self)?.node_id ??
      graphTopology.nodes[0]?.node_id ??
      null,
  );
  const fallbackNodeId =
    graphTopology.nodes.find((node) => node.is_self)?.node_id ??
    graphTopology.nodes[0]?.node_id ??
    null;
  const resolvedActiveNodeId = graphTopology.nodes.some(
    (node) => node.node_id === activeNodeId,
  )
    ? activeNodeId
    : fallbackNodeId;
  const showNodeMenu = useCallback(
    (nodeId: string, x: number, y: number): void => {
      const placement = placements.get(nodeId);
      onShowMenu(
        nodeId,
        x,
        y,
        placement ? { x: placement.x, y: placement.y } : undefined,
      );
    },
    [onShowMenu, placements],
  );
  const longPress = useTouchLongPress(showNodeMenu);
  const focusTargetNodeRef = useRef(focusTargetNodeId);
  const focusNode = useCallback((nodeId: string): void => {
    setActiveNodeId(nodeId);
    focusedNodeIdRef.current = nodeId;
  }, []);
  const setNodeRef = useCallback(
    (nodeId: string, element: SVGGElement | null): void => {
      if (element) nodeRefs.current.set(nodeId, element);
      else nodeRefs.current.delete(nodeId);
    },
    [],
  );
  const menuFromFocus = useCallback(
    (event: ReactKeyboardEvent<SVGGElement>, nodeId: string): void => {
      if (event.key !== "ContextMenu" && !(event.shiftKey && event.key === "F10")) {
        return;
      }
      event.preventDefault();
      const position = rectPosition(event.currentTarget.getBoundingClientRect());
      const placement = placements.get(nodeId);
      onShowMenu(
        nodeId,
        position.x,
        position.y,
        placement ? { x: placement.x, y: placement.y } : undefined,
      );
    },
    [onShowMenu, placements],
  );
  const navigateNode = useCallback(
    (event: ReactKeyboardEvent<SVGGElement>, nodeId: string): void => {
      if (
        !["ArrowLeft", "ArrowRight", "ArrowUp", "ArrowDown", "Home", "End"].includes(
          event.key,
        )
      ) {
        return;
      }
      event.preventDefault();
      const nodeIds = nodeIdsRef.current;
      const currentIndex = Math.max(0, nodeIds.indexOf(nodeId));
      let nextIndex = 0;
      if (event.key === "End") nextIndex = nodeIds.length - 1;
      else if (event.key === "ArrowRight" || event.key === "ArrowDown") {
        nextIndex = (currentIndex + 1) % nodeIds.length;
      } else if (event.key === "ArrowLeft" || event.key === "ArrowUp") {
        nextIndex = (currentIndex - 1 + nodeIds.length) % nodeIds.length;
      }
      const nextNodeId = nodeIds[nextIndex];
      if (!nextNodeId) return;
      setActiveNodeId(nextNodeId);
      focusedNodeIdRef.current = nextNodeId;
      nodeRefs.current.get(nextNodeId)?.focus();
    },
    [],
  );

  useEffect(() => {
    nodeIdsRef.current = nodeIds;
  }, [nodeIds]);

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
      graphTopology.nodes.some((node) => node.node_id === focusedNodeId)
    ) {
      return;
    }
    focusedNodeIdRef.current = resolvedActiveNodeId;
    const fallback = resolvedActiveNodeId
      ? nodeRefs.current.get(resolvedActiveNodeId)
      : undefined;
    fallback?.focus();
    fallback?.scrollIntoView?.({ block: "center", inline: "center" });
  }, [graphTopology.nodes, resolvedActiveNodeId]);

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

  if (!layout) {
    return (
      <div
        className="topology-canvas topology-canvas-pending"
        id="topology-object-navigation"
        role="status"
        data-layout-pending="true"
      >
        {label(locale, "topology.layout.pending")}
      </div>
    );
  }

  return (
    <div
      className="topology-canvas"
      id="topology-object-navigation"
      data-layout-pending={layoutPending ? "true" : "false"}
      data-rendered-nodes={renderedNodes.length}
      data-rendered-connections={renderedConnections.length}
    >
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
            .replace("{nodes}", String(renderedNodes.length))
            .replace(
              "{links}",
              String(
                renderedConnections.filter(
                  (connection) => connection.state === "active",
                ).length,
              ),
            )}
        </desc>
        {layout.anchors.map((anchor) => {
          const boundary = index.boundariesById.get(anchor.networkId);
          const visual = visuals.get(anchor.networkId);
          if (!boundary || !visual) return null;
          const memberSlots = placementsByNetwork.get(anchor.networkId) ?? [];
          const rx = Math.max(
            86,
            ...memberSlots.map(
              (placement) => Math.abs(placement.x - anchor.x) + 88,
            ),
          );
          const ry = Math.max(
            76,
            ...memberSlots.map(
              (placement) => Math.abs(placement.y - anchor.y) + 36,
            ),
          );
          const hidden = hiddenMemberCount(topology, anchor.networkId, index);
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
                x={anchor.x - rx + 14}
                y={Math.max(12, anchor.y - ry - 7)}
                className="topology-boundary-label"
                fill={visual.color}
              >
                {boundary.name}
              </text>
            </g>
          );
        })}
        {renderedConnections.map((connection) => (
          <ConnectionLine
            key={connection.connection_id}
            connection={connection}
            placements={placements}
            locale={locale}
            fromName={
              index.nodesById.get(connection.from)?.display_name ??
              connection.from
            }
            toName={
              index.nodesById.get(connection.to)?.display_name ??
              connection.to
            }
            boundaryNames={boundaryNames}
          />
        ))}
        <g
          role="listbox"
          aria-label={label(locale, "topology.aria.nodes")}
          aria-multiselectable="true"
          aria-keyshortcuts="ArrowLeft ArrowRight ArrowUp ArrowDown Home End"
        >
          {renderedNodes.map((node, nodeIndex) => {
            const placement = placements.get(node.node_id);
            if (!placement) return null;
            const visual = visuals.get(placement.anchorNetworkId);
            return (
              <TopologyNodeGlyph
                key={node.node_id}
                itemIndex={nodeIndex}
                itemCount={renderedNodes.length}
                node={node}
                ariaLabel={nodeLabels.get(node.node_id) ?? ""}
                x={placement.x}
                y={placement.y}
                mode={mode}
                locale={locale}
                networkColor={visual?.color ?? "#6b7280"}
                selected={selectedNodeIds.has(node.node_id)}
                layoutPinned={fixedPositions.has(node.node_id)}
                tabIndex={resolvedActiveNodeId === node.node_id ? 0 : -1}
                onOpenNode={onOpenNode}
                onToggleNode={onToggleNode}
                onShowHover={onShowHover}
                onHideHover={onHideHover}
                onShowMenu={onShowMenu}
                onDragPreview={previewNodeDrag}
                onCommitDrag={commitNodeDrag}
                onCancelDrag={cancelNodeDrag}
                onMenuKey={menuFromFocus}
                onNavigate={navigateNode}
                onFocusNode={focusNode}
                onLongPressStart={longPress.start}
                onLongPressMove={longPress.move}
                onLongPressEnd={longPress.end}
                consumeLongPress={longPress.consumeActivation}
                setNodeRef={setNodeRef}
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
      {budget.hiddenNodes > 0 || budget.hiddenConnections > 0 ? (
        <p className="topology-graph-budget" role="status">
          {label(locale, "topology.graph.compact")
            .replace("{nodes}", String(graphTopology.nodes.length))
            .replace("{totalNodes}", String(topology.nodes.length))
            .replace("{connections}", String(graphTopology.connections.length))
            .replace("{totalConnections}", String(topology.connections.length))}
        </p>
      ) : null}
    </div>
  );
});

const ConnectionLine = memo(function ConnectionLine({
  connection,
  placements,
  locale,
  fromName,
  toName,
  boundaryNames,
}: {
  connection: TopologyConnection;
  placements: Map<string, { x: number; y: number }>;
  locale: Locale;
  fromName: string;
  toName: string;
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
    <g
      className={`topology-connection ${connection.state}`}
      data-connection-id={connection.connection_id}
      data-from={connection.from}
      data-to={connection.to}
      data-networks={(connection.networks ?? []).join(",")}
      data-observed-at={connection.observed_at_unix_ms ?? ""}
    >
      <title>
        {`${fromName} → ${toName} · ${stateLabel}${networks ? ` · ${networks}` : ""}`}
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
}, areConnectionLinePropsEqual);

function samePlacement(
  left: { x: number; y: number } | undefined,
  right: { x: number; y: number } | undefined,
): boolean {
  return left === right || (left?.x === right?.x && left?.y === right?.y);
}

function sameConnectionNetworks(
  left: TopologyConnection,
  right: TopologyConnection,
  boundaryNames: Map<string, string>,
): boolean {
  const leftNetworks = left.networks ?? [];
  const rightNetworks = right.networks ?? [];
  return (
    leftNetworks.length === rightNetworks.length &&
    leftNetworks.every(
      (networkId, index) =>
        networkId === rightNetworks[index] &&
        boundaryNames.get(networkId) === boundaryNames.get(rightNetworks[index] ?? ""),
    )
  );
}

function areConnectionLinePropsEqual(
  previous: {
    connection: TopologyConnection;
    placements: Map<string, { x: number; y: number }>;
    locale: Locale;
    fromName: string;
    toName: string;
    boundaryNames: Map<string, string>;
  },
  next: {
    connection: TopologyConnection;
    placements: Map<string, { x: number; y: number }>;
    locale: Locale;
    fromName: string;
    toName: string;
    boundaryNames: Map<string, string>;
  },
): boolean {
  if (
    previous.connection === next.connection &&
    previous.placements === next.placements &&
    previous.locale === next.locale &&
    previous.fromName === next.fromName &&
    previous.toName === next.toName &&
    previous.boundaryNames === next.boundaryNames
  ) {
    return true;
  }
  if (
    previous.locale !== next.locale ||
    previous.connection.connection_id !== next.connection.connection_id ||
    previous.connection.from !== next.connection.from ||
    previous.connection.to !== next.connection.to ||
    previous.connection.state !== next.connection.state ||
    previous.fromName !== next.fromName ||
    previous.toName !== next.toName ||
    !sameConnectionNetworks(
      previous.connection,
      next.connection,
      next.boundaryNames,
    ) ||
    !samePlacement(
      previous.placements.get(previous.connection.from),
      next.placements.get(next.connection.from),
    ) ||
    !samePlacement(
      previous.placements.get(previous.connection.to),
      next.placements.get(next.connection.to),
    )
  ) {
    return false;
  }
  return true;
}

type TopologyNodeGlyphProps = {
  itemIndex: number;
  itemCount: number;
  node: TopologyNode;
  ariaLabel: string;
  x: number;
  y: number;
  mode: TopologyColorMode;
  locale: Locale;
  networkColor: string;
  selected: boolean;
  layoutPinned: boolean;
  tabIndex: number;
  onOpenNode?: (nodeId: string) => void;
  onToggleNode?: (nodeId: string) => void;
  onShowHover(nodeId: string, rect: DOMRect): void;
  onHideHover(): void;
  onShowMenu(
    nodeId: string,
    x: number,
    y: number,
    layoutPosition?: TopologyNodePosition,
  ): void;
  onDragPreview(nodeId: string, position: TopologyNodePosition): void;
  onCommitDrag(nodeId: string, position: TopologyNodePosition): void;
  onCancelDrag(): void;
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
  setNodeRef(nodeId: string, element: SVGGElement | null): void;
};

const TopologyNodeGlyph = memo(function TopologyNodeGlyph({
  itemIndex,
  itemCount,
  node,
  ariaLabel,
  x,
  y,
  mode,
  locale,
  networkColor,
  selected,
  layoutPinned,
  tabIndex,
  onOpenNode,
  onToggleNode,
  onShowHover,
  onHideHover,
  onShowMenu,
  onDragPreview,
  onCommitDrag,
  onCancelDrag,
  onMenuKey,
  onNavigate,
  onFocusNode,
  onLongPressStart,
  onLongPressMove,
  onLongPressEnd,
  consumeLongPress,
  setNodeRef,
}: TopologyNodeGlyphProps): JSX.Element {
  const segments = capabilitySegments(node, mode);
  const health = healthEncoding(node.health);
  const shared = node.networks.length > 1;
  const rawName = node.display_name?.trim() || node.node_id;
  const displayName =
    rawName.length > 19 ? `${rawName.slice(0, 18)}…` : rawName;
  const roleText = nodeRoleText(locale, node, undefined);
  const metaText = `${roleText} · ${healthLabel(locale, node.health)}`;
  const dragRef = useRef<{
    pointerId: number;
    startClientX: number;
    startClientY: number;
    startX: number;
    startY: number;
    active: boolean;
  } | null>(null);
  const suppressClickRef = useRef(false);
  const [dragging, setDragging] = useState(false);

  function pointerDeltaInSvg(
    event: ReactPointerEvent<SVGGElement>,
    startClientX: number,
    startClientY: number,
  ): TopologyNodePosition {
    const svg = event.currentTarget.ownerSVGElement;
    const matrix = svg?.getScreenCTM?.();
    if (!svg || !matrix || typeof svg.createSVGPoint !== "function") {
      return {
        x: event.clientX - startClientX,
        y: event.clientY - startClientY,
      };
    }
    try {
      const start = svg.createSVGPoint();
      start.x = startClientX;
      start.y = startClientY;
      const current = svg.createSVGPoint();
      current.x = event.clientX;
      current.y = event.clientY;
      const inverse = matrix.inverse();
      const localStart = start.matrixTransform(inverse);
      const localCurrent = current.matrixTransform(inverse);
      return {
        x: localCurrent.x - localStart.x,
        y: localCurrent.y - localStart.y,
      };
    } catch {
      return {
        x: event.clientX - startClientX,
        y: event.clientY - startClientY,
      };
    }
  }

  function draggedPosition(
    event: ReactPointerEvent<SVGGElement>,
    drag: NonNullable<typeof dragRef.current>,
  ): TopologyNodePosition {
    const delta = pointerDeltaInSvg(
      event,
      drag.startClientX,
      drag.startClientY,
    );
    return { x: drag.startX + delta.x, y: drag.startY + delta.y };
  }

  function startDrag(event: ReactPointerEvent<SVGGElement>): void {
    onLongPressStart(event, node.node_id);
    if (event.pointerType !== "mouse" || event.button !== 0) return;
    dragRef.current = {
      pointerId: event.pointerId,
      startClientX: event.clientX,
      startClientY: event.clientY,
      startX: x,
      startY: y,
      active: false,
    };
    try {
      event.currentTarget.setPointerCapture?.(event.pointerId);
    } catch {
      // Pointer capture is an enhancement; pointerup still commits the drag.
    }
  }

  function moveDrag(event: ReactPointerEvent<SVGGElement>): void {
    onLongPressMove(event);
    const drag = dragRef.current;
    if (!drag || drag.pointerId !== event.pointerId) return;
    if (!drag.active) {
      if (
        Math.hypot(
          event.clientX - drag.startClientX,
          event.clientY - drag.startClientY,
        ) < 4
      ) {
        return;
      }
      drag.active = true;
      setDragging(true);
    }
    event.preventDefault();
    onDragPreview(node.node_id, draggedPosition(event, drag));
  }

  function finishDrag(
    event: ReactPointerEvent<SVGGElement>,
    commit: boolean,
  ): void {
    const drag = dragRef.current;
    if (!drag || drag.pointerId !== event.pointerId) return;
    dragRef.current = null;
    setDragging(false);
    if (drag.active) {
      suppressClickRef.current = true;
      if (commit) onCommitDrag(node.node_id, draggedPosition(event, drag));
      else onCancelDrag();
    }
    try {
      if (event.currentTarget.hasPointerCapture?.(event.pointerId)) {
        event.currentTarget.releasePointerCapture(event.pointerId);
      }
    } catch {
      // The browser may already have released capture during pointer cancel.
    }
  }

  function openOrToggle(event: ReactMouseEvent<SVGGElement>): void {
    if (suppressClickRef.current) {
      suppressClickRef.current = false;
      event.preventDefault();
      return;
    }
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
      ref={(element) => setNodeRef(node.node_id, element)}
      transform={`translate(${x} ${y})`}
      className={`topology-node ${node.is_self ? "self" : ""} ${node.health} ${selected ? "selected" : ""} ${dragging ? "dragging" : ""}`}
      data-node-id={node.node_id}
      data-selected={selected ? "true" : "false"}
      data-layout-pinned={layoutPinned ? "true" : "false"}
      data-dragging={dragging ? "true" : "false"}
      tabIndex={tabIndex}
      role="option"
      aria-selected={selected}
      aria-posinset={itemIndex + 1}
      aria-setsize={itemCount}
      aria-keyshortcuts="Enter Space Shift+F10 ContextMenu"
      aria-label={ariaLabel}
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
      onPointerDown={startDrag}
      onPointerMove={moveDrag}
      onPointerUp={(event) => {
        onLongPressEnd();
        finishDrag(event, true);
      }}
      onPointerCancel={(event) => {
        onLongPressEnd();
        finishDrag(event, false);
      }}
      onPointerLeave={onLongPressEnd}
      onContextMenu={(event) => {
        event.preventDefault();
        onShowMenu(node.node_id, event.clientX, event.clientY, { x, y });
      }}
    >
      <title>{ariaLabel}</title>
      <rect
        x="-82"
        y="-32"
        width="164"
        height="64"
        rx="13"
        className="topology-node-card"
        stroke={networkColor}
      />
      {selected ? (
        <g className="topology-node-selection-mark" transform="translate(-74 -23)">
          <circle r="8" />
          <text x="0" y="2.6" textAnchor="middle">
            ✓
          </text>
        </g>
      ) : null}
      <g className="topology-node-status" transform="translate(-59 -10)">
        {segments.map((segment) => (
          <path
            key={segment.capability}
            d={arcPath(
              0,
              0,
              NODE_STATUS_RADIUS,
              segment.startAngle,
              segment.endAngle,
            )}
            fill="none"
            stroke={segment.enabled ? segment.color : "var(--border)"}
            strokeWidth={segment.enabled ? 3 : 1.5}
            strokeDasharray={segment.enabled ? undefined : "2 2"}
            strokeLinecap="round"
            className={
              segment.enabled ? "capability-enabled" : "capability-disabled"
            }
          />
        ))}
        <circle r="5" className={`topology-node-core ${health.tone}`} />
      </g>
      <text x="-38" y="-6" className="topology-node-name">
        {displayName}
      </text>
      <text x="-38" y="14" className="topology-node-meta">
        {metaText}
      </text>
      <HealthBadge encoding={health} x={59} y={-20} />
      {shared ? (
        <g className="topology-node-shared" transform="translate(59 18)">
          <title>
            {label(locale, "topology.node.networks_count").replace(
              "{count}",
              String(node.networks.length),
            )}
          </title>
          <circle r="8" />
          <text x="0" y="2.7" textAnchor="middle">
            ×{node.networks.length}
          </text>
        </g>
      ) : node.is_self ? (
        <text x="59" y="21" textAnchor="middle" className="topology-node-self-tag">
          {label(locale, "topology.list.self")}
        </text>
      ) : null}
    </g>
  );
}, areTopologyNodeGlyphPropsEqual);

function areTopologyNodeGlyphPropsEqual(
  previous: TopologyNodeGlyphProps,
  next: TopologyNodeGlyphProps,
): boolean {
  return (
    previous.itemIndex === next.itemIndex &&
    previous.itemCount === next.itemCount &&
    previous.ariaLabel === next.ariaLabel &&
    previous.x === next.x &&
    previous.y === next.y &&
    previous.mode === next.mode &&
    previous.locale === next.locale &&
    previous.networkColor === next.networkColor &&
    previous.selected === next.selected &&
    previous.layoutPinned === next.layoutPinned &&
    previous.tabIndex === next.tabIndex &&
    previous.onOpenNode === next.onOpenNode &&
    previous.onToggleNode === next.onToggleNode &&
    previous.onShowHover === next.onShowHover &&
    previous.onHideHover === next.onHideHover &&
    previous.onShowMenu === next.onShowMenu &&
    previous.onDragPreview === next.onDragPreview &&
    previous.onCommitDrag === next.onCommitDrag &&
    previous.onCancelDrag === next.onCancelDrag &&
    previous.onMenuKey === next.onMenuKey &&
    previous.onNavigate === next.onNavigate &&
    previous.onFocusNode === next.onFocusNode &&
    previous.onLongPressStart === next.onLongPressStart &&
    previous.onLongPressMove === next.onLongPressMove &&
    previous.onLongPressEnd === next.onLongPressEnd &&
    previous.consumeLongPress === next.consumeLongPress &&
    previous.setNodeRef === next.setNodeRef
  );
}

function ListView({
  topology,
  index,
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
  index: TopologyIndex;
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
  const rows = useMemo(
    () => topologyListRows(topology, mode, index),
    [index, mode, topology],
  );
  const nodes = index.nodesById;
  const networkNames = useMemo(
    () =>
      new Map(
        [...index.boundariesById.values()].map((boundary) => [
          boundary.network_id,
          boundary.name,
        ]),
      ),
    [index],
  );
  const rowIndexById = useMemo(
    () => new Map(rows.map((row, rowIndex) => [row.nodeId, rowIndex])),
    [rows],
  );
  const scrollRef = useRef<HTMLDivElement>(null);
  // TanStack Virtual intentionally exposes non-memoizable callbacks. Keep the
  // virtualizer scoped here so it never crosses a memoized component boundary.
  // eslint-disable-next-line react-hooks/incompatible-library
  const virtualizer = useVirtualizer({
    count: rows.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => 72,
    overscan: 8,
    initialRect: { width: 840, height: 420 },
  });
  const rowRefs = useRef(new Map<string, HTMLTableRowElement>());
  const focusedRowIdRef = useRef<string | null>(null);
  const pendingFocusRowIdRef = useRef<string | null>(null);
  const focusRequestTokenRef = useRef(0);
  const focusTimerRef = useRef<number | null>(null);
  const [activeRowId, setActiveRowId] = useState<string | null>(
    () => rows[0]?.nodeId ?? null,
  );
  const resolvedActiveRowId = rows.some((row) => row.nodeId === activeRowId)
    ? activeRowId
    : rows[0]?.nodeId ?? null;
  const longPress = useTouchLongPress(onShowMenu);
  const focusTargetNodeRef = useRef(focusTargetNodeId);
  const tryFocusPendingRow = useCallback((): boolean => {
    const nodeId = pendingFocusRowIdRef.current;
    if (!nodeId) return true;
    const element = rowRefs.current.get(nodeId);
    if (!element) return false;
    pendingFocusRowIdRef.current = null;
    if (focusTimerRef.current !== null) {
      window.clearTimeout(focusTimerRef.current);
      focusTimerRef.current = null;
    }
    element.focus({ preventScroll: true });
    element.scrollIntoView?.({ block: "center", inline: "center" });
    return true;
  }, []);
  const focusRow = useCallback(
    (nodeId: string): void => {
      const rowIndex = rowIndexById.get(nodeId);
      if (rowIndex === undefined) return;
      const requestToken = ++focusRequestTokenRef.current;
      pendingFocusRowIdRef.current = nodeId;
      setActiveRowId(nodeId);
      focusedRowIdRef.current = nodeId;
      if (focusTimerRef.current !== null) {
        window.clearTimeout(focusTimerRef.current);
        focusTimerRef.current = null;
      }
      virtualizer.scrollToIndex(rowIndex, { align: "center" });
      const focusWhenMounted = (attempt: number): void => {
        if (focusRequestTokenRef.current !== requestToken) return;
        if (tryFocusPendingRow() || attempt >= 30) {
          if (attempt >= 30) pendingFocusRowIdRef.current = null;
          return;
        }
        focusTimerRef.current = window.setTimeout(
          () => focusWhenMounted(attempt + 1),
          16,
        );
      };
      focusWhenMounted(0);
    },
    [rowIndexById, tryFocusPendingRow, virtualizer],
  );

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
    if (resolvedActiveRowId) focusRow(resolvedActiveRowId);
  }, [focusRow, resolvedActiveRowId, rows]);

  useEffect(() => {
    if (focusRequest === 0) return;
    const nodeId = focusTargetNodeRef.current;
    if (!nodeId) return;
    focusRow(nodeId);
  }, [focusRequest, focusRow]);

  useEffect(
    () => () => {
      focusRequestTokenRef.current += 1;
      pendingFocusRowIdRef.current = null;
      if (focusTimerRef.current !== null) {
        window.clearTimeout(focusTimerRef.current);
      }
    },
    [],
  );

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
      const currentIndex = rowIndexById.get(nodeId) ?? 0;
      let nextIndex = 0;
      if (event.key === "End") nextIndex = rows.length - 1;
      else if (event.key === "ArrowRight" || event.key === "ArrowDown") {
        nextIndex = (currentIndex + 1) % rows.length;
      } else if (event.key === "ArrowLeft" || event.key === "ArrowUp") {
        nextIndex = (currentIndex - 1 + rows.length) % rows.length;
      }
      const nextNodeId = rows[nextIndex]?.nodeId;
      if (!nextNodeId) return;
      focusRow(nextNodeId);
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

  const measuredVirtualRows = virtualizer.getVirtualItems();
  const virtualRows =
    measuredVirtualRows.length > 0
      ? measuredVirtualRows
      : rows.slice(0, 12).map((row, rowIndex) => ({
          index: rowIndex,
          key: row.nodeId,
          start: rowIndex * 72,
          size: 72,
          end: (rowIndex + 1) * 72,
        }));
  useEffect(() => {
    tryFocusPendingRow();
  }, [tryFocusPendingRow, virtualRows]);

  const firstVirtualRow = virtualRows[0];
  const lastVirtualRow = virtualRows[virtualRows.length - 1];
  const topSpacer = firstVirtualRow?.start ?? 0;
  const bottomSpacer = lastVirtualRow
    ? Math.max(0, virtualizer.getTotalSize() - lastVirtualRow.end)
    : 0;

  return (
    <div
      ref={scrollRef}
      className="table-shell topology-list-table"
      id="topology-object-navigation"
      data-rendered-rows={virtualRows.length}
    >
      <table aria-rowcount={rows.length + 1}>
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
          <tr
            className="topology-list-spacer"
            role="presentation"
            aria-hidden="true"
          >
            <td colSpan={6} style={{ height: `${topSpacer}px` }} />
          </tr>
          {virtualRows.map((virtualRow) => {
            const row = rows[virtualRow.index];
            if (!row) return null;
            const node = nodes.get(row.nodeId);
            if (!node) return null;
            const selected = selectedNodeIds.has(row.nodeId);
            return (
              <tr
                key={row.nodeId}
                ref={(element) => {
                  if (element) rowRefs.current.set(row.nodeId, element);
                  else rowRefs.current.delete(row.nodeId);
                  virtualizer.measureElement(element);
                }}
                data-index={virtualRow.index}
                data-node-id={row.nodeId}
                data-selected={selected ? "true" : "false"}
                className={selected ? "selected" : undefined}
                aria-selected={selected}
                aria-rowindex={virtualRow.index + 2}
                aria-keyshortcuts="ArrowLeft ArrowRight ArrowUp ArrowDown Home End Enter Space Shift+F10 ContextMenu"
                aria-label={nodeAriaLabel(locale, node, networkNames, index)}
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
          <tr
            className="topology-list-spacer"
            role="presentation"
            aria-hidden="true"
          >
            <td colSpan={6} style={{ height: `${bottomSpacer}px` }} />
          </tr>
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
  presentationSignals,
  filters,
  onFiltersChange,
  selectedNodeIds = EMPTY_SELECTED_NODE_IDS,
  onToggleNode,
  onOpenNode,
  networkId = null,
  membersByNodeId = new Map<string, NetworkMember>(),
  lifecycleActions,
  onPrepareLifecycle,
  onClearSelection,
  onSelectNetwork,
  selectionNotice = "",
}: Props): JSX.Element {
  const presentation = topologyPresentation(topology, presentationSignals);
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
  const [layoutPersistence, setLayoutPersistence] = useState(() =>
    loadPersistedTopologyPositions(),
  );
  const [internalFilters, setInternalFilters] = useState<TopologyFilters>({
    ...EMPTY_TOPOLOGY_FILTERS,
  });
  const [hover, setHover] = useState<HoverState | null>(null);
  const [menu, setMenu] = useState<NodeMenuState | null>(null);
  const menuOpenRef = useRef(false);
  const menuRef = useRef<HTMLDivElement>(null);
  const menuNodeIdRef = useRef<string | null>(null);
  const activeFilters = filters ?? internalFilters;
  const fixedPositions = layoutPersistence.positions;
  const layoutStorageStatus = layoutPersistence.status;
  const deferredFilters = useDeferredValue(activeFilters);
  const sourceTopology = frozenTopology ?? topology;
  const sourceIndex = useMemo(
    () => (sourceTopology ? cachedTopologyIndex(sourceTopology) : null),
    [sourceTopology],
  );
  const filtered = useMemo(
    () =>
      sourceTopology && sourceIndex
        ? filteredTopologySnapshot(
            sourceTopology,
            deferredFilters,
            sourceIndex,
          )
        : null,
    [deferredFilters, sourceIndex, sourceTopology],
  );
  const visible = filtered ?? sourceTopology;
  const displayed = visible ?? sourceTopology;
  const displayedIndex = useMemo(() => {
    if (!displayed) return null;
    if (displayed === sourceTopology && sourceIndex) return sourceIndex;
    return createTopologyIndex(displayed, sourceIndex);
  }, [displayed, sourceIndex, sourceTopology]);
  const focusTargetNodeId =
    visible?.nodes.find((node) => node.is_self)?.node_id ??
    visible?.nodes[0]?.node_id ??
    null;
  const visibleTopologyRef = useRef(visible);

  useEffect(() => {
    visibleTopologyRef.current = visible;
  }, [visible]);

  useEffect(() => {
    menuOpenRef.current = menu !== null;
  }, [menu]);

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

  const showHover = useCallback((nodeId: string, rect: DOMRect): void => {
    setHover((current) => {
      if (menuOpenRef.current) return current;
      const position = rectPosition(rect);
      const clamped = clampOverlayPosition(
        position.x + 14,
        position.y + 14,
        340,
        190,
      );
      return { nodeId, x: clamped.x, y: clamped.y };
    });
  }, []);

  const hideHover = useCallback((): void => setHover(null), []);

  const showMenu = useCallback(
    (
      nodeId: string,
      x: number,
      y: number,
      layoutPosition?: TopologyNodePosition,
    ): void => {
      setHover(null);
      const clamped = clampOverlayPosition(x, y, 270, 360);
      menuNodeIdRef.current = nodeId;
      setMenu({ nodeId, x: clamped.x, y: clamped.y, layoutPosition });
    },
    [],
  );

  const pinNodePosition = useCallback(
    (nodeId: string, position: TopologyNodePosition): void => {
      const next = new Map(fixedPositions);
      next.set(nodeId, position);
      const persisted = savePersistedTopologyPositions(next);
      setLayoutPersistence({
        positions: next,
        status: persisted ? "loaded" : "unavailable",
      });
    },
    [fixedPositions],
  );

  function resetLayout(): void {
    const cleared = clearPersistedTopologyPositions();
    setLayoutPersistence({
      positions: new Map<string, TopologyNodePosition>(),
      status: cleared ? "empty" : "unavailable",
    });
  }

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

  if (!presentation.canRender || !sourceTopology) {
    return (
      <div
        className="topology-state-shell"
        data-topology-state={presentation.kind}
      >
        <TopologyPresentationNotices
          presentation={presentation}
          locale={locale}
        />
      </div>
    );
  }
  if (sourceTopology.boundaries.length === 0) {
    return (
      <div className="topology-state-shell" data-topology-state="empty">
        <TopologyPresentationNotices
          presentation={topologyPresentation(sourceTopology)}
          locale={locale}
        />
      </div>
    );
  }

  if (!displayed || !displayedIndex) {
    return (
      <div className="empty-state">
        {label(locale, "topology.empty.unavailable")}
      </div>
    );
  }
  const hoverNode = hover ? sourceIndex?.nodesById.get(hover.nodeId) : null;
  const menuNode = menu ? sourceIndex?.nodesById.get(menu.nodeId) : null;
  const selectionCount = [...selectedNodeIds].filter((nodeId) =>
    sourceIndex?.nodesById.has(nodeId),
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
      data-layout-fixed-count={fixedPositions.size}
      data-layout-pinned={fixedPositions.size > 0 ? "true" : "false"}
      data-layout-storage-status={layoutStorageStatus}
      data-topology-state={presentation.kind}
      data-view={view}
    >
      <TopologyPresentationNotices
        presentation={presentation}
        locale={locale}
      />
      <div className="topology-stage">
        <div
          className="topology-floating-toolbar"
          role="group"
          aria-label={label(locale, "topology.navigation.label")}
        >
          <button
            type="button"
            className="topology-icon-button"
            aria-label={label(locale, "topology.zoom.out")}
            title={label(locale, "topology.zoom.out")}
            disabled={view !== "graph" || zoomIndex <= 0}
            onClick={() => zoomBy(-1)}
          >
            <Minus size={16} aria-hidden="true" />
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
            className="topology-icon-button"
            aria-label={label(locale, "topology.zoom.in")}
            title={label(locale, "topology.zoom.in")}
            disabled={
              view !== "graph" || zoomIndex === ZOOM_LEVELS.length - 1
            }
            onClick={() => zoomBy(1)}
          >
            <Plus size={16} aria-hidden="true" />
          </button>
          <button
            type="button"
            className="topology-icon-button"
            aria-label={label(locale, "topology.zoom.reset")}
            title={label(locale, "topology.zoom.reset")}
            disabled={view !== "graph" || zoom === 1}
            onClick={() => setZoom(1)}
          >
            <RotateCcw size={15} aria-hidden="true" />
          </button>
          <button
            type="button"
            className="topology-icon-button"
            aria-label={label(locale, "topology.locate.current")}
            title={label(locale, "topology.locate.current")}
            disabled={!focusTargetNodeId}
            onClick={locateCurrentNode}
          >
            <LocateFixed size={15} aria-hidden="true" />
          </button>
        </div>

        <div
          className="topology-stage-actions"
          role="group"
          aria-label={label(locale, "topology.view.label")}
        >
          <div className="topology-view-switch" role="group">
            <button
              type="button"
              aria-label={label(locale, "topology.view.graph")}
              aria-pressed={view === "graph"}
              title={label(locale, "topology.view.graph")}
              onClick={() => changeView("graph")}
            >
              <Network size={16} aria-hidden="true" />
            </button>
            <button
              type="button"
              aria-label={label(locale, "topology.view.list")}
              aria-pressed={view === "list"}
              title={label(locale, "topology.view.list")}
              onClick={() => changeView("list")}
            >
              <List size={16} aria-hidden="true" />
            </button>
          </div>
          <details className="topology-options">
            <summary
              className="topology-icon-button"
              aria-label={label(locale, "topology.view.label")}
            >
              <MoreHorizontal size={17} aria-hidden="true" />
            </summary>
            <div className="topology-options-panel">
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
              </div>
              <label className="topology-contrast-toggle">
                <input
                  type="checkbox"
                  checked={highContrast}
                  onChange={(event) =>
                    setHighContrast(event.currentTarget.checked)
                  }
                />
                <span>{label(locale, "topology.mode.high_contrast")}</span>
              </label>
              <div className="topology-option-actions">
                <button
                  type="button"
                  className="topology-control-button topology-control-wide"
                  disabled={view !== "graph" || fixedPositions.size === 0}
                  onClick={resetLayout}
                >
                  <PinOff size={15} aria-hidden="true" />
                  <span>{label(locale, "topology.layout.reset")}</span>
                </button>
                <button
                  type="button"
                  className="topology-control-button topology-control-wide"
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
              </div>
              <FilterToolbar
                topology={sourceTopology}
                locale={locale}
                filters={activeFilters}
                visibleCount={displayed.nodes.length}
                onChange={changeFilters}
              />
              {layoutStorageStatus === "unavailable" ? (
                <p className="topology-layout-status warning" role="status">
                  {label(locale, "topology.layout.storage_unavailable")}
                </p>
              ) : layoutStorageStatus === "invalid" ||
                layoutStorageStatus === "unsupported_version" ? (
                <p className="topology-layout-status warning" role="status">
                  {label(locale, "topology.layout.storage_invalid")}
                </p>
              ) : fixedPositions.size > 0 ? (
                <p className="topology-layout-status" role="status">
                  {tr(locale, "topology.layout.pinned", {
                    count: String(fixedPositions.size),
                  })}
                </p>
              ) : null}
            </div>
          </details>
        </div>

        {displayed.nodes.length === 0 ? (
          <div className="empty-state">
            {label(locale, "topology.filters.empty")}
          </div>
        ) : view === "graph" ? (
          <GraphView
            topology={displayed}
            index={displayedIndex}
            locale={locale}
            mode={mode}
            zoom={zoom}
            fixedPositions={fixedPositions}
            focusRequest={focusRequest}
            focusTargetNodeId={focusTargetNodeId}
            selectedNodeIds={selectedNodeIds}
            onOpenNode={onOpenNode}
            onToggleNode={onToggleNode}
            onPinNode={pinNodePosition}
            onShowHover={showHover}
            onHideHover={hideHover}
            onShowMenu={showMenu}
          />
        ) : (
          <ListView
            topology={displayed}
            index={displayedIndex}
            locale={locale}
            mode={mode}
            focusRequest={focusRequest}
            focusTargetNodeId={focusTargetNodeId}
            selectedNodeIds={selectedNodeIds}
            onOpenNode={onOpenNode}
            onToggleNode={onToggleNode}
            onShowHover={showHover}
            onHideHover={hideHover}
            onShowMenu={showMenu}
          />
        )}
        <footer className="topology-canvas-footer">
          <div className="topology-network-chips">
            {displayed.boundaries.map((boundary) => (
              <button
                type="button"
                className={
                  boundary.network_id === networkId ? "active" : undefined
                }
                aria-pressed={boundary.network_id === networkId}
                key={boundary.network_id}
                onClick={() => onSelectNetwork?.(boundary.network_id)}
              >
                <span aria-hidden="true" />
                {boundary.name}
              </button>
            ))}
          </div>
          <Legend topology={displayed} locale={locale} mode={mode} />
        </footer>
      </div>
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
          layoutPinned={fixedPositions.has(menuNode.node_id)}
          layoutPosition={menu.layoutPosition}
          x={menu.x}
          y={menu.y}
          menuRef={menuRef}
          onOpen={(nodeId) => onOpenNode?.(nodeId)}
          onToggle={(nodeId) => onToggleNode?.(nodeId)}
          onPinLayout={pinNodePosition}
          onPrepareLifecycle={(nodeId, action) =>
            onPrepareLifecycle?.(nodeId, action)
          }
          onClose={closeMenu}
        />
      ) : null}
    </div>
  );
}
