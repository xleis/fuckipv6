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
  Check,
  CheckCircle2,
  CircleDot,
  Copy,
  Cpu,
  Database,
  DoorOpen,
  Gauge,
  Globe2,
  KeyRound,
  Network,
  RadioTower,
  RefreshCw,
  Search,
  Server,
  ShieldCheck,
  SlidersHorizontal,
  Waypoints,
  Wifi,
  WifiOff,
  type LucideIcon,
} from "lucide-react";
import {
  useEffect,
  useMemo,
  useRef,
  useState,
  type JSX,
  type ReactNode,
} from "react";
import type {
  ActivityEntry,
  NodeDiagnosticLayer,
  NodeDiagnosticState,
  NodeDiagnosticsSnapshot,
  NodeLogEntry,
  NodeNetworkSnapshot,
  NodePeerSnapshot,
  NodeProblem,
  NodeRoutingDefaultAction,
  NodeRoutingSummarySnapshot,
  NodeRoutingTargetSnapshot,
  NodeSnapshot,
  NodeUiPage,
} from "../../../contracts/typescript/node-contract-v1";
import {
  executionBlockReason,
  i18n,
  label,
  primaryStatus,
  type ActionValues,
} from "../contract/invariants";
import {
  EMPTY_DISCOVERY_FILTERS,
  discoveryFacets,
  discoveryHasStaleResults,
  projectDiscovery,
  type DiscoveryFilters,
  type DiscoveryProjection,
  type DiscoveryResult,
} from "../discovery";
import { t, tr } from "../i18n";
import { advancedSettingsGroups } from "../settings";

type PageContentProps = {
  page: NodeUiPage;
  snapshot: NodeSnapshot;
  locale: Locale;
  selectedNetwork: string;
  recentDiagnostics: NodeDiagnosticsSnapshot | null;
  footer?: ReactNode;
  onSelectNetwork(name: string): void;
  onNavigate(pageId: string): void;
  onRequestAction(
    actionId: string,
    initialValues?: ActionValues,
  ): void;
};

function empty(text: string): JSX.Element {
  return <div className="empty-state">{text}</div>;
}

function statusTone(value: string): string {
  if (["online", "running", "enabled", "available", "up"].includes(value))
    return "good";
  if (["failed", "error", "unavailable", "unsupported", "down"].includes(value))
    return "bad";
  if (["degraded", "connecting", "starting", "stopping"].includes(value))
    return "warn";
  return "neutral";
}

function routingTone(status: NodeRoutingSummarySnapshot["status"]): string {
  if (status === "current") return "good";
  if (status === "stale") return "warn";
  return "bad";
}

function routingActionLabel(
  locale: Locale,
  action: NodeRoutingDefaultAction,
): string {
  return label(locale, `routing.action.${action.kind}`);
}

function routingTargetLabel(
  locale: Locale,
  target: NodeRoutingTargetSnapshot | null | undefined,
): string {
  if (!target) return label(locale, "common.none");
  return target.kind === "network"
    ? tr(locale, "routing.target.network", { network: target.network_name })
    : tr(locale, "routing.target.exit", { node: target.node_id });
}

function routingProblemLabel(
  locale: Locale,
  problem: NodeProblem | null | undefined,
): string | null {
  if (!problem) return null;
  const localized = tr(locale, problem.message_key, problem.params ?? {});
  return localized === problem.message_key ? problem.message : localized;
}

function NetworkContextMenu({
  network,
  locale,
  onSelectNetwork,
  onNavigate,
  onRequestAction,
}: {
  network: NodeNetworkSnapshot;
  locale: Locale;
  onSelectNetwork(name: string): void;
  onNavigate(pageId: string): void;
  onRequestAction(actionId: string, initialValues?: ActionValues): void;
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
          <ContextMenu.Item
            className="context-item danger"
            onSelect={() => {
              onSelectNetwork(network.name);
              onRequestAction("leave_network", {
                network_name: network.name,
              });
            }}
          >
            {label(locale, "action.leave_network")}
          </ContextMenu.Item>
        </ContextMenu.Content>
      </ContextMenu.Portal>
    </ContextMenu.Root>
  );
}

function MembershipPolicyNotice({
  membership,
  snapshot,
  locale,
  onRequestAction,
}: {
  membership: NonNullable<NodeSnapshot["memberships"]>[number];
  snapshot: NodeSnapshot;
  locale: Locale;
  onRequestAction(
    actionId: string,
    initialValues?: ActionValues,
  ): void;
}) {
  const [exitConfirmed, setExitConfirmed] = useState(false);
  const currentTerms = membership.current_terms;
  const currentHash = membership.current_terms_hash;
  const currentExitObligation =
    currentTerms?.exit_contribution ?? membership.exit_contribution;
  const currentExitScope =
    currentTerms?.exit_contribution_scope ??
    membership.exit_contribution_scope ??
    "not_applicable";
  const exitRequired = currentExitObligation === "required";
  const policyChanged =
    Boolean(currentHash) && currentHash !== membership.terms_hash;
  const confirmationRequired = membership.policy_confirmation_required;
  const localSupported = snapshot.exit_node_capabilities?.supported === true;
  const localEnabled = snapshot.networks.some(
    (network) =>
      network.name === membership.network_name &&
      network.roles.includes("exit_node"),
  );

  return (
    <article
      className="membership-policy-card"
      data-testid={`membership-policy-${membership.network_id}`}
    >
      <div className="membership-policy-heading">
        <div>
          <strong>{membership.network_name}</strong>
          <span>{label(locale, "membership.policy.title")}</span>
        </div>
        <span
          className={`status-pill ${confirmationRequired ? "warn" : "good"}`}
        >
          {tr(
            locale,
            confirmationRequired
              ? "membership.policy.confirmation_required"
              : policyChanged
                ? "membership.policy.notice_only"
                : "membership.policy.acknowledged",
            {
              network: membership.network_name,
            },
          )}
        </span>
      </div>
      <dl className="profile-definition-list membership-policy-details">
        <div>
          <dt>{label(locale, "join.network_policy_revision")}</dt>
          <dd>
            {(currentTerms?.network_policy_revision ??
              membership.network_policy_revision) == null
              ? label(locale, "common.none")
              : i18n().formatInteger(
                  locale,
                  currentTerms?.network_policy_revision ??
                    membership.network_policy_revision ??
                    0,
                )}
          </dd>
        </div>
        <div>
          <dt>{label(locale, "join.privacy_policy_revision")}</dt>
          <dd>
            {(currentTerms?.privacy_policy_revision ??
              membership.privacy_policy_revision) == null
              ? label(locale, "common.none")
              : i18n().formatInteger(
                  locale,
                  currentTerms?.privacy_policy_revision ??
                    membership.privacy_policy_revision ??
                    0,
                )}
          </dd>
        </div>
        <div>
          <dt>{label(locale, "join.exit.scope")}</dt>
          <dd>
            {label(
              locale,
              currentExitObligation === "required"
                ? "join.exit.required"
                : "join.exit.not_required",
            )}
          </dd>
        </div>
        <div>
          <dt>{label(locale, "join.exit_contribution")}</dt>
          <dd>{label(locale, `join.exit.scope.${currentExitScope}`)}</dd>
        </div>
        <div>
          <dt>{label(locale, "membership.policy.current")}</dt>
          <dd>{currentHash ?? membership.terms_hash ?? label(locale, "common.none")}</dd>
        </div>
        <div>
          <dt>{label(locale, "membership.policy.local_exit_state")}</dt>
          <dd>
            {label(
              locale,
              localSupported
                ? "join.exit.local_supported"
                : "join.exit.local_unsupported",
            )}
            {" · "}
            {label(
              locale,
              localEnabled
                ? "join.exit.local_enabled"
                : "join.exit.local_not_enabled",
            )}
          </dd>
        </div>
      </dl>
      {confirmationRequired && currentTerms ? (
        <div className="membership-policy-confirmation">
          <div className="inline-notice warning">
            <ShieldCheck size={15} />
            <span>
              {label(locale, "join.exit.disclosure.purpose")}{" "}
              {label(locale, "join.exit.disclosure.bandwidth")}{" "}
              {label(locale, "join.exit.disclosure.logging")}{" "}
              {label(locale, "join.exit.disclosure.disable")}
            </span>
          </div>
          {exitRequired ? (
            <label className="check-field join-terms-check">
              <input
                type="checkbox"
                checked={exitConfirmed}
                onChange={(event) => setExitConfirmed(event.target.checked)}
              />
              <span>{label(locale, "membership.policy.confirm_exit")}</span>
            </label>
          ) : null}
          <p>{label(locale, "membership.policy.local_exit_separate")}</p>
          <button
            className="primary-button"
            type="button"
            disabled={!currentHash || (exitRequired && !exitConfirmed)}
            onClick={() =>
              onRequestAction("acknowledge_membership_policy", {
                network_id: membership.network_id,
                expected_terms_hash: currentHash ?? "",
                acknowledged_at_unix_ms: String(Date.now()),
                ...(exitRequired
                  ? { exit_contribution_acknowledged: "true" }
                  : {}),
              })
            }
          >
            {label(locale, "membership.policy.confirm")}
          </button>
        </div>
      ) : null}
    </article>
  );
}

const OVERVIEW_ACTIVITY_ACTIONS = new Set([
  "network.create",
  "network.join",
  "network.leave",
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

type RuntimePageAction = { id: string; labelKey: string };

function runtimeActionPlan(statusId: string): {
  primary: RuntimePageAction;
  secondary?: RuntimePageAction;
} | null {
  if (statusId === "uninitialized" || statusId === "stopped") {
    return { primary: { id: "start_node", labelKey: "action.start_node" } };
  }
  if (statusId === "running") {
    return {
      primary: { id: "stop_node", labelKey: "action.stop_node" },
      secondary: { id: "restart_node", labelKey: "action.restart_node" },
    };
  }
  if (statusId === "degraded" || statusId === "failed") {
    return {
      primary: {
        id: "get_node_diagnostics",
        labelKey: "action.get_node_diagnostics",
      },
      secondary: { id: "restart_node", labelKey: "action.restart_node" },
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
    .sort(
      (left, right) =>
        right.occurred_at_unix_ms - left.occurred_at_unix_ms ||
        right.audit_id.localeCompare(left.audit_id),
    )
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
  snapshot,
  locale,
  onSelectNetwork,
  onNavigate,
  onRequestAction,
}: Pick<
  PageContentProps,
  "snapshot" | "locale" | "onSelectNetwork" | "onNavigate" | "onRequestAction"
>) {
  const status = primaryStatus(snapshot, locale);
  const issues = overviewIssues(snapshot, locale);
  const activity = recentOverviewActivity(snapshot, locale);
  const nodeName =
    snapshot.identity?.display_name ||
    snapshot.identity?.node_id ||
    label(locale, "overview.node_name_fallback");
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
        <button
          className="primary-button overview-primary-action"
          type="button"
          onClick={() => onNavigate("node_runtime")}
        >
          {label(locale, "overview.manage_runtime")}
        </button>
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
            onClick={() => shortcut("refresh_discovery_index")}
          >
            <span>{label(locale, "shortcut.discover_networks")}</span>
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

function diagnosticStateTone(state: NodeDiagnosticState): string {
  if (state === "passed") return "good";
  if (state === "failed") return "bad";
  if (state === "unsupported" || state === "no_permission") return "warn";
  return "neutral";
}

function diagnosticDuration(locale: Locale, durationMs: number): string {
  if (durationMs < 1000) {
    return tr(locale, "diagnostic.duration_ms_value", {
      duration: formatInteger(locale, durationMs),
    });
  }
  const seconds = Math.round((durationMs / 1000) * 10) / 10;
  return tr(locale, "diagnostic.duration_value", {
    duration: seconds.toFixed(Number.isInteger(seconds) ? 0 : 1),
  });
}

const DIAGNOSTIC_LAYERS: NodeDiagnosticLayer[] = [
  "network",
  "dns",
  "tun",
  "coordinator",
  "relay",
  "exit",
];

function DiagnosticsPanel({
  diagnostics,
  locale,
  onRequestAction,
}: {
  diagnostics: NodeDiagnosticsSnapshot | null;
  locale: Locale;
  onRequestAction(actionId: string): void;
}) {
  const [copyFeedback, setCopyFeedback] = useState<
    "id" | "report" | "failed" | null
  >(null);

  async function copy(value: string, kind: "id" | "report"): Promise<void> {
    try {
      await navigator.clipboard.writeText(value);
      setCopyFeedback(kind);
    } catch {
      setCopyFeedback("failed");
    }
  }

  if (!diagnostics) {
    return (
      <section className="surface-card" data-testid="runtime-diagnostics">
        <div className="card-heading">
          <h3>{label(locale, "diagnostic.title")}</h3>
          <button
            className="secondary-button"
            type="button"
            onClick={() => onRequestAction("get_node_diagnostics")}
          >
            {label(locale, "diagnostic.run")}
          </button>
        </div>
        <div className="empty-state">
          <strong>{label(locale, "diagnostic.empty_title")}</strong>
          <span>{label(locale, "diagnostic.empty_detail")}</span>
        </div>
      </section>
    );
  }

  const duration = Math.max(
    0,
    diagnostics.completed_at_unix_ms - diagnostics.started_at_unix_ms,
  );
  const residue = diagnostics.residue;
  const residueSummary =
    residue.sessions_remaining === 0 &&
    residue.listeners_remaining === 0 &&
    residue.peers_remaining === 0 &&
    residue.embedded_services_remaining === 0 &&
    residue.residual_networks.length === 0 &&
    residue.issues.length === 0
      ? label(locale, "diagnostic.residue_clean")
      : tr(locale, "diagnostic.residue_counts", {
          sessions: String(residue.sessions_remaining),
          listeners: String(residue.listeners_remaining),
          peers: String(residue.peers_remaining),
          services: String(residue.embedded_services_remaining),
          networks: String(residue.residual_networks.length),
        });

  return (
    <section className="surface-card" data-testid="runtime-diagnostics">
      <div className="card-heading">
        <h3>{label(locale, "diagnostic.title")}</h3>
        <button
          className="secondary-button"
          type="button"
          onClick={() => onRequestAction("get_node_diagnostics")}
        >
          {label(locale, "diagnostic.run")}
        </button>
      </div>

      <dl className="detail-list diagnostic-metadata">
        <div>
          <dt>{label(locale, "diagnostic.id")}</dt>
          <dd>
            <span className="diagnostic-copy-value">
              <code>{diagnostics.diagnostic_id}</code>
              <button
                className="icon-button"
                type="button"
                aria-label={label(locale, "operation.copy_diagnostic_id")}
                onClick={() => void copy(diagnostics.diagnostic_id, "id")}
              >
                {copyFeedback === "id" ? (
                  <Check size={15} />
                ) : (
                  <Copy size={15} />
                )}
              </button>
            </span>
          </dd>
        </div>
        <div>
          <dt>{label(locale, "diagnostic.started_at")}</dt>
          <dd>{formatTimestamp(locale, diagnostics.started_at_unix_ms)}</dd>
        </div>
        <div>
          <dt>{label(locale, "diagnostic.completed_at")}</dt>
          <dd>{formatTimestamp(locale, diagnostics.completed_at_unix_ms)}</dd>
        </div>
        <div>
          <dt>{label(locale, "diagnostic.duration")}</dt>
          <dd>{diagnosticDuration(locale, duration)}</dd>
        </div>
        <div>
          <dt>{label(locale, "diagnostic.timeout")}</dt>
          <dd>
            {label(
              locale,
              diagnostics.timed_out
                ? "diagnostic.timed_out_yes"
                : "diagnostic.timed_out_no",
            )}
          </dd>
        </div>
      </dl>

      <div className="diagnostic-checks">
        {DIAGNOSTIC_LAYERS.map((layer) => {
          const check = diagnostics.checks.find(
            (candidate) => candidate.layer === layer,
          );
          if (!check) {
            const stateLabel = label(locale, "diagnostic.state.not_tested");
            return (
              <article
                className="diagnostic-check neutral"
                data-diagnostic-layer={layer}
                data-diagnostic-state="not_tested"
                key={layer}
              >
                <div className="diagnostic-check-heading">
                  <strong>{label(locale, `diagnostic.layer.${layer}`)}</strong>
                  <span className="status-pill neutral">{stateLabel}</span>
                </div>
                <p>{label(locale, "diagnostic.check_missing")}</p>
              </article>
            );
          }
          const message = label(locale, check.message_key);
          const suggestion = check.suggestion_key
            ? label(locale, check.suggestion_key)
            : label(locale, "diagnostic.no_suggestion");
          return (
            <article
              className={`diagnostic-check ${diagnosticStateTone(check.state)}`}
              data-diagnostic-layer={check.layer}
              data-diagnostic-state={check.state}
              key={layer}
            >
              <div className="diagnostic-check-heading">
                <strong>{label(locale, `diagnostic.layer.${check.layer}`)}</strong>
                <span
                  className={`status-pill ${diagnosticStateTone(check.state)}`}
                >
                  {label(locale, `diagnostic.state.${check.state}`)}
                </span>
              </div>
              <p>{message}</p>
              <dl className="diagnostic-check-meta">
                <div>
                  <dt>{label(locale, "diagnostic.started_at")}</dt>
                  <dd>{formatTimestamp(locale, check.started_at_unix_ms)}</dd>
                </div>
                <div>
                  <dt>{label(locale, "diagnostic.duration")}</dt>
                  <dd>{diagnosticDuration(locale, check.duration_ms)}</dd>
                </div>
                <div>
                  <dt>{label(locale, "diagnostic.error_code")}</dt>
                  <dd>
                    <code>{check.error_code ?? label(locale, "common.none")}</code>
                  </dd>
                </div>
                <div>
                  <dt>{label(locale, "diagnostic.suggestion")}</dt>
                  <dd>{suggestion}</dd>
                </div>
              </dl>
            </article>
          );
        })}
      </div>

      <div className="diagnostic-residue">
        <strong>{label(locale, "diagnostic.residue")}</strong>
        <span>{residueSummary}</span>
        {diagnostics.cleanup_candidate_networks.length > 0 ? (
          <span>
            {tr(locale, "diagnostic.cleanup_candidates", {
              networks: diagnostics.cleanup_candidate_networks.join(
                label(locale, "common.list_separator"),
              ),
            })}
          </span>
        ) : null}
      </div>

      <section className="diagnostic-recovery">
        <div>
          <h4>{label(locale, "diagnostic.recovery.title")}</h4>
          <p>{label(locale, "diagnostic.recovery.disclaimer")}</p>
        </div>
        {diagnostics.recovery_actions.length === 0 ? (
          <p className="empty-copy">
            {label(locale, "diagnostic.recovery.none")}
          </p>
        ) : (
          <div className="diagnostic-recovery-list">
            {diagnostics.recovery_actions.map((action) => (
              <article
                className="diagnostic-recovery-action"
                data-recovery-action-id={action.action_id}
                key={action.action_id}
              >
                <div className="diagnostic-recovery-heading">
                  <strong>{label(locale, `action.${action.action_id}`)}</strong>
                  <code>{action.action_id}</code>
                </div>
                <dl className="detail-list">
                  <div>
                    <dt>{label(locale, "diagnostic.recovery.impact")}</dt>
                    <dd>{label(locale, action.impact_key)}</dd>
                  </div>
                  <div>
                    <dt>{label(locale, "diagnostic.recovery.action")}</dt>
                    <dd>
                      {label(
                        locale,
                        action.confirmation_required
                          ? "diagnostic.recovery.confirmation_required"
                          : "diagnostic.recovery.no_confirmation",
                      )}
                    </dd>
                  </div>
                </dl>
              </article>
            ))}
          </div>
        )}
      </section>

      <details className="diagnostic-report">
        <summary>{label(locale, "diagnostic.report")}</summary>
        <div className="diagnostic-report-actions">
          <button
            className="secondary-button"
            type="button"
            onClick={() =>
              void copy(diagnostics.sanitized_report, "report")
            }
          >
            {copyFeedback === "report" ? (
              <Check size={14} />
            ) : (
              <Copy size={14} />
            )}
            {label(locale, "diagnostic.copy_report")}
          </button>
          {copyFeedback === "report" ? (
            <span role="status">
              {label(locale, "diagnostic.report_copied")}
            </span>
          ) : null}
        </div>
        <pre>{diagnostics.sanitized_report}</pre>
      </details>

      {copyFeedback === "failed" ? (
        <p className="form-error" role="alert">
          {label(locale, "diagnostic.copy_failed")}
        </p>
      ) : null}
    </section>
  );
}

function NodeRuntime({
  page,
  snapshot,
  locale,
  selectedNetwork,
  recentDiagnostics,
  onSelectNetwork,
  onNavigate,
  onRequestAction,
}: PageContentProps) {
  const status = primaryStatus(snapshot, locale);
  const actionPlan = runtimeActionPlan(status.id);
  const executableActions = [
    actionPlan?.primary,
    actionPlan?.secondary,
  ].flatMap((requestedAction, index) => {
    if (!requestedAction) return [];
    const action = page.actions.find(
      (candidate) => candidate.id === requestedAction.id,
    );
    if (!action || executionBlockReason(action, snapshot, selectedNetwork)) {
      return [];
    }
    return [{ action, primary: index === 0 }];
  });
  const phase = label(locale, `status.phase.${snapshot.lifecycle.phase}`);
  const controlHealth = label(
    locale,
    `status.control.${snapshot.control_health ?? "unknown"}`,
  );
  const failedNetworks = snapshot.lifecycle.failed_networks.length
    ? snapshot.lifecycle.failed_networks.join(", ")
    : label(locale, "common.none");
  const lastError = snapshot.last_error
    ? tr(
        locale,
        snapshot.last_error.message_key,
        snapshot.last_error.params ?? {},
      )
    : label(locale, "common.none");
  const interfaces = snapshot.interfaces ?? [];
  const interfaceProblems = snapshot.interface_problems ?? [];
  const routingSummaries = snapshot.routing_summaries ?? [];
  const tunCapabilities = snapshot.tun_capabilities;
  const tunPlatform = tunCapabilities
    ? label(locale, `runtime.value.tun_platform.${tunCapabilities.platform}`)
    : label(locale, "common.unknown");
  const tunSupported = tunCapabilities
    ? label(
        locale,
        tunCapabilities.supported ? "common.enabled" : "common.disabled",
      )
    : label(locale, "common.not_available");
  const tunRebuildable = tunCapabilities
    ? label(
        locale,
        tunCapabilities.can_rebuild ? "common.enabled" : "common.disabled",
      )
    : label(locale, "common.not_available");
  const tunReasons = tunCapabilities?.reasons?.length
    ? tunCapabilities.reasons
        .map((reason) => label(locale, `runtime.value.tun_reason.${reason}`))
        .join(label(locale, "common.list_separator"))
    : label(locale, "common.not_available");

  return (
    <div className="page-stack runtime-stack">
      <section className={`hero-status ${statusTone(status.id)}`}>
        <div className="hero-status-icon">
          {status.id === "running" ? (
            <CheckCircle2 size={30} />
          ) : status.id === "failed" ? (
            <AlertTriangle size={30} />
          ) : (
            <Server size={30} />
          )}
        </div>
        <div className="hero-status-copy">
          <span>{label(locale, "runtime.metric.status")}</span>
          <h2>{status.label}</h2>
          <p>{status.detail}</p>
        </div>
        {executableActions.length > 0 ? (
          <div className="runtime-action-group">
            {executableActions.map(({ action, primary }) => (
              <button
                className={
                  primary
                    ? "primary-button runtime-primary-action"
                    : "secondary-button runtime-secondary-action"
                }
                type="button"
                key={action.id}
                onClick={() => onRequestAction(action.id)}
              >
                {label(locale, action.label_key)}
              </button>
            ))}
          </div>
        ) : null}
      </section>

      <div className="metric-grid">
        <div className="metric-card">
          <Activity size={20} />
          <span>{label(locale, "runtime.metric.phase")}</span>
          <strong>{phase}</strong>
        </div>
        <div className="metric-card">
          <Cpu size={20} />
          <span>{label(locale, "runtime.metric.revision")}</span>
          <strong>{snapshot.revision}</strong>
        </div>
        <div className="metric-card">
          <Network size={20} />
          <span>{label(locale, "runtime.metric.active_networks")}</span>
          <strong>{snapshot.lifecycle.active_networks}</strong>
        </div>
        <div className="metric-card">
          <Database size={20} />
          <span>{label(locale, "runtime.metric.configured_networks")}</span>
          <strong>{snapshot.lifecycle.configured_networks}</strong>
        </div>
      </div>

      <DiagnosticsPanel
        diagnostics={recentDiagnostics}
        locale={locale}
        onRequestAction={onRequestAction}
      />

      <section className="surface-card" data-testid="runtime-state">
        <div className="card-heading">
          <h3>{label(locale, "runtime.section.state")}</h3>
          <Server size={17} />
        </div>
        <dl className="detail-list">
          <div>
            <dt>{label(locale, "runtime.row.primary_status")}</dt>
            <dd>{status.label}</dd>
          </div>
          <div>
            <dt>{label(locale, "runtime.row.phase")}</dt>
            <dd>{phase}</dd>
          </div>
          <div>
            <dt>{label(locale, "runtime.row.control_health")}</dt>
            <dd>{controlHealth}</dd>
          </div>
          <div>
            <dt>{label(locale, "runtime.row.revision")}</dt>
            <dd>{snapshot.revision}</dd>
          </div>
          <div>
            <dt>{label(locale, "runtime.row.failed_networks")}</dt>
            <dd>{failedNetworks}</dd>
          </div>
          <div>
            <dt>{label(locale, "runtime.row.last_error")}</dt>
            <dd>{lastError}</dd>
          </div>
        </dl>
      </section>

      <section className="surface-card" data-testid="runtime-tun">
        <div className="card-heading">
          <h3>{label(locale, "runtime.section.tun")}</h3>
          <ShieldCheck size={17} />
        </div>
        <dl className="detail-list">
          <div>
            <dt>{label(locale, "runtime.row.tun_platform")}</dt>
            <dd>{tunPlatform}</dd>
          </div>
          <div>
            <dt>{label(locale, "runtime.row.tun_supported")}</dt>
            <dd>{tunSupported}</dd>
          </div>
          <div>
            <dt>{label(locale, "runtime.row.tun_rebuildable")}</dt>
            <dd>{tunRebuildable}</dd>
          </div>
          <div>
            <dt>{label(locale, "runtime.row.tun_reason")}</dt>
            <dd>{tunReasons}</dd>
          </div>
        </dl>
      </section>

      <section className="surface-card" data-testid="runtime-networks">
        <div className="card-heading">
          <h3>{label(locale, "runtime.section.networks")}</h3>
          <span>{snapshot.networks.length}</span>
        </div>
        {snapshot.networks.length === 0
          ? empty(label(locale, "runtime.empty.networks"))
          : snapshot.networks.map((network) => (
              <div className="compact-row" key={network.name}>
                <CircleDot size={15} className={statusTone(network.status)} />
                <span>
                  <strong>{network.name}</strong>
                  <small>
                    {tr(locale, "runtime.row.network_detail", {
                      status: label(locale, `status.network.${network.status}`),
                      peers: network.peer_count,
                      roles:
                        network.roles.join(" · ") ||
                        label(locale, "common.none"),
                    })}
                  </small>
                </span>
              </div>
            ))}
      </section>

      <section className="surface-card" data-testid="runtime-interfaces">
        <div className="card-heading">
          <h3>{label(locale, "runtime.section.interfaces")}</h3>
          <span>{interfaces.length}</span>
        </div>
        {interfaces.length === 0
          ? empty(label(locale, "runtime.empty.interfaces"))
          : interfaces.map((iface) => {
              const addresses = iface.addresses.length
                ? iface.addresses
                    .map(
                      (address) => `${address.address}/${address.prefix_len}`,
                    )
                    .join(", ")
                : label(locale, "common.none");
              const defaults = tr(locale, "runtime.row.interface_defaults", {
                ipv4: label(
                  locale,
                  iface.default_ipv4 ? "common.enabled" : "common.disabled",
                ),
                ipv6: label(
                  locale,
                  iface.default_ipv6 ? "common.enabled" : "common.disabled",
                ),
              });
              return (
                <div
                  className="compact-row"
                  data-interface-index={iface.index}
                  key={iface.name}
                >
                  <CircleDot size={15} className={statusTone(iface.status)} />
                  <span>
                    <strong>{iface.name}</strong>
                    <small>
                      {tr(locale, "runtime.row.interface_detail", {
                        kind: label(locale, `interface.kind.${iface.kind}`),
                        ownership: label(
                          locale,
                          `interface.ownership.${iface.ownership}`,
                        ),
                        status: label(
                          locale,
                          `interface.status.${iface.status}`,
                        ),
                        mtu: iface.mtu,
                        addresses,
                      })}
                    </small>
                    <small>{defaults}</small>
                  </span>
                </div>
              );
            })}
      </section>

      <section className="surface-card" data-testid="runtime-routing">
        <div className="card-heading">
          <h3>{label(locale, "runtime.section.routing")}</h3>
          <span>{routingSummaries.length}</span>
        </div>
        {routingSummaries.length === 0
          ? empty(label(locale, "runtime.empty.routing"))
          : routingSummaries.map((summary) => {
              const revision =
                summary.last_applied_revision == null
                  ? label(locale, "common.not_available")
                  : String(summary.last_applied_revision);
              const applied =
                summary.last_applied_at_unix_ms == null
                  ? label(locale, "common.not_available")
                  : formatTimestamp(locale, summary.last_applied_at_unix_ms);
              const problem =
                summary.status === "current"
                  ? null
                  : routingProblemLabel(locale, summary.problem);
              return (
                <div
                  className="compact-row"
                  data-routing-network={summary.network_name}
                  data-routing-status={summary.status}
                  key={summary.network_name}
                >
                  <CircleDot
                    size={15}
                    className={routingTone(summary.status)}
                  />
                  <span>
                    <strong>{summary.network_name}</strong>
                    <small>
                      {tr(locale, "runtime.routing_summary_detail", {
                        status: label(
                          locale,
                          `routing.status.${summary.status}`,
                        ),
                        action: routingActionLabel(
                          locale,
                          summary.default_action,
                        ),
                        target: routingTargetLabel(
                          locale,
                          summary.effective_target,
                        ),
                        rules: formatInteger(
                          locale,
                          summary.enabled_rule_count,
                        ),
                        revision,
                        applied,
                      })}
                    </small>
                    {problem ? <small className="bad">{problem}</small> : null}
                  </span>
                  <button
                    className="secondary-button"
                    type="button"
                    onClick={() => {
                      onSelectNetwork(summary.network_name);
                      onNavigate("routing");
                    }}
                  >
                    {label(locale, "routing.manage")}
                  </button>
                </div>
              );
            })}
      </section>

      {interfaceProblems.length > 0 ? (
        <section
          className="surface-card"
          data-testid="runtime-interface-problems"
        >
          <div className="card-heading">
            <h3>{label(locale, "runtime.section.interface_problems")}</h3>
            <AlertTriangle size={17} />
          </div>
          {interfaceProblems.map((problem, index) => {
            const localized = tr(
              locale,
              problem.message_key,
              problem.params ?? {},
            );
            return (
              <div
                className="compact-row"
                key={`${problem.message_key}-${index}`}
              >
                <AlertTriangle size={15} className="bad" />
                <span>
                  <strong>
                    {problem.network_name ??
                      label(locale, "runtime.interface.host")}
                  </strong>
                  <small>
                    {localized === problem.message_key
                      ? problem.message
                      : localized}
                  </small>
                </span>
              </div>
            );
          })}
        </section>
      ) : null}

      <section className="surface-card" data-testid="runtime-services">
        <div className="card-heading">
          <h3>{label(locale, "runtime.section.services")}</h3>
          <span>{snapshot.embedded_services?.length ?? 0}</span>
        </div>
        {(snapshot.embedded_services?.length ?? 0) === 0
          ? empty(label(locale, "runtime.empty.services"))
          : (snapshot.embedded_services ?? []).map((service) => (
              <div className="compact-row" key={service.network_name}>
                <RadioTower size={15} className={statusTone(service.status)} />
                <span>
                  <strong>{service.network_name}</strong>
                  <small>
                    {tr(locale, "runtime.row.service_detail", {
                      status: label(locale, `status.service.${service.status}`),
                      signaling:
                        service.signaling_addr ??
                        label(locale, "common.not_available"),
                      relay:
                        service.relay_addr ??
                        label(locale, "common.not_available"),
                    })}
                  </small>
                </span>
              </div>
            ))}
      </section>
    </div>
  );
}

function discoveryStateMessage(
  locale: Locale,
  projection: DiscoveryProjection,
): string {
  return label(locale, `discovery.state.${projection.state}`);
}

function discoveryJoinLabel(locale: Locale, mode: DiscoveryResult["joinMode"]): string {
  switch (mode) {
    case "open":
      return label(locale, "discovery.result.open_join");
    case "approval_required":
      return label(locale, "discovery.result.request_join");
    case "invite_only":
      return label(locale, "discovery.result.use_invite");
  }
}

function DiscoveryResultCard({
  result,
  locale,
  onRequestAction,
}: {
  result: DiscoveryResult;
  locale: Locale;
  onRequestAction(
    actionId: string,
    initialValues?: ActionValues,
  ): void;
}) {
  return (
    <article
      className={`discovery-result-card${result.stale ? " stale" : ""}`}
      data-testid="discovery-result"
      data-discovery-network-id={result.networkId}
    >
      <div className="discovery-result-heading">
        <div className="discovery-result-title">
          <div className="network-icon">
            <Network size={18} />
          </div>
          <div>
            <strong>{result.title}</strong>
            <small>{result.purpose}</small>
          </div>
        </div>
        <div className="discovery-badges">
          {result.stale ? (
            <span className="status-pill warn">
              {label(locale, "discovery.result.stale")}
            </span>
          ) : null}
          <span className="status-pill neutral">
            {label(locale, `choice.visibility.${result.visibility}`)}
          </span>
          <span className="status-pill neutral">
            {label(locale, `choice.join_mode.${result.joinMode}`)}
          </span>
        </div>
      </div>
      <div className="discovery-result-meta">
        <span>{tr(locale, "discovery.result.last_verified", {
          time: formatTimestamp(locale, result.verifiedAtUnixMs),
        })}</span>
        {result.stale ? (
          <span>{label(locale, "discovery.result.stale_detail")}</span>
        ) : null}
      </div>
      <details className="discovery-result-details">
        <summary>{label(locale, "discovery.result.details")}</summary>
        <dl>
          <div>
            <dt>{label(locale, "discovery.result.network_id")}</dt>
            <dd>
              <code>{result.networkId}</code>
            </dd>
          </div>
          <div>
            <dt>{label(locale, "discovery.filter.language")}</dt>
            <dd>{result.language}</dd>
          </div>
          <div>
            <dt>{label(locale, "discovery.filter.region")}</dt>
            <dd>{result.region}</dd>
          </div>
          <div>
            <dt>{label(locale, "discovery.result.capabilities")}</dt>
            <dd>
              {result.capabilities.length > 0
                ? result.capabilities
                    .map((capability) =>
                      label(locale, `capability.${capability}`),
                    )
                    .join(" · ")
                : label(locale, "common.none")}
            </dd>
          </div>
          <div>
            <dt>{label(locale, "discovery.result.tags")}</dt>
            <dd>
              {result.tags.length > 0
                ? result.tags.join(" · ")
                : label(locale, "common.none")}
            </dd>
          </div>
        </dl>
        <p>{label(locale, "discovery.result.privacy_summary")}</p>
      </details>
      <div className="discovery-result-actions">
        <button
          className="primary-button"
          type="button"
          onClick={() =>
            onRequestAction("preview_join_network", {
              network_id: result.networkId,
            })
          }
        >
          {discoveryJoinLabel(locale, result.joinMode)}
        </button>
      </div>
    </article>
  );
}

function Networks({
  snapshot,
  locale,
  onSelectNetwork,
  onNavigate,
  onRequestAction,
}: Omit<PageContentProps, "page" | "selectedNetwork">) {
  const [searchInput, setSearchInput] = useState("");
  const [searchQuery, setSearchQuery] = useState("");
  const [filters, setFilters] = useState<DiscoveryFilters>({
    ...EMPTY_DISCOVERY_FILTERS,
  });

  useEffect(() => {
    const timer = window.setTimeout(() => setSearchQuery(searchInput), 250);
    return () => window.clearTimeout(timer);
  }, [searchInput]);

  const projection = useMemo(
    () => projectDiscovery(snapshot, searchQuery, filters),
    [filters, searchQuery, snapshot],
  );
  const facets = useMemo(() => discoveryFacets(snapshot), [snapshot]);
  const staleResults = discoveryHasStaleResults(projection);
  const discoveryActionPending = projection.state === "pending";

  function updateFilter<Key extends keyof DiscoveryFilters>(
    key: Key,
    value: DiscoveryFilters[Key],
  ): void {
    setFilters((current) => ({ ...current, [key]: value }));
  }

  return (
    <div className="network-page-stack">
      <section
        className="surface-card discovery-section"
        data-testid="joined-networks-section"
      >
        <div className="card-heading">
          <div>
            <h3>{label(locale, "discovery.section.joined")}</h3>
            <p>{label(locale, "discovery.section.joined_detail")}</p>
          </div>
          <span>{snapshot.networks.length}</span>
        </div>
        {snapshot.networks.length === 0 ? (
          <div className="empty-state">
            {label(locale, "discovery.empty.joined")}
          </div>
        ) : (
          <div className="network-grid">
            {snapshot.networks.map((network) => (
              <NetworkContextMenu
                key={network.name}
                network={network}
                locale={locale}
                onSelectNetwork={onSelectNetwork}
                onNavigate={onNavigate}
                onRequestAction={onRequestAction}
              />
            ))}
          </div>
        )}
      </section>

      {(snapshot.memberships?.length ?? 0) > 0 ? (
        <section
          className="surface-card membership-policy-section"
          data-testid="membership-policy-section"
        >
          <div className="card-heading">
            <div>
              <h3>{label(locale, "membership.policy.title")}</h3>
              <p>{label(locale, "membership.policy.local_exit_separate")}</p>
            </div>
            <span>{snapshot.memberships?.length}</span>
          </div>
          <div className="membership-policy-grid">
            {(snapshot.memberships ?? []).map((membership) => (
              <MembershipPolicyNotice
                key={membership.network_id}
                membership={membership}
                snapshot={snapshot}
                locale={locale}
                onRequestAction={onRequestAction}
              />
            ))}
          </div>
        </section>
      ) : null}

      <section
        className="surface-card discovery-section"
        data-testid="discovery-results-section"
      >
        <div className="card-heading discovery-section-heading">
          <div>
            <h3>{label(locale, "discovery.section.results")}</h3>
            <p>{label(locale, "discovery.section.results_detail")}</p>
          </div>
          <span>{projection.results.length}</span>
        </div>

        <div className="discovery-toolbar">
          <div className="discovery-search">
            <label htmlFor="discovery-query-input">
              {label(locale, "discovery.search.label")}
            </label>
            <span className="discovery-search-control">
              <Search size={15} aria-hidden="true" />
              <input
                id="discovery-query-input"
                type="search"
                value={searchInput}
                placeholder={label(locale, "discovery.search.placeholder")}
                aria-describedby="discovery-search-private-note"
                autoComplete="off"
                spellCheck={false}
                onChange={(event) => setSearchInput(event.target.value)}
              />
            </span>
            <small id="discovery-search-private-note">
              {label(locale, "discovery.search.private_note")}
            </small>
          </div>

          <div className="discovery-filters" role="group" aria-label={label(locale, "discovery.section.results")}>
            <label>
              <span>{label(locale, "discovery.filter.visibility")}</span>
              <select
                value={filters.visibility}
                onChange={(event) =>
                  updateFilter(
                    "visibility",
                    event.target.value as DiscoveryFilters["visibility"],
                  )
                }
              >
                <option value="all">{label(locale, "discovery.filter.all")}</option>
                <option value="public">{label(locale, "discovery.filter.public")}</option>
                <option value="private">{label(locale, "discovery.filter.private")}</option>
              </select>
            </label>
            <label>
              <span>{label(locale, "discovery.filter.join_mode")}</span>
              <select
                value={filters.join_mode}
                onChange={(event) =>
                  updateFilter(
                    "join_mode",
                    event.target.value as DiscoveryFilters["join_mode"],
                  )
                }
              >
                <option value="all">{label(locale, "discovery.filter.all")}</option>
                <option value="open">{label(locale, "choice.join_mode.open")}</option>
                <option value="approval_required">
                  {label(locale, "choice.join_mode.approval_required")}
                </option>
                <option value="invite_only">
                  {label(locale, "choice.join_mode.invite_only")}
                </option>
              </select>
            </label>
            <label>
              <span>{label(locale, "discovery.filter.language")}</span>
              <select
                value={filters.language}
                onChange={(event) => updateFilter("language", event.target.value)}
              >
                <option value="all">{label(locale, "discovery.filter.all")}</option>
                {facets.languages.map((language) => (
                  <option key={language} value={language}>
                    {language}
                  </option>
                ))}
              </select>
            </label>
            <label>
              <span>{label(locale, "discovery.filter.region")}</span>
              <select
                value={filters.region}
                onChange={(event) => updateFilter("region", event.target.value)}
              >
                <option value="all">{label(locale, "discovery.filter.all")}</option>
                {facets.regions.map((region) => (
                  <option key={region} value={region}>
                    {region}
                  </option>
                ))}
              </select>
            </label>
          </div>
        </div>

        <div className="discovery-status-row">
          <div className="discovery-state-stack" aria-live="polite">
            {projection.state !== "ready" ? (
              <div
                className={`discovery-state-notice ${projection.state}`}
                role={projection.state === "failed" ? "alert" : "status"}
                data-testid="discovery-state"
              >
                <span>{discoveryStateMessage(locale, projection)}</span>
                {projection.errorCategory ? (
                  <small>
                    {tr(locale, "discovery.error.detail", {
                      message: label(
                        locale,
                        `discovery.error.${projection.errorCategory}`,
                      ),
                    })}
                  </small>
                ) : null}
              </div>
            ) : null}
            {projection.partial ? (
              <span className="status-pill warn" data-testid="discovery-partial">
                {projection.rejectedRecordCount > 0
                  ? tr(locale, "discovery.state.rejected", {
                      count: String(projection.rejectedRecordCount),
                    })
                  : label(locale, "discovery.state.degraded")}
              </span>
            ) : null}
            {staleResults ? (
              <span className="status-pill warn" data-testid="discovery-stale">
                {label(locale, "discovery.state.stale")}
              </span>
            ) : null}
            {projection.lastSuccessUnixMs != null ? (
              <small className="discovery-last-success">
                {tr(locale, "discovery.last_success", {
                  time: formatTimestamp(locale, projection.lastSuccessUnixMs),
                })}
              </small>
            ) : null}
          </div>
          <div className="discovery-actions">
            <button
              className="secondary-button"
              type="button"
              disabled={discoveryActionPending}
              onClick={() => onRequestAction("refresh_discovery_index")}
            >
              <RefreshCw
                size={15}
                className={discoveryActionPending ? "spin" : undefined}
              />
              {label(locale, "discovery.action.refresh")}
            </button>
            {projection.state === "failed" ||
            projection.state === "degraded" ? (
              <button
                className="secondary-button"
                type="button"
                disabled={discoveryActionPending}
                onClick={() => onRequestAction("retry_discovery_index")}
              >
                {label(locale, "discovery.action.retry")}
              </button>
            ) : null}
          </div>
        </div>

        {projection.results.length > 0 ? (
          <div className="discovery-result-list">
            {projection.results.map((result) => (
              <DiscoveryResultCard
                key={result.networkId}
                result={result}
                locale={locale}
                onRequestAction={onRequestAction}
              />
            ))}
          </div>
        ) : null}
        {projection.noMoreResults ? (
          <p className="discovery-complete">
            <CheckCircle2 size={15} />
            {label(locale, "discovery.result.no_more_results")}
          </p>
        ) : null}
      </section>
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
  onNavigate,
}: Omit<
  PageContentProps,
  "page" | "selectedNetwork" | "onSelectNetwork" | "onNavigate"
> & { onNavigate(pageId: string): void }) {
  const identity = snapshot.identity;
  return (
    <div className="page-content-stack">
      {identity ? (
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
              <div>
                <dt>{label(locale, "identity.row.created_at")}</dt>
                <dd>
                  {identity.created_at_unix_ms != null
                    ? formatTimestamp(locale, identity.created_at_unix_ms)
                    : label(locale, "common.not_available")}
                </dd>
              </div>
            </dl>
          </div>
        </section>
      ) : (
        empty(label(locale, "common.no_data"))
      )}
      <section className="surface-card identity-guidance-card">
        <div className="card-heading">
          <h3>{label(locale, "identity.rotation.title")}</h3>
          <AlertTriangle size={17} />
        </div>
        <p className="muted-copy">
          {label(locale, "identity.rotation.impact")}
        </p>
      </section>
      <section className="surface-card identity-guidance-card">
        <div className="card-heading">
          <h3>{label(locale, "identity.security.title")}</h3>
          <ShieldCheck size={17} />
        </div>
        <p className="muted-copy">{label(locale, "identity.security.detail")}</p>
        <button
          className="secondary-button"
          type="button"
          onClick={() => onNavigate("settings")}
        >
          {label(locale, "identity.security.settings_button")}
        </button>
      </section>
    </div>
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
    <section
      className="surface-card settings-section"
      data-settings-group={group}
    >
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
              unsupportedCapabilities.join(
                label(locale, "common.list_separator"),
              ) || label(locale, "capability.exit"),
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
  const advancedGroups = advancedSettingsGroups(snapshot, locale);
  const advancedIcons: Record<string, LucideIcon> = {
    "network-listeners": Network,
    "dns-privacy": Globe2,
    "transport-limits": Gauge,
    "web-security": ShieldCheck,
    "observability-maintenance": Activity,
  };

  return (
    <div className="settings-content">
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
                  ? label(
                      locale,
                      `choice.ui_mode.${snapshot.ui_mode.effective}`,
                    )
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
                  <dd>
                    {label(locale, `status.capability.${capability.state}`)}
                  </dd>
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
                        service.relay_addr ??
                        label(locale, "common.not_available"),
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
                {discoveryRoles ||
                  label(locale, "settings.discovery_roles.none")}
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

      <section
        className="advanced-settings"
        aria-labelledby="advanced-settings-title"
      >
        <div className="advanced-settings-heading">
          <div>
            <h2 id="advanced-settings-title">
              {label(locale, "settings.advanced.title")}
            </h2>
            <p>{label(locale, "settings.advanced.description")}</p>
          </div>
          <SlidersHorizontal size={20} />
        </div>
        <div className="settings-grid advanced-settings-grid">
          {advancedGroups.map((group) => {
            const Icon = advancedIcons[group.id] ?? SlidersHorizontal;
            return (
              <SettingsSection
                key={group.id}
                group={group.id}
                title={group.title}
                icon={Icon}
              >
                <dl className="detail-list advanced-detail-list">
                  {group.rows.map((setting) => (
                    <div
                      key={setting.id}
                      data-advanced-setting-id={setting.id}
                      data-search-keywords={setting.keywords}
                    >
                      <dt>{setting.title}</dt>
                      <dd>
                        <span
                          className={`settings-state ${setting.state}`}
                          data-setting-state={setting.state}
                        >
                          {label(
                            locale,
                            `settings.advanced.state.${setting.state}`,
                          )}
                        </span>
                        <span>{setting.detail}</span>
                      </dd>
                    </div>
                  ))}
                </dl>
              </SettingsSection>
            );
          })}
        </div>
      </section>
    </div>
  );
}

const PAGE_ICONS: Record<string, LucideIcon> = {
  overview: Gauge,
  networks: Network,
  peers: Waypoints,
  node_runtime: Server,
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
    case "node_runtime":
      content = <NodeRuntime {...props} />;
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
    <section className="page-content" aria-labelledby="page-title">
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
      {props.footer}
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
