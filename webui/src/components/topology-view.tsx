import { useMemo, useState, type JSX } from "react";
import type {
  CapabilityKind,
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

type Props = {
  topology: TopologySnapshot | null;
  locale: Locale;
};

const NODE_RING_RADIUS = 17;

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
      {encoding.shape === "square" && <rect x="-6" y="-6" width="12" height="12" rx="2" />}
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

function healthLabel(locale: Locale, health: TopologyNode["health"]): string {
  return label(locale, `topology.health.${health}`);
}

function nodeAriaLabel(
  locale: Locale,
  node: TopologyNode,
  networkNames: Map<string, string>,
): string {
  const capabilities =
    node.capabilities && node.capabilities.length > 0
      ? node.capabilities
          .map((capability) => label(locale, `capability.${capability}`))
          .join(" / ")
      : label(locale, "status.capability.unknown");
  const networks = node.networks
    .map(
      (network) =>
        `${networkNames.get(network.network_id) ?? network.network_id} (${label(
          locale,
          `join_request.role.${network.role}`,
        )})`,
    )
    .join(", ");
  return `${node.display_name?.trim() || node.node_id} · ${healthLabel(locale, node.health)} · ${capabilities} · ${networks}`;
}

function GraphView({
  topology,
  locale,
  mode,
}: {
  topology: TopologySnapshot;
  locale: Locale;
  mode: TopologyColorMode;
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
        <g role="list" aria-label={label(locale, "topology.aria.nodes")}>
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
}: {
  node: TopologyNode;
  x: number;
  y: number;
  mode: TopologyColorMode;
  locale: Locale;
  networkNames: Map<string, string>;
  networkColor: string;
}): JSX.Element {
  const segments = capabilitySegments(node, mode);
  const health = healthEncoding(node.health);
  const shared = node.networks.length > 1;
  return (
    <g
      transform={`translate(${x} ${y})`}
      className={`topology-node ${node.is_self ? "self" : ""} ${node.health}`}
      data-node-id={node.node_id}
      tabIndex={0}
      role="listitem"
      aria-label={nodeAriaLabel(locale, node, networkNames)}
    >
      <title>{nodeAriaLabel(locale, node, networkNames)}</title>
      <circle r="24" className="topology-node-halo" stroke={networkColor} />
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
      <HealthBadge
        encoding={health}
        x={14}
        y={-14}
      />
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

function HealthBadgeInline({ health }: { health: TopologyNode["health"] }): JSX.Element {
  const encoding = healthEncoding(health);
  return (
    <span className={`topology-health-inline ${encoding.tone}`}>
      <svg viewBox="-8 -8 16 16" aria-hidden="true">
        {encoding.shape === "circle" && <circle r="7" />}
        {encoding.shape === "square" && <rect x="-6" y="-6" width="12" height="12" rx="2" />}
        {encoding.shape === "diamond" && <path d="M 0 -7 L 7 0 L 0 7 L -7 0 Z" />}
        {encoding.shape === "triangle" && <path d="M 0 -7 L 7 6 L -7 6 Z" />}
        <text x="0" y="0.5" textAnchor="middle" dominantBaseline="middle">
          {encoding.glyph}
        </text>
      </svg>
    </span>
  );
}

function ListView({
  topology,
  locale,
  mode,
}: {
  topology: TopologySnapshot;
  locale: Locale;
  mode: TopologyColorMode;
}): JSX.Element {
  const rows = useMemo(() => topologyListRows(topology, mode), [topology, mode]);
  return (
    <div className="table-shell topology-list-table">
      <table>
        <caption className="sr-only">
          {label(locale, "topology.view.list")}
        </caption>
        <thead>
          <tr>
            <th scope="col">{label(locale, "topology.list.name")}</th>
            <th scope="col">{label(locale, "topology.list.networks")}</th>
            <th scope="col">{label(locale, "topology.list.capabilities")}</th>
            <th scope="col">{label(locale, "topology.list.health")}</th>
            <th scope="col">{label(locale, "topology.list.connections")}</th>
          </tr>
        </thead>
        <tbody>
          {rows.map((row) => (
            <tr key={row.nodeId} data-node-id={row.nodeId}>
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
                      .map((capability) => label(locale, `capability.${capability}`))
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
          ))}
        </tbody>
      </table>
    </div>
  );
}

function Legend({ topology, locale, mode }: {
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
                  <svg viewBox="0 0 24 12" aria-hidden="true" className="topology-legend-line">
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
                <svg viewBox="0 0 24 12" aria-hidden="true" className="topology-legend-line">
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

export function TopologyView({ topology, locale }: Props): JSX.Element {
  const [view, setView] = useState<"graph" | "list">("graph");
  const [mode, setMode] = useState<TopologyColorMode>("default");
  const [highContrast, setHighContrast] = useState(false);
  if (!topology) {
    return <div className="empty-state">{label(locale, "topology.empty.unavailable")}</div>;
  }
  if (topology.boundaries.length === 0) {
    return <div className="empty-state">{label(locale, "topology.empty.no_networks")}</div>;
  }
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
        <div className="topology-view-switch" role="group" aria-label={label(locale, "topology.view.label")}>
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
        <div className="topology-color-switch" role="group" aria-label={label(locale, "topology.mode.label")}>
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
      {view === "graph" ? (
        <GraphView topology={topology} locale={locale} mode={mode} />
      ) : (
        <ListView topology={topology} locale={locale} mode={mode} />
      )}
      <Legend topology={topology} locale={locale} mode={mode} />
    </div>
  );
}
