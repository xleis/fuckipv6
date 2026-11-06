import {
  useEffect,
  useMemo,
  useRef,
  useState,
  type JSX,
  type KeyboardEvent as ReactKeyboardEvent,
  type MouseEvent as ReactMouseEvent,
} from "react";
import { Search, ShieldCheck, X } from "lucide-react";
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

function nodeAriaLabel(
  locale: Locale,
  node: TopologyNode,
  networkNames: Map<string, string>,
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
  return `${nodeName(node)} · ${healthLabel(locale, node.health)} · ${capabilities} · ${networks}`;
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
  onClose(): void;
}) {
  return (
    <div
      ref={menuRef}
      className="topology-node-menu"
      role="menu"
      aria-label={tr(locale, "topology.menu.label", { name: nodeName(node) })}
      style={{ left: x, top: y }}
      data-testid="topology-node-menu"
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
          onClose();
          onOpen(node.node_id);
        }}
      >
        {label(locale, "topology.menu.open")}
      </button>
      <button
        type="button"
        role="menuitem"
        onClick={() => {
          onClose();
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
              onClose();
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
      <span className="topology-filter-count">
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
    <div className="topology-canvas">
      <svg
        viewBox={`0 0 ${layout.width} ${layout.height}`}
        role="group"
        aria-labelledby="topology-svg-title"
        aria-describedby="topology-svg-desc"
        data-color-mode={mode}
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
        >
          {topology.nodes.map((node) => {
            const placement = placements.get(node.node_id);
            if (!placement) return null;
            const visual = visuals.get(placement.anchorNetworkId);
            return (
              <TopologyNodeGlyph
                key={node.node_id}
                node={node}
                x={placement.x}
                y={placement.y}
                mode={mode}
                locale={locale}
                networkNames={boundaryNames}
                networkColor={visual?.color ?? "#6b7280"}
                selected={selectedNodeIds.has(node.node_id)}
                onOpenNode={onOpenNode}
                onToggleNode={onToggleNode}
                onShowHover={onShowHover}
                onHideHover={onHideHover}
                onShowMenu={onShowMenu}
                onMenuKey={menuFromFocus}
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
  node,
  x,
  y,
  mode,
  locale,
  networkNames,
  networkColor,
  selected,
  onOpenNode,
  onToggleNode,
  onShowHover,
  onHideHover,
  onShowMenu,
  onMenuKey,
}: {
  node: TopologyNode;
  x: number;
  y: number;
  mode: TopologyColorMode;
  locale: Locale;
  networkNames: Map<string, string>;
  networkColor: string;
  selected: boolean;
  onOpenNode?: (nodeId: string) => void;
  onToggleNode?: (nodeId: string) => void;
  onShowHover(nodeId: string, rect: DOMRect): void;
  onHideHover(): void;
  onShowMenu(nodeId: string, x: number, y: number): void;
  onMenuKey(event: ReactKeyboardEvent<SVGGElement>, nodeId: string): void;
}): JSX.Element {
  const segments = capabilitySegments(node, mode);
  const health = healthEncoding(node.health);
  const shared = node.networks.length > 1;

  function openOrToggle(event: ReactMouseEvent<SVGGElement>): void {
    if (event.ctrlKey || event.metaKey || event.shiftKey) {
      event.preventDefault();
      onToggleNode?.(node.node_id);
      return;
    }
    onOpenNode?.(node.node_id);
  }

  function onKeyDown(event: ReactKeyboardEvent<SVGGElement>): void {
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
      transform={`translate(${x} ${y})`}
      className={`topology-node ${node.is_self ? "self" : ""} ${node.health} ${selected ? "selected" : ""}`}
      data-node-id={node.node_id}
      data-selected={selected ? "true" : "false"}
      tabIndex={0}
      role="option"
      aria-selected={selected}
      aria-label={nodeAriaLabel(locale, node, networkNames)}
      onClick={openOrToggle}
      onKeyDown={onKeyDown}
      onMouseEnter={(event) =>
        onShowHover(node.node_id, event.currentTarget.getBoundingClientRect())
      }
      onMouseLeave={onHideHover}
      onFocus={(event) =>
        onShowHover(node.node_id, event.currentTarget.getBoundingClientRect())
      }
      onBlur={onHideHover}
      onContextMenu={(event) => {
        event.preventDefault();
        onShowMenu(node.node_id, event.clientX, event.clientY);
      }}
    >
      <title>{nodeAriaLabel(locale, node, networkNames)}</title>
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

  function handleRowKey(
    event: ReactKeyboardEvent<HTMLTableRowElement>,
    nodeId: string,
  ): void {
    if (event.target !== event.currentTarget) return;
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
    <div className="table-shell topology-list-table">
      <table>
        <caption className="sr-only">{label(locale, "topology.view.list")}</caption>
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
                data-node-id={row.nodeId}
                data-selected={selected ? "true" : "false"}
                className={selected ? "selected" : undefined}
                aria-selected={selected}
                tabIndex={0}
                onClick={(event) => {
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
                onFocus={(event) =>
                  onShowHover(row.nodeId, event.currentTarget.getBoundingClientRect())
                }
                onBlur={onHideHover}
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
  const [view, setView] = useState<"graph" | "list">("graph");
  const [mode, setMode] = useState<TopologyColorMode>("default");
  const [highContrast, setHighContrast] = useState(false);
  const [internalFilters, setInternalFilters] = useState<TopologyFilters>({
    ...EMPTY_TOPOLOGY_FILTERS,
  });
  const [hover, setHover] = useState<HoverState | null>(null);
  const [menu, setMenu] = useState<NodeMenuState | null>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  const activeFilters = filters ?? internalFilters;
  const filtered = useMemo(
    () => (topology ? filteredTopologySnapshot(topology, activeFilters) : null),
    [topology, activeFilters],
  );

  useEffect(() => {
    if (!menu) return;
    const focusTimer = window.setTimeout(() => {
      menuRef.current
        ?.querySelector<HTMLElement>('[role="menuitem"]:not([disabled])')
        ?.focus();
    }, 0);
    function closeOnOutside(event: PointerEvent): void {
      if (!menuRef.current?.contains(event.target as Node)) setMenu(null);
    }
    function closeOnEscape(event: KeyboardEvent): void {
      if (event.key !== "Escape") return;
      const nodeId = menu?.nodeId;
      setMenu(null);
      if (nodeId) {
        window.setTimeout(() => {
          document
            .querySelector<HTMLElement>(`[data-node-id="${nodeId}"]`)
            ?.focus();
        }, 0);
      }
    }
    document.addEventListener("pointerdown", closeOnOutside);
    document.addEventListener("keydown", closeOnEscape);
    return () => {
      window.clearTimeout(focusTimer);
      document.removeEventListener("pointerdown", closeOnOutside);
      document.removeEventListener("keydown", closeOnEscape);
    };
  }, [menu]);

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

  function showMenu(nodeId: string, x: number, y: number): void {
    setHover(null);
    const clamped = clampOverlayPosition(x, y, 270, 360);
    setMenu({ nodeId, x: clamped.x, y: clamped.y });
  }

  if (!topology) {
    return (
      <div className="empty-state">
        {label(locale, "topology.empty.unavailable")}
      </div>
    );
  }
  if (topology.boundaries.length === 0) {
    return (
      <div className="empty-state">
        {label(locale, "topology.empty.no_networks")}
      </div>
    );
  }

  const visible = filtered ?? topology;
  const hoverNode = hover
    ? topology.nodes.find((node) => node.node_id === hover.nodeId)
    : null;
  const menuNode = menu
    ? topology.nodes.find((node) => node.node_id === menu.nodeId)
    : null;
  const selectionCount = [...selectedNodeIds].filter((nodeId) =>
    topology.nodes.some((node) => node.node_id === nodeId),
  ).length;

  return (
    <div
      className={`topology-view ${highContrast ? "topology-high-contrast" : ""}`}
      data-color-mode={mode}
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
            onClick={() => setView("graph")}
          >
            {label(locale, "topology.view.graph")}
          </button>
          <button
            type="button"
            aria-pressed={view === "list"}
            onClick={() => setView("list")}
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
      </div>
      <FilterToolbar
        topology={topology}
        locale={locale}
        filters={activeFilters}
        visibleCount={visible.nodes.length}
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
      {visible.nodes.length === 0 ? (
        <div className="empty-state">{label(locale, "topology.filters.empty")}</div>
      ) : view === "graph" ? (
        <GraphView
          topology={visible}
          locale={locale}
          mode={mode}
          selectedNodeIds={selectedNodeIds}
          onOpenNode={onOpenNode}
          onToggleNode={onToggleNode}
          onShowHover={showHover}
          onHideHover={() => setHover(null)}
          onShowMenu={showMenu}
        />
      ) : (
        <ListView
          topology={visible}
          locale={locale}
          mode={mode}
          selectedNodeIds={selectedNodeIds}
          onOpenNode={onOpenNode}
          onToggleNode={onToggleNode}
          onShowHover={showHover}
          onHideHover={() => setHover(null)}
          onShowMenu={showMenu}
        />
      )}
      <Legend topology={visible} locale={locale} mode={mode} />
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
          onClose={() => setMenu(null)}
        />
      ) : null}
    </div>
  );
}
