import * as ContextMenu from "@radix-ui/react-context-menu";
import { useVirtualizer } from "@tanstack/react-virtual";
import { flexRender, getCoreRowModel, useReactTable, type ColumnDef } from "@tanstack/react-table";
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
import { useMemo, useRef, type JSX } from "react";
import type {
  NodeCapabilitySnapshot,
  NodeLogEntry,
  NodeNetworkSnapshot,
  NodePeerSnapshot,
  NodeSnapshot,
  NodeUiPage,
} from "../../../contracts/typescript/node-contract-v1";
import { label, primaryStatus } from "../contract/invariants";
import { t } from "../i18n";

type PageContentProps = {
  page: NodeUiPage;
  snapshot: NodeSnapshot;
  locale: Locale;
  selectedNetwork: string;
  onSelectNetwork(name: string): void;
  onNavigate(pageId: string): void;
};

function empty(text: string): JSX.Element {
  return <div className="empty-state">{text}</div>;
}

function statusTone(value: string): string {
  if (["online", "running", "enabled", "available"].includes(value)) return "good";
  if (["failed", "error", "unavailable", "unsupported"].includes(value)) return "bad";
  if (["degraded", "connecting", "starting", "stopping"].includes(value)) return "warn";
  return "neutral";
}

function CapabilityMark({ capability }: { capability: NodeCapabilitySnapshot }) {
  return (
    <div className={`capability ${statusTone(capability.state)}`}>
      <span className="capability-dot" aria-hidden="true" />
      <div>
        <strong>{label("zh-CN", `capability.${capability.kind}`)}</strong>
        <span>{capability.state}</span>
      </div>
    </div>
  );
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
            <div className="network-icon"><Network size={18} /></div>
            <div className="network-name">
              <strong>{network.name}</strong>
              <span>{network.tun_name}</span>
            </div>
            <span className={`status-pill ${statusTone(network.status)}`}>{t(locale, `status.network.${network.status}`)}</span>
          </div>
          <div className="network-metrics">
            <div><span>{label(locale, "common.node_navigation")}</span><strong>{network.peer_count}</strong></div>
            <div><span>{formatDuration(locale, network.uptime_secs)}</span><strong>{network.roles.join(" · ") || "member"}</strong></div>
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

function Overview({
  snapshot,
  locale,
  onSelectNetwork,
  onNavigate,
}: Omit<PageContentProps, "page">) {
  const status = primaryStatus(snapshot, locale);
  const capabilities = snapshot.capabilities?.capabilities ?? [];
  const issues = [
    ...snapshot.lifecycle.failed_networks.map((name) => `${label(locale, "status.network.failed")}: ${name}`),
    ...(snapshot.last_error ? [snapshot.last_error.message] : []),
  ].slice(0, 3);
  return (
    <div className="page-stack">
      <section className={`hero-status ${statusTone(status.id)}`}>
        <div className="hero-status-icon">
          {status.id === "running" ? <CheckCircle2 size={30} /> : status.id === "failed" ? <AlertTriangle size={30} /> : <Activity size={30} />}
        </div>
        <div className="hero-status-copy">
          <span>{label(locale, "common.primary_status")}</span>
          <h2>{status.label}</h2>
          <p>{status.detail}</p>
        </div>
        <div className="hero-status-meta">
          <strong>{snapshot.identity?.display_name || snapshot.identity?.node_id || "—"}</strong>
          <span>{label(locale, "common.revision")} {snapshot.revision}</span>
        </div>
      </section>

      <div className="overview-grid">
        <section className="surface-card">
          <div className="card-heading"><h3>{label(locale, "nav.networks")}</h3><span>{snapshot.networks.length}</span></div>
          {snapshot.networks.length === 0
            ? empty(label(locale, "common.no_running_networks"))
            : snapshot.networks.slice(0, 3).map((network) => (
              <button className="compact-row" type="button" key={network.name} onClick={() => onSelectNetwork(network.name)}>
                <CircleDot size={15} className={statusTone(network.status)} />
                <span><strong>{network.name}</strong><small>{network.peer_count} peers · {network.tun_name}</small></span>
                <span className={`status-pill ${statusTone(network.status)}`}>{t(locale, `status.network.${network.status}`)}</span>
              </button>
            ))}
        </section>

        <section className="surface-card">
          <div className="card-heading"><h3>{label(locale, "common.node_capabilities")}</h3><Boxes size={17} /></div>
          <div className="capability-grid">
            {capabilities.length === 0 ? empty(label(locale, "common.no_data")) : capabilities.map((capability) => <CapabilityMark key={capability.kind} capability={capability} />)}
          </div>
        </section>

        <section className="surface-card issues-card">
          <div className="card-heading"><h3>{label(locale, "common.runtime_status")}</h3><AlertTriangle size={17} /></div>
          {issues.length === 0 ? <div className="all-clear"><ShieldCheck size={18} /> {status.label}</div> : issues.map((issue) => <p className="issue-row" key={issue}>{issue}</p>)}
          <button className="text-button" type="button" onClick={() => onNavigate("logs")}>{label(locale, "nav.logs")}</button>
        </section>
      </div>
    </div>
  );
}

function Networks({ snapshot, locale, onSelectNetwork, onNavigate }: Omit<PageContentProps, "page">) {
  if (snapshot.networks.length === 0) return empty(label(locale, "common.no_running_networks"));
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

function Peers({ snapshot, locale }: Omit<PageContentProps, "page" | "selectedNetwork" | "onSelectNetwork" | "onNavigate">) {
  const columns = useMemo<ColumnDef<NodePeerSnapshot>[]>(() => [
    {
      header: label(locale, "field.peer_target"),
      accessorKey: "peer_id",
      cell: (cell) => <code>{String(cell.getValue())}</code>,
    },
    { header: label(locale, "field.network_name"), accessorKey: "network_name" },
    { header: "IPv6", accessorKey: "virtual_ipv6", cell: (cell) => <code>{String(cell.getValue())}</code> },
    {
      header: label(locale, "common.runtime_status"),
      accessorKey: "online",
      cell: (cell) => (
        <span className={`status-pill ${cell.getValue() ? "good" : "neutral"}`}>
          {cell.getValue() ? <Wifi size={13} /> : <WifiOff size={13} />}
          {t(locale, cell.getValue() ? "status.peer.online" : "status.peer.offline")}
        </span>
      ),
    },
    {
      header: "RTT",
      accessorKey: "rtt_us",
      cell: (cell) => cell.getValue() == null ? "—" : `${(Number(cell.getValue()) / 1000).toFixed(1)} ms`,
    },
    { header: "P2P", accessorKey: "p2p_active", cell: (cell) => cell.getValue() ? "P2P" : "Relay" },
  ], [locale]);
  // TanStack Table intentionally exposes non-memoizable callbacks. The table is
  // scoped to this component instance and does not cross a memo boundary.
  // eslint-disable-next-line react-hooks/incompatible-library
  const table = useReactTable({ data: snapshot.peers, columns, getCoreRowModel: getCoreRowModel() });
  if (snapshot.peers.length === 0) return empty(label(locale, "common.no_data"));
  return (
    <div className="table-shell">
      <table>
        <thead>{table.getHeaderGroups().map((group) => <tr key={group.id}>{group.headers.map((header) => <th key={header.id}>{flexRender(header.column.columnDef.header, header.getContext())}</th>)}</tr>)}</thead>
        <tbody>
          {table.getRowModel().rows.map((row) => (
            <tr key={row.id}>{row.getVisibleCells().map((cell) => <td key={cell.id}>{flexRender(cell.column.columnDef.cell, cell.getContext())}</td>)}</tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

function Topology({ snapshot, locale }: Omit<PageContentProps, "page" | "selectedNetwork" | "onSelectNetwork" | "onNavigate">) {
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
                <path d={`M ${centerX} ${centerY} Q ${(centerX + x) / 2} ${centerY - 80} ${x} ${centerY}`} className="topology-link" />
                <circle cx={x} cy={centerY} r="34" className={`topology-network ${statusTone(network.status)}`} />
                <text x={x} y={centerY + 5} textAnchor="middle">{network.name.slice(0, 10)}</text>
                {snapshot.peers.filter((peer) => peer.network_name === network.name).map((peer, index) => {
                  const angle = (index / Math.max(1, network.peer_count)) * Math.PI * 2;
                  const peerX = x + Math.cos(angle) * 64;
                  const peerY = centerY + Math.sin(angle) * 64;
                  return <circle key={peer.peer_id} cx={peerX} cy={peerY} r="8" className={peer.online ? "topology-peer online" : "topology-peer"} />;
                })}
              </g>
            );
          })}
          <circle cx={centerX} cy={centerY} r="44" className="topology-center" />
          <text x={centerX} y={centerY + 5} textAnchor="middle">THIS NODE</text>
        </svg>
      </div>
      <div className="topology-list">
        {snapshot.networks.map((network) => (
          <div className="topology-row" key={network.name}>
            <Network size={16} />
            <div><strong>{network.name}</strong><span>{network.peer_count} peers · {network.roles.join(" / ") || "member"}</span></div>
            <span className={`status-pill ${statusTone(network.status)}`}>{t(locale, `status.network.${network.status}`)}</span>
          </div>
        ))}
      </div>
    </div>
  );
}

function Traffic({ snapshot, locale }: Omit<PageContentProps, "page" | "selectedNetwork" | "onSelectNetwork" | "onNavigate">) {
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
        <div className="metric-card"><ArrowDownToLine size={20} /><span>{label(locale, "traffic.row.tun_rx")}</span><strong>{formatBytes(locale, total.rx)}</strong></div>
        <div className="metric-card"><ArrowUpFromLine size={20} /><span>{label(locale, "traffic.row.tun_tx")}</span><strong>{formatBytes(locale, total.tx)}</strong></div>
        <div className="metric-card"><AlertTriangle size={20} /><span>{label(locale, "traffic.row.errors")}</span><strong>{formatInteger(locale, total.errors)}</strong></div>
      </div>
      <div className="table-shell">
        <table>
          <thead><tr><th>{label(locale, "field.network_name")}</th><th>RX</th><th>TX</th><th>P2P RX</th><th>P2P TX</th></tr></thead>
          <tbody>{snapshot.networks.map((network) => <tr key={network.name}><td>{network.name}</td><td>{formatBytes(locale, network.traffic.tun_rx_bytes)}</td><td>{formatBytes(locale, network.traffic.tun_tx_bytes)}</td><td>{formatBytes(locale, network.traffic.p2p_rx_bytes)}</td><td>{formatBytes(locale, network.traffic.p2p_tx_bytes)}</td></tr>)}</tbody>
        </table>
      </div>
    </div>
  );
}

function Routing({ snapshot, locale }: Omit<PageContentProps, "page" | "selectedNetwork" | "onSelectNetwork" | "onNavigate">) {
  const configs = snapshot.configuration?.networks ?? [];
  if (configs.length === 0) return empty(label(locale, "common.no_data"));
  return (
    <div className="settings-grid">
      {configs.map((config) => (
        <section className="surface-card route-card" key={config.network_name}>
          <div className="card-heading"><h3>{config.network_name}</h3><span className="status-pill neutral">{config.routing_mode}</span></div>
          <dl className="detail-list">
            <div><dt>{label(locale, "field.exit_node")}</dt><dd>{config.exit_node || "—"}</dd></div>
            <div><dt>MTU</dt><dd>{config.mtu}</dd></div>
            <div><dt>ACL</dt><dd>{config.acl_enabled ? `${config.acl_rule_count} rules` : "disabled"}</dd></div>
            <div><dt>Direct CIDR</dt><dd>{config.direct_cidrs.length}</dd></div>
          </dl>
        </section>
      ))}
    </div>
  );
}

function CapabilityPage({ snapshot, locale, kind }: { snapshot: NodeSnapshot; locale: Locale; kind: "exit" | "relay" }) {
  const capability = snapshot.capabilities?.capabilities.find((item) => item.kind === kind);
  return (
    <div className="page-stack">
      <section className="surface-card capability-feature">
        <div className={`feature-icon ${statusTone(capability?.state ?? "unknown")}`}>{kind === "exit" ? <DoorOpen size={26} /> : <RadioTower size={26} />}</div>
        <div><span>{label(locale, `capability.${kind}`)}</span><h2>{capability?.state ?? "unknown"}</h2></div>
      </section>
      {kind === "exit" && snapshot.exit_node_capabilities ? (
        <section className="surface-card">
          <div className="card-heading"><h3>Platform</h3><span>{snapshot.exit_node_capabilities.platform}</span></div>
          <div className="capability-grid">
            {[
              ["IPv4 forwarding", snapshot.exit_node_capabilities.ipv4_forwarding],
              ["IPv6 forwarding", snapshot.exit_node_capabilities.ipv6_forwarding],
              ["NAT", snapshot.exit_node_capabilities.nat],
              ["Cleanup", snapshot.exit_node_capabilities.cleanup],
            ].map(([name, enabled]) => <div className={`capability ${enabled ? "good" : "bad"}`} key={String(name)}><span className="capability-dot" /><div><strong>{name}</strong><span>{enabled ? "available" : "unavailable"}</span></div></div>)}
          </div>
        </section>
      ) : null}
      {kind === "relay" ? <section className="surface-card"><div className="card-heading"><h3>{label(locale, "field.enabled")}</h3><RadioTower size={17} /></div><p className="muted-copy">{capability?.state === "enabled" ? "Relay capability is active." : "Relay capability is not active."}</p></section> : null}
    </div>
  );
}

function Logs({ snapshot, locale }: Omit<PageContentProps, "page" | "selectedNetwork" | "onSelectNetwork" | "onNavigate">) {
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
      <div className="log-virtual" style={{ height: `${virtualizer.getTotalSize()}px` }}>
        {virtualizer.getVirtualItems().map((row) => {
          const entry = snapshot.logs[row.index] as NodeLogEntry;
          return (
            <div className="log-row" key={row.key} style={{ transform: `translateY(${row.start}px)` }}>
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

function Identity({ snapshot, locale }: Omit<PageContentProps, "page" | "selectedNetwork" | "onSelectNetwork" | "onNavigate">) {
  const identity = snapshot.identity;
  if (!identity) return empty(label(locale, "common.no_data"));
  return (
    <section className="surface-card identity-card">
      <div className="identity-avatar"><KeyRound size={28} /></div>
      <div className="identity-copy">
        <span>{label(locale, "nav.identity")}</span>
        <h2>{identity.display_name || label(locale, "common.unknown")}</h2>
        <dl className="detail-list">
          <div><dt>{label(locale, "identity.row.node_id")}</dt><dd><code>{identity.node_id}</code></dd></div>
          <div><dt>{label(locale, "identity.row.fingerprint")}</dt><dd><code>{identity.public_key_fingerprint}</code></dd></div>
        </dl>
      </div>
    </section>
  );
}

function Settings({ snapshot, locale }: Omit<PageContentProps, "page" | "selectedNetwork" | "onSelectNetwork" | "onNavigate">) {
  return (
    <div className="settings-grid">
      <section className="surface-card">
        <div className="card-heading"><h3>{label(locale, "settings.row.permissions")}</h3><ShieldCheck size={17} /></div>
        <div className="chip-list">{snapshot.permissions?.permissions.map((permission) => <span className="data-chip" key={permission}>{permission}</span>) ?? empty(label(locale, "common.no_data"))}</div>
      </section>
      <section className="surface-card">
        <div className="card-heading"><h3>{label(locale, "settings.row.ui_mode")}</h3><Cpu size={17} /></div>
        <dl className="detail-list">
          <div><dt>{label(locale, "settings.row.access_model")}</dt><dd>{snapshot.permissions?.access_model ?? "trusted_control_plane"}</dd></div>
          <div><dt>Mode</dt><dd>{snapshot.ui_mode?.effective ?? "web_ui"}</dd></div>
          <div><dt>Environment</dt><dd>{snapshot.ui_mode?.display ?? "headless"}</dd></div>
        </dl>
      </section>
      <section className="surface-card">
        <div className="card-heading"><h3>{label(locale, "common.runtime_status")}</h3><Server size={17} /></div>
        <dl className="detail-list">
          <div><dt>{label(locale, "common.primary_status")}</dt><dd>{primaryStatus(snapshot, locale).label}</dd></div>
          <div><dt>{label(locale, "common.revision")}</dt><dd>{snapshot.revision}</dd></div>
          <div><dt>{label(locale, "common.node_capabilities")}</dt><dd>{snapshot.capabilities?.capabilities.length ?? 0}</dd></div>
        </dl>
      </section>
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
    case "overview": content = <Overview {...props} />; break;
    case "networks": content = <Networks {...props} />; break;
    case "peers": content = <Peers {...props} />; break;
    case "topology": content = <Topology {...props} />; break;
    case "traffic": content = <Traffic {...props} />; break;
    case "routing": content = <Routing {...props} />; break;
    case "exit_node": content = <CapabilityPage snapshot={snapshot} locale={locale} kind="exit" />; break;
    case "relay": content = <CapabilityPage snapshot={snapshot} locale={locale} kind="relay" />; break;
    case "logs": content = <Logs {...props} />; break;
    case "identity": content = <Identity {...props} />; break;
    case "settings": content = <Settings {...props} />; break;
    default: content = empty(label(locale, "common.unknown_page"));
  }
  const Icon = PAGE_ICONS[page.id] ?? CircleDot;
  return (
    <section className="page-content" data-ui-region="content" aria-labelledby="page-title">
      <header className="page-heading">
        <div className="page-heading-icon"><Icon size={20} /></div>
        <div><h1 id="page-title">{label(locale, page.title_key)}</h1><p>{label(locale, "common.node_contract_description")}</p></div>
      </header>
      {content}
    </section>
  );
}

function formatBytes(locale: Locale, value: number): string {
  try { return window.FuckIPV6I18n?.formatBytes(locale, value) ?? `${value} B`; } catch { return `${value} B`; }
}

function formatInteger(locale: Locale, value: number): string {
  try { return window.FuckIPV6I18n?.formatInteger(locale, value) ?? String(value); } catch { return String(value); }
}

function formatDuration(locale: Locale, seconds: number): string {
  try { return window.FuckIPV6I18n?.formatDuration(locale, seconds) ?? `${seconds}s`; } catch { return `${seconds}s`; }
}

function formatTimestamp(locale: Locale, unixMs: number): string {
  try { return window.FuckIPV6I18n?.formatTimestamp(locale, unixMs) ?? new Date(unixMs).toISOString(); } catch { return new Date(unixMs).toISOString(); }
}
