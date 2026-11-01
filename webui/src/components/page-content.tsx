import * as ContextMenu from "@radix-ui/react-context-menu";
import { useVirtualizer } from "@tanstack/react-virtual";
import {
  flexRender,
  getCoreRowModel,
  useReactTable,
  type ColumnDef,
} from "@tanstack/react-table";
import {
  Activity,
  AlertTriangle,
  ArrowDownToLine,
  ArrowUpFromLine,
  Boxes,
  CheckCircle2,
  CircleDot,
  Cpu,
  Database,
  DoorOpen,
  Gauge,
  Globe2,
  KeyRound,
  Network,
  RadioTower,
  Server,
  ShieldCheck,
  Waypoints,
  Wifi,
  WifiOff,
  type LucideIcon,
} from "lucide-react";
import { useMemo, useRef, type JSX, type ReactNode } from "react";
import type {
  ActivityEntry,
  NodeLogEntry,
  NodeNetworkSnapshot,
  NodePeerSnapshot,
  NodeSnapshot,
  NodeUiPage,
} from "../../../contracts/typescript/node-contract-v1";
import {
  executionBlockReason,
  label,
  primaryStatus,
} from "../contract/invariants";
import { t, tr } from "../i18n";

type PageContentProps = {
  page: NodeUiPage;
  snapshot: NodeSnapshot;
  locale: Locale;
  selectedNetwork: string;
  onSelectNetwork(name: string): void;
  onNavigate(pageId: string): void;
  onRequestAction(actionId: string): void;
};

function empty(text: string): JSX.Element {
  return <div className="empty-state">{text}</div>;
}

function statusTone(value: string): string {
  if (["online", "running", "enabled", "available"].includes(value))
    return "good";
  if (["failed", "error", "unavailable", "unsupported"].includes(value))
    return "bad";
  if (["degraded", "connecting", "starting", "stopping"].includes(value))
    return "warn";
  return "neutral";
}

function NetworkContextMenu({
  network,
  locale,
  onSelectNetwork,
  onNavigate,
}: {
  network: NodeNetworkSnapshot;
  locale: Locale;
  onSelectNetwork(name: string): void;
  onNavigate(pageId: string): void;
}) {
  return (
    <ContextMenu.Root>
      <ContextMenu.Trigger asChild>
        <article className={`network-card ${statusTone(network.status)}`}>
          <div className="network-card-head">
            <div className="network-icon">
              <Network size={18} />
            </div>
            <div className="network-name">
              <strong>{network.name}</strong>
              <span>{network.tun_name}</span>
            </div>
            <span className={`status-pill ${statusTone(network.status)}`}>
              {t(locale, `status.network.${network.status}`)}
            </span>
          </div>
          <div className="network-metrics">
            <div>
              <span>{label(locale, "common.node_navigation")}</span>
              <strong>{network.peer_count}</strong>
            </div>
            <div>
              <span>{formatDuration(locale, network.uptime_secs)}</span>
              <strong>{network.roles.join(" · ") || "member"}</strong>
            </div>
          </div>
          <div className="network-addresses">
            <code>{network.assigned_ipv4 ?? "IPv4 —"}</code>
            <code>{network.assigned_ipv6 ?? "IPv6 —"}</code>
          </div>
        </article>
      </ContextMenu.Trigger>
      <ContextMenu.Portal>
        <ContextMenu.Content className="context-menu">
          <ContextMenu.Item
            className="context-item"
            onSelect={() => {
              onSelectNetwork(network.name);
              onNavigate("peers");
            }}
          >
            {label(locale, "nav.peers")}
          </ContextMenu.Item>
          <ContextMenu.Item
            className="context-item"
            onSelect={() => {
              onSelectNetwork(network.name);
              onNavigate("routing");
            }}
          >
            {label(locale, "nav.routing")}
          </ContextMenu.Item>
          <ContextMenu.Item
            className="context-item"
            onSelect={() => {
              onSelectNetwork(network.name);
              onNavigate("traffic");
            }}
          >
            {label(locale, "nav.traffic")}
          </ContextMenu.Item>
        </ContextMenu.Content>
      </ContextMenu.Portal>
    </ContextMenu.Root>
  );
}

const OVERVIEW_ACTIVITY_ACTIONS = new Set([
  "network.create",
  "network.join",
  "network.start",
  "network.stop",
  "network.restart",
  "routing.update",
  "network.role.set",
  "network.blacklist.ban",
  "network.blacklist.unban",
  "network.allocation.update",
  "peer.probe",
  "node.start",
  "node.stop",
  "node.restart",
  "node.cleanup_residuals",
  "node.shutdown",
]);

function overviewPrimaryAction(
  statusId: string,
): { id: string; labelKey: string } | null {
  if (statusId === "uninitialized" || statusId === "stopped") {
    return { id: "start_node", labelKey: "action.start_node" };
  }
  if (statusId === "running") {
    return { id: "stop_node", labelKey: "action.stop_node" };
  }
  if (["degraded", "failed", "control_unavailable"].includes(statusId)) {
    return {
      id: "get_node_diagnostics",
      labelKey: "action.get_node_diagnostics",
    };
  }
  return null;
}

function overviewIssues(snapshot: NodeSnapshot, locale: Locale): string[] {
  const issues: Array<{ key: string; text: string }> = [];
  const seen = new Set<string>();
  const add = (key: string, text: string) => {
    if (seen.has(key)) return;
    seen.add(key);
    issues.push({ key, text });
  };

  for (const network of snapshot.lifecycle.failed_networks) {
    add(
      `network:${network}`,
      tr(locale, "overview.issue.network", {
        network,
        status: label(locale, "status.network.failed"),
      }),
    );
  }

  if (snapshot.last_error) {
    const problem = tr(
      locale,
      snapshot.last_error.message_key,
      snapshot.last_error.params ?? {},
    );
    add(
      "last_error",
      problem && problem !== snapshot.last_error.message_key
        ? problem
        : label(locale, "status.primary.degraded.detail"),
    );
  }

  for (const network of snapshot.networks) {
    if (
      !["connecting", "degraded", "stopping", "failed"].includes(network.status)
    )
      continue;
    add(
      `network:${network.name}`,
      tr(locale, "overview.issue.network", {
        network: network.name,
        status: label(locale, `status.network.${network.status}`),
      }),
    );
  }

  for (const service of snapshot.embedded_services ?? []) {
    if (service.status !== "failed") continue;
    add(
      `service:${service.network_name}`,
      tr(locale, "overview.issue.service", {
        network: service.network_name,
        status: label(locale, "status.service.failed"),
      }),
    );
  }

  return issues.slice(0, 3).map((issue) => issue.text);
}

function activityTitle(entry: ActivityEntry, locale: Locale): string {
  return label(
    locale,
    OVERVIEW_ACTIVITY_ACTIONS.has(entry.action)
      ? `activity.action.${entry.action}`
      : "activity.action.node.command",
  );
}

function recentOverviewActivity(
  snapshot: NodeSnapshot,
  locale: Locale,
): Array<{
  id: string;
  title: string;
  detail: string;
  command?: string;
}> {
  return [...(snapshot.recent_activity ?? [])]
    .sort((left, right) => right.occurred_at_unix_ms - left.occurred_at_unix_ms)
    .slice(0, 5)
    .map((entry) => ({
      id: entry.audit_id,
      title: activityTitle(entry, locale),
      detail: tr(locale, "overview.activity.detail", {
        outcome: label(locale, `activity.outcome.${entry.outcome}`),
        resource: entry.resource,
      }),
      command: entry.details.command,
    }));
}

function Overview({
  page,
  snapshot,
  locale,
  selectedNetwork,
  onSelectNetwork,
  onNavigate,
  onRequestAction,
}: PageContentProps) {
  const status = primaryStatus(snapshot, locale);
  const primaryAction = overviewPrimaryAction(status.id);
  const issues = overviewIssues(snapshot, locale);
  const activity = recentOverviewActivity(snapshot, locale);
  const nodeName =
    snapshot.identity?.display_name ||
    snapshot.identity?.node_id ||
    label(locale, "overview.node_name_fallback");
  const availableActions = new Map(
    page.actions.map((action) => [action.id, action]),
  );
  const requestedPrimaryAction = primaryAction
    ? availableActions.get(primaryAction.id)
    : undefined;
  const executablePrimaryAction =
    requestedPrimaryAction &&
    !executionBlockReason(requestedPrimaryAction, snapshot, selectedNetwork)
      ? requestedPrimaryAction
      : undefined;
  const shortcut = (actionId: string) => {
    onNavigate("networks");
    onRequestAction(actionId);
  };

  return (
    <div className="page-stack overview-stack">
      <section className={`hero-status ${statusTone(status.id)}`}>
        <div className="hero-status-icon">
          {status.id === "running" ? (
            <CheckCircle2 size={30} />
          ) : status.id === "failed" ? (
            <AlertTriangle size={30} />
          ) : (
            <Activity size={30} />
          )}
        </div>
        <div className="hero-status-copy">
          <span>{label(locale, "common.primary_status")}</span>
          <h2>{nodeName}</h2>
          <p>
            <strong>{status.label}</strong> · {status.detail}
          </p>
        </div>
        {executablePrimaryAction ? (
          <button
            className="primary-button overview-primary-action"
            type="button"
            onClick={() => onRequestAction(executablePrimaryAction.id)}
          >
            {label(locale, executablePrimaryAction.label_key)}
          </button>
        ) : null}
      </section>

      {issues.length > 0 ? (
        <section
          className="surface-card issues-card"
          data-testid="overview-issues"
        >
          <div className="card-heading">
            <h3>{label(locale, "overview.need_attention")}</h3>
            <AlertTriangle size={17} />
          </div>
          {issues.map((issue) => (
            <p className="issue-row" key={issue}>
              <AlertTriangle size={15} />
              <span>{issue}</span>
            </p>
          ))}
        </section>
      ) : null}

      <section className="surface-card" data-testid="overview-networks">
        <div className="card-heading">
          <h3>{label(locale, "overview.networks_title")}</h3>
          <span>{snapshot.networks.length}</span>
        </div>
        {snapshot.networks.length === 0
          ? empty(label(locale, "common.no_running_networks"))
          : snapshot.networks.map((network) => {
              const config = snapshot.configuration?.networks.find(
                (candidate) => candidate.network_name === network.name,
              );
              const routing = config
                ? label(locale, `choice.routing.${config.routing_mode}`)
                : "—";
              const exit = config?.exit_node
                ? tr(locale, "overview.exit.selected", {
                    node: config.exit_node,
                  })
                : label(locale, "overview.exit.none");
              return (
                <button
                  className="compact-row overview-network-row"
                  type="button"
                  key={network.name}
                  onClick={() => {
                    onSelectNetwork(network.name);
                    onNavigate("networks");
                  }}
                >
                  <CircleDot size={15} className={statusTone(network.status)} />
                  <span>
                    <strong>{network.name}</strong>
                    <small>
                      {tr(locale, "overview.network.detail", {
                        peers: network.peer_count,
                        tun: network.tun_name,
                      })}
                    </small>
                    <small>
                      {tr(locale, "overview.network.path", { routing, exit })}
                    </small>
                  </span>
                  <span className={`status-pill ${statusTone(network.status)}`}>
                    {label(locale, `status.network.${network.status}`)}
                  </span>
                </button>
              );
            })}
      </section>

      <section className="surface-card" data-testid="overview-activity">
        <div className="card-heading">
          <h3>{label(locale, "overview.activity_title")}</h3>
          <Activity size={17} />
        </div>
        {activity.length === 0
          ? empty(label(locale, "overview.activity.empty"))
          : activity.map((entry) => (
              <div
                className="overview-activity-row"
                key={entry.id}
                data-activity-action={entry.title}
              >
                <CircleDot size={14} />
                <span>
                  <strong>{entry.title}</strong>
                  <small>{entry.detail}</small>
                  {entry.command ? <code>{entry.command}</code> : null}
                </span>
              </div>
            ))}
      </section>

      <section className="surface-card" data-testid="overview-shortcuts">
        <div className="card-heading">
          <h3>{label(locale, "overview.shortcuts")}</h3>
          <Waypoints size={17} />
        </div>
        <div className="overview-shortcuts">
          <button
            className="secondary-button"
            type="button"
            onClick={() => shortcut("create_network")}
          >
            {label(locale, "action.create_network")}
          </button>
          <button
            className="secondary-button"
            type="button"
            disabled
            title={label(locale, "shortcut.discovery_unavailable")}
          >
            <span>{label(locale, "shortcut.discover_networks")}</span>
            <small>{label(locale, "shortcut.discovery_unavailable")}</small>
          </button>
          <button
            className="secondary-button"
            type="button"
            onClick={() => shortcut("join_network")}
          >
            {label(locale, "action.join_network")}
          </button>
        </div>
      </section>
    </div>
  );
}

function Networks({
  snapshot,
  locale,
  onSelectNetwork,
  onNavigate,
}: Omit<PageContentProps, "page">) {
  if (snapshot.networks.length === 0)
    return empty(label(locale, "common.no_running_networks"));
  return (
    <div className="network-grid">
      {snapshot.networks.map((network) => (
        <NetworkContextMenu
          key={network.name}
          network={network}
          locale={locale}
          onSelectNetwork={onSelectNetwork}
          onNavigate={onNavigate}
        />
      ))}
    </div>
  );
}

function Peers({
  snapshot,
  locale,
}: Omit<
  PageContentProps,
  "page" | "selectedNetwork" | "onSelectNetwork" | "onNavigate"
>) {
  const columns = useMemo<ColumnDef<NodePeerSnapshot>[]>(
    () => [
      {
        header: label(locale, "field.peer_target"),
        accessorKey: "peer_id",
        cell: (cell) => <code>{String(cell.getValue())}</code>,
      },
      {
        header: label(locale, "field.network_name"),
        accessorKey: "network_name",
      },
      {
        header: "IPv6",
        accessorKey: "virtual_ipv6",
        cell: (cell) => <code>{String(cell.getValue())}</code>,
      },
      {
        header: label(locale, "common.runtime_status"),
        accessorKey: "online",
        cell: (cell) => (
          <span
            className={`status-pill ${cell.getValue() ? "good" : "neutral"}`}
          >
            {cell.getValue() ? <Wifi size={13} /> : <WifiOff size={13} />}
            {t(
              locale,
              cell.getValue() ? "status.peer.online" : "status.peer.offline",
            )}
          </span>
        ),
      },
      {
        header: "RTT",
        accessorKey: "rtt_us",
        cell: (cell) =>
          cell.getValue() == null
            ? "—"
            : `${(Number(cell.getValue()) / 1000).toFixed(1)} ms`,
      },
      {
        header: "P2P",
        accessorKey: "p2p_active",
        cell: (cell) => (cell.getValue() ? "P2P" : "Relay"),
      },
    ],
    [locale],
  );
  // TanStack Table intentionally exposes non-memoizable callbacks. The table is
  // scoped to this component instance and does not cross a memo boundary.
  // eslint-disable-next-line react-hooks/incompatible-library
  const table = useReactTable({
    data: snapshot.peers,
    columns,
    getCoreRowModel: getCoreRowModel(),
  });
  if (snapshot.peers.length === 0)
    return empty(label(locale, "common.no_data"));
  return (
    <div className="table-shell">
      <table>
        <thead>
          {table.getHeaderGroups().map((group) => (
            <tr key={group.id}>
              {group.headers.map((header) => (
                <th key={header.id}>
                  {flexRender(
                    header.column.columnDef.header,
                    header.getContext(),
                  )}
                </th>
              ))}
            </tr>
          ))}
        </thead>
        <tbody>
          {table.getRowModel().rows.map((row) => (
            <tr key={row.id}>
              {row.getVisibleCells().map((cell) => (
                <td key={cell.id}>
                  {flexRender(cell.column.columnDef.cell, cell.getContext())}
                </td>
              ))}
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

function Topology({
  snapshot,
  locale,
}: Omit<
  PageContentProps,
  "page" | "selectedNetwork" | "onSelectNetwork" | "onNavigate"
>) {
  const centerX = 360;
  const centerY = 170;
  return (
    <div className="topology-layout">
      <div className="topology-canvas" aria-hidden="true">
        <svg viewBox="0 0 720 340" role="presentation">
          {snapshot.networks.map((network, networkIndex) => {
            const x = 130 + networkIndex * 230;
            return (
              <g key={network.name}>
                <path
                  d={`M ${centerX} ${centerY} Q ${(centerX + x) / 2} ${centerY - 80} ${x} ${centerY}`}
                  className="topology-link"
                />
                <circle
                  cx={x}
                  cy={centerY}
                  r="34"
                  className={`topology-network ${statusTone(network.status)}`}
                />
                <text x={x} y={centerY + 5} textAnchor="middle">
                  {network.name.slice(0, 10)}
                </text>
                {snapshot.peers
                  .filter((peer) => peer.network_name === network.name)
                  .map((peer, index) => {
                    const angle =
                      (index / Math.max(1, network.peer_count)) * Math.PI * 2;
                    const peerX = x + Math.cos(angle) * 64;
                    const peerY = centerY + Math.sin(angle) * 64;
                    return (
                      <circle
                        key={peer.peer_id}
                        cx={peerX}
                        cy={peerY}
                        r="8"
                        className={
                          peer.online ? "topology-peer online" : "topology-peer"
                        }
                      />
                    );
                  })}
              </g>
            );
          })}
          <circle
            cx={centerX}
            cy={centerY}
            r="44"
            className="topology-center"
          />
          <text x={centerX} y={centerY + 5} textAnchor="middle">
            THIS NODE
          </text>
        </svg>
      </div>
      <div className="topology-list">
        {snapshot.networks.map((network) => (
          <div className="topology-row" key={network.name}>
            <Network size={16} />
            <div>
              <strong>{network.name}</strong>
              <span>
                {network.peer_count} peers ·{" "}
                {network.roles.join(" / ") || "member"}
              </span>
            </div>
            <span className={`status-pill ${statusTone(network.status)}`}>
              {t(locale, `status.network.${network.status}`)}
            </span>
          </div>
        ))}
      </div>
    </div>
  );
}

function Traffic({
  snapshot,
  locale,
}: Omit<
  PageContentProps,
  "page" | "selectedNetwork" | "onSelectNetwork" | "onNavigate"
>) {
  const total = snapshot.networks.reduce(
    (sum, network) => ({
      rx: sum.rx + network.traffic.tun_rx_bytes,
      tx: sum.tx + network.traffic.tun_tx_bytes,
      errors: sum.errors + network.traffic.error_count,
    }),
    { rx: 0, tx: 0, errors: 0 },
  );
  return (
    <div className="page-stack">
      <div className="metric-grid">
        <div className="metric-card">
          <ArrowDownToLine size={20} />
          <span>{label(locale, "traffic.row.tun_rx")}</span>
          <strong>{formatBytes(locale, total.rx)}</strong>
        </div>
        <div className="metric-card">
          <ArrowUpFromLine size={20} />
          <span>{label(locale, "traffic.row.tun_tx")}</span>
          <strong>{formatBytes(locale, total.tx)}</strong>
        </div>
        <div className="metric-card">
          <AlertTriangle size={20} />
          <span>{label(locale, "traffic.row.errors")}</span>
          <strong>{formatInteger(locale, total.errors)}</strong>
        </div>
      </div>
      <div className="table-shell">
        <table>
          <thead>
            <tr>
              <th>{label(locale, "field.network_name")}</th>
              <th>RX</th>
              <th>TX</th>
              <th>P2P RX</th>
              <th>P2P TX</th>
            </tr>
          </thead>
          <tbody>
            {snapshot.networks.map((network) => (
              <tr key={network.name}>
                <td>{network.name}</td>
                <td>{formatBytes(locale, network.traffic.tun_rx_bytes)}</td>
                <td>{formatBytes(locale, network.traffic.tun_tx_bytes)}</td>
                <td>{formatBytes(locale, network.traffic.p2p_rx_bytes)}</td>
                <td>{formatBytes(locale, network.traffic.p2p_tx_bytes)}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </div>
  );
}

function Routing({
  snapshot,
  locale,
}: Omit<
  PageContentProps,
  "page" | "selectedNetwork" | "onSelectNetwork" | "onNavigate"
>) {
  const configs = snapshot.configuration?.networks ?? [];
  if (configs.length === 0) return empty(label(locale, "common.no_data"));
  return (
    <div className="settings-grid">
      {configs.map((config) => (
        <section className="surface-card route-card" key={config.network_name}>
          <div className="card-heading">
            <h3>{config.network_name}</h3>
            <span className="status-pill neutral">{config.routing_mode}</span>
          </div>
          <dl className="detail-list">
            <div>
              <dt>{label(locale, "field.exit_node")}</dt>
              <dd>{config.exit_node || "—"}</dd>
            </div>
            <div>
              <dt>MTU</dt>
              <dd>{config.mtu}</dd>
            </div>
            <div>
              <dt>ACL</dt>
              <dd>
                {config.acl_enabled
                  ? `${config.acl_rule_count} rules`
                  : "disabled"}
              </dd>
            </div>
            <div>
              <dt>Direct CIDR</dt>
              <dd>{config.direct_cidrs.length}</dd>
            </div>
          </dl>
        </section>
      ))}
    </div>
  );
}

function CapabilityPage({
  snapshot,
  locale,
  kind,
}: {
  snapshot: NodeSnapshot;
  locale: Locale;
  kind: "exit" | "relay";
}) {
  const capability = snapshot.capabilities?.capabilities.find(
    (item) => item.kind === kind,
  );
  return (
    <div className="page-stack">
      <section className="surface-card capability-feature">
        <div
          className={`feature-icon ${statusTone(capability?.state ?? "unknown")}`}
        >
          {kind === "exit" ? <DoorOpen size={26} /> : <RadioTower size={26} />}
        </div>
        <div>
          <span>{label(locale, `capability.${kind}`)}</span>
          <h2>{capability?.state ?? "unknown"}</h2>
        </div>
      </section>
      {kind === "exit" && snapshot.exit_node_capabilities ? (
        <section className="surface-card">
          <div className="card-heading">
            <h3>Platform</h3>
            <span>{snapshot.exit_node_capabilities.platform}</span>
          </div>
          <div className="capability-grid">
            {[
              [
                "IPv4 forwarding",
                snapshot.exit_node_capabilities.ipv4_forwarding,
              ],
              [
                "IPv6 forwarding",
                snapshot.exit_node_capabilities.ipv6_forwarding,
              ],
              ["NAT", snapshot.exit_node_capabilities.nat],
              ["Cleanup", snapshot.exit_node_capabilities.cleanup],
            ].map(([name, enabled]) => (
              <div
                className={`capability ${enabled ? "good" : "bad"}`}
                key={String(name)}
              >
                <span className="capability-dot" />
                <div>
                  <strong>{name}</strong>
                  <span>{enabled ? "available" : "unavailable"}</span>
                </div>
              </div>
            ))}
          </div>
        </section>
      ) : null}
      {kind === "relay" ? (
        <section className="surface-card">
          <div className="card-heading">
            <h3>{label(locale, "field.enabled")}</h3>
            <RadioTower size={17} />
          </div>
          <p className="muted-copy">
            {capability?.state === "enabled"
              ? "Relay capability is active."
              : "Relay capability is not active."}
          </p>
        </section>
      ) : null}
    </div>
  );
}

function Logs({
  snapshot,
  locale,
}: Omit<
  PageContentProps,
  "page" | "selectedNetwork" | "onSelectNetwork" | "onNavigate"
>) {
  const parentRef = useRef<HTMLDivElement>(null);
  // TanStack Virtual uses the same intentional unstable-callback API as Table.
  // eslint-disable-next-line react-hooks/incompatible-library
  const virtualizer = useVirtualizer({
    count: snapshot.logs.length,
    getScrollElement: () => parentRef.current,
    estimateSize: () => 44,
    overscan: 10,
  });
  if (snapshot.logs.length === 0) return empty(label(locale, "common.no_data"));
  return (
    <div className="log-viewport" ref={parentRef}>
      <div
        className="log-virtual"
        style={{ height: `${virtualizer.getTotalSize()}px` }}
      >
        {virtualizer.getVirtualItems().map((row) => {
          const entry = snapshot.logs[row.index] as NodeLogEntry;
          return (
            <div
              className="log-row"
              key={row.key}
              style={{ transform: `translateY(${row.start}px)` }}
            >
              <span className={`log-level ${entry.level}`}>{entry.level}</span>
              <time>{formatTimestamp(locale, entry.unix_ms)}</time>
              <code>{entry.target}</code>
              <p>{entry.message}</p>
            </div>
          );
        })}
      </div>
    </div>
  );
}

function Identity({
  snapshot,
  locale,
}: Omit<
  PageContentProps,
  "page" | "selectedNetwork" | "onSelectNetwork" | "onNavigate"
>) {
  const identity = snapshot.identity;
  if (!identity) return empty(label(locale, "common.no_data"));
  return (
    <section className="surface-card identity-card">
      <div className="identity-avatar">
        <KeyRound size={28} />
      </div>
      <div className="identity-copy">
        <span>{label(locale, "nav.identity")}</span>
        <h2>{identity.display_name || label(locale, "common.unknown")}</h2>
        <dl className="detail-list">
          <div>
            <dt>{label(locale, "identity.row.node_id")}</dt>
            <dd>
              <code>{identity.node_id}</code>
            </dd>
          </div>
          <div>
            <dt>{label(locale, "identity.row.fingerprint")}</dt>
            <dd>
              <code>{identity.public_key_fingerprint}</code>
            </dd>
          </div>
        </dl>
      </div>
    </section>
  );
}

function SettingsSection({
  group,
  title,
  icon: Icon,
  children,
}: {
  group: string;
  title: string;
  icon: LucideIcon;
  children: ReactNode;
}) {
  return (
    <section className="surface-card settings-section" data-settings-group={group}>
      <div className="card-heading">
        <h3>{title}</h3>
        <Icon size={17} />
      </div>
      {children}
    </section>
  );
}

function Settings({
  snapshot,
  locale,
}: Omit<
  PageContentProps,
  "page" | "selectedNetwork" | "onSelectNetwork" | "onNavigate"
>) {
  const permissions = snapshot.permissions?.permissions ?? [];
  const permissionText = permissions.length
    ? permissions
        .map((permission) => label(locale, `choice.permission.${permission}`))
        .join(label(locale, "common.list_separator"))
    : label(locale, "common.none");
  const capabilities = snapshot.capabilities?.capabilities ?? [];
  const embeddedServices = snapshot.embedded_services ?? [];
  const unsupportedCapabilities = capabilities
    .filter((capability) => capability.state === "unsupported")
    .map((capability) => label(locale, `capability.${capability.kind}`));
  const unsupportedReasons = (
    snapshot.exit_node_capabilities?.unsupported_reasons ?? []
  ).map((reason) => label(locale, `exit_node.unsupported_reason.${reason}`));
  const platformLimitations =
    unsupportedCapabilities.length === 0 && unsupportedReasons.length === 0
      ? label(locale, "settings.platform_limitations.none")
      : [
          tr(locale, "settings.platform_limitations.detail", {
            capabilities:
              unsupportedCapabilities.join(label(locale, "common.list_separator")) ||
              label(locale, "capability.exit"),
          }),
          unsupportedReasons.length
            ? tr(locale, "settings.platform_limitations.reasons", {
                reasons: unsupportedReasons.join(
                  label(locale, "common.list_separator"),
                ),
              })
            : "",
        ]
          .filter(Boolean)
          .join(label(locale, "common.list_separator"));
  const discoveryRoles = snapshot.networks
    .filter((network) => network.roles.includes("discovery"))
    .map(
      (network) =>
        `${network.name} · ${label(locale, `status.network.${network.status}`)}`,
    )
    .join(label(locale, "common.list_separator"));

  return (
    <div className="settings-grid">
      <SettingsSection
        group="owner-security"
        title={label(locale, "settings.section.owner_security")}
        icon={ShieldCheck}
      >
        <dl className="detail-list">
          <div>
            <dt>{label(locale, "settings.row.owner_identity")}</dt>
            <dd>
              {snapshot.identity?.display_name?.trim() ||
                label(locale, "common.unknown")}
            </dd>
          </div>
          <div>
            <dt>{label(locale, "settings.row.node_id")}</dt>
            <dd>
              <code>
                {snapshot.identity?.node_id ??
                  label(locale, "common.not_available")}
              </code>
            </dd>
          </div>
          <div>
            <dt>{label(locale, "settings.row.public_key_fingerprint")}</dt>
            <dd>
              <code>
                {snapshot.identity?.public_key_fingerprint ??
                  label(locale, "common.not_available")}
              </code>
            </dd>
          </div>
          <div>
            <dt>{label(locale, "settings.row.access_model")}</dt>
            <dd>
              {label(
                locale,
                `choice.access_model.${snapshot.permissions?.access_model ?? "trusted_control_plane"}`,
              )}
            </dd>
          </div>
          <div>
            <dt>{label(locale, "settings.row.granted_permissions")}</dt>
            <dd>{permissionText}</dd>
          </div>
          <div>
            <dt>{label(locale, "settings.row.web_authentication")}</dt>
            <dd>{label(locale, "settings.web_session.detail")}</dd>
          </div>
          <div>
            <dt>{label(locale, "settings.row.owner_recovery")}</dt>
            <dd>{label(locale, "settings.owner_recovery.detail")}</dd>
          </div>
        </dl>
      </SettingsSection>

      <SettingsSection
        group="remote-access"
        title={label(locale, "settings.section.remote_access")}
        icon={Globe2}
      >
        <dl className="detail-list">
          <div>
            <dt>{label(locale, "settings.row.effective_ui")}</dt>
            <dd>
              {snapshot.ui_mode
                ? label(locale, `choice.ui_mode.${snapshot.ui_mode.effective}`)
                : label(locale, "common.unknown")}
            </dd>
          </div>
          <div>
            <dt>{label(locale, "settings.row.display_environment")}</dt>
            <dd>
              {snapshot.ui_mode
                ? label(
                    locale,
                    `choice.display_environment.${snapshot.ui_mode.display}`,
                  )
                : label(locale, "common.unknown")}
            </dd>
          </div>
          <div>
            <dt>{label(locale, "settings.row.webui_configuration")}</dt>
            <dd>{label(locale, "settings.webui_configuration.detail")}</dd>
          </div>
          <div>
            <dt>{label(locale, "settings.row.session_security")}</dt>
            <dd>{label(locale, "settings.web_session.detail")}</dd>
          </div>
        </dl>
      </SettingsSection>

      <SettingsSection
        group="services-updates"
        title={label(locale, "settings.section.services_updates")}
        icon={Server}
      >
        <dl className="detail-list">
          {capabilities.length ? (
            capabilities.map((capability) => (
              <div key={capability.kind}>
                <dt>{label(locale, `capability.${capability.kind}`)}</dt>
                <dd>{label(locale, `status.capability.${capability.state}`)}</dd>
              </div>
            ))
          ) : (
            <div>
              <dt>{label(locale, "settings.row.capabilities")}</dt>
              <dd>{label(locale, "common.unknown")}</dd>
            </div>
          )}
          {embeddedServices.length ? (
            embeddedServices.map((service) => (
              <div key={service.network_name}>
                <dt>{service.network_name}</dt>
                <dd>
                  {tr(locale, "gpui.embedded_service_detail", {
                    status: label(locale, `status.service.${service.status}`),
                    signal:
                      service.signaling_addr ??
                      label(locale, "common.not_available"),
                    relay:
                      service.relay_addr ?? label(locale, "common.not_available"),
                  })}
                </dd>
              </div>
            ))
          ) : (
            <div>
              <dt>{label(locale, "settings.row.embedded_services")}</dt>
              <dd>{label(locale, "common.none")}</dd>
            </div>
          )}
          <div>
            <dt>{label(locale, "settings.row.update_configuration")}</dt>
            <dd>{label(locale, "settings.updates.detail")}</dd>
          </div>
          <div>
            <dt>{label(locale, "settings.row.platform_limitations")}</dt>
            <dd>{platformLimitations}</dd>
          </div>
        </dl>
      </SettingsSection>

      <SettingsSection
        group="discovery-index"
        title={label(locale, "settings.section.discovery_index")}
        icon={Wifi}
      >
        <dl className="detail-list">
          <div>
            <dt>{label(locale, "settings.row.discovery_roles")}</dt>
            <dd>
              {discoveryRoles || label(locale, "settings.discovery_roles.none")}
            </dd>
          </div>
          <div>
            <dt>{label(locale, "settings.row.discovery_index")}</dt>
            <dd>{label(locale, "settings.discovery_index.detail")}</dd>
          </div>
          <div>
            <dt>{label(locale, "settings.row.custom_discovery_index")}</dt>
            <dd>{label(locale, "settings.custom_index.detail")}</dd>
          </div>
        </dl>
      </SettingsSection>

      <SettingsSection
        group="data-recovery"
        title={label(locale, "settings.section.data_recovery")}
        icon={Database}
      >
        <dl className="detail-list">
          <div>
            <dt>{label(locale, "settings.row.snapshot_revision")}</dt>
            <dd>{snapshot.revision}</dd>
          </div>
          <div>
            <dt>{label(locale, "settings.row.configured_networks")}</dt>
            <dd>{snapshot.configuration?.networks.length ?? 0}</dd>
          </div>
          <div>
            <dt>{label(locale, "settings.row.recent_activity")}</dt>
            <dd>{snapshot.recent_activity?.length ?? 0}</dd>
          </div>
          <div>
            <dt>{label(locale, "settings.row.backup_restore")}</dt>
            <dd>{label(locale, "settings.backup.detail")}</dd>
          </div>
          <div>
            <dt>{label(locale, "settings.row.write_mode")}</dt>
            <dd>
              {label(locale, "settings.value.read_only")} ·{" "}
              {label(locale, "settings.read_only.detail")}
            </dd>
          </div>
        </dl>
      </SettingsSection>
    </div>
  );
}

const PAGE_ICONS: Record<string, LucideIcon> = {
  overview: Gauge,
  networks: Network,
  peers: Waypoints,
  topology: Globe2,
  traffic: Activity,
  routing: Database,
  exit_node: DoorOpen,
  relay: RadioTower,
  logs: Boxes,
  identity: KeyRound,
  settings: Cpu,
};

export function PageContent(props: PageContentProps) {
  const { page, snapshot, locale } = props;
  let content: JSX.Element;
  switch (page.id) {
    case "overview":
      content = <Overview {...props} />;
      break;
    case "networks":
      content = <Networks {...props} />;
      break;
    case "peers":
      content = <Peers {...props} />;
      break;
    case "topology":
      content = <Topology {...props} />;
      break;
    case "traffic":
      content = <Traffic {...props} />;
      break;
    case "routing":
      content = <Routing {...props} />;
      break;
    case "exit_node":
      content = (
        <CapabilityPage snapshot={snapshot} locale={locale} kind="exit" />
      );
      break;
    case "relay":
      content = (
        <CapabilityPage snapshot={snapshot} locale={locale} kind="relay" />
      );
      break;
    case "logs":
      content = <Logs {...props} />;
      break;
    case "identity":
      content = <Identity {...props} />;
      break;
    case "settings":
      content = <Settings {...props} />;
      break;
    default:
      content = empty(label(locale, "common.unknown_page"));
  }
  const Icon = PAGE_ICONS[page.id] ?? CircleDot;
  return (
    <section
      className="page-content"
      data-ui-region="content"
      aria-labelledby="page-title"
    >
      <header className="page-heading">
        <div className="page-heading-icon">
          <Icon size={20} />
        </div>
        <div>
          <h1 id="page-title">{label(locale, page.title_key)}</h1>
          <p>{label(locale, "common.node_contract_description")}</p>
        </div>
      </header>
      {content}
    </section>
  );
}

function formatBytes(locale: Locale, value: number): string {
  try {
    return window.FuckIPV6I18n?.formatBytes(locale, value) ?? `${value} B`;
  } catch {
    return `${value} B`;
  }
}

function formatInteger(locale: Locale, value: number): string {
  try {
    return window.FuckIPV6I18n?.formatInteger(locale, value) ?? String(value);
  } catch {
    return String(value);
  }
}

function formatDuration(locale: Locale, seconds: number): string {
  try {
    return (
      window.FuckIPV6I18n?.formatDuration(locale, seconds) ?? `${seconds}s`
    );
  } catch {
    return `${seconds}s`;
  }
}

function formatTimestamp(locale: Locale, unixMs: number): string {
  try {
    return (
      window.FuckIPV6I18n?.formatTimestamp(locale, unixMs) ??
      new Date(unixMs).toISOString()
    );
  } catch {
    return new Date(unixMs).toISOString();
  }
}
