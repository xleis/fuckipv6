import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  Activity,
  ArrowRight,
  Network,
  RefreshCw,
  Route,
  SlidersHorizontal,
  X,
} from "lucide-react";
import type { ReactNode } from "react";
import type {
  BatchMemberAction,
  DirectoryLoad,
  TopologyActionSubmit,
  TopologyFilters,
} from "../topology-actions";
import type {
  MemberLifecycleAction,
  NetworkMember,
  NodeUiAction,
  TopologyNode,
  TopologySnapshot,
} from "../../../contracts/typescript/node-contract-v1";
import {
  BATCH_MEMBER_ACTIONS,
  EMPTY_TOPOLOGY_FILTERS,
  retainTopologySelection,
} from "../topology-actions";
import { i18n, label, outcomeSummary } from "../contract/invariants";
import { tr } from "../i18n";
import {
  memberExitDeclaredText,
  memberExitFreshnessText,
  memberExitReasonText,
  memberExitSourceText,
  memberExitStateClass,
  memberExitStateText,
} from "../members";
import { TopologyGovernancePanel } from "./topology-governance";
import { TopologyView } from "./topology-view";
import type { TopologyPresentationSignals } from "../topology-presentation";

type Props = {
  topology: TopologySnapshot | null;
  locale: Locale;
  preferredNetworkId?: string | null;
  actions: NodeUiAction[];
  onSubmitAction?: TopologyActionSubmit;
  presentationSignals?: TopologyPresentationSignals;
  headingActions?: ReactNode;
};

function validNetworkId(
  topology: TopologySnapshot | null,
  requested: string | null | undefined,
): string | null {
  if (!topology) return null;
  if (
    requested &&
    topology.boundaries.some((boundary) => boundary.network_id === requested)
  ) {
    return requested;
  }
  return topology.boundaries[0]?.network_id ?? null;
}

function errorText(error: unknown, locale: Locale): string {
  if (error instanceof Error && error.message) return error.message;
  return label(locale, "common.operation_failed");
}

function memberLifecycleActions(
  member: NetworkMember | undefined,
  directoryLoad: DirectoryLoad,
  networkId: string | null,
  topologyRevision: number,
  topologyGeneratedAtUnixMs: number,
  canListMembers: boolean,
  canChangeLifecycle: boolean,
): MemberLifecycleAction[] {
  if (
    !member ||
    !networkId ||
    !canListMembers ||
    !canChangeLifecycle ||
    directoryLoad.state !== "ready" ||
    directoryLoad.networkId !== networkId ||
    directoryLoad.topologyRevision !== topologyRevision ||
    directoryLoad.topologyGeneratedAtUnixMs !== topologyGeneratedAtUnixMs ||
    !directoryLoad.directory.can_manage_members
  ) {
    return [];
  }
  return (member.available_actions ?? []).filter(
    (action): action is BatchMemberAction =>
      BATCH_MEMBER_ACTIONS.includes(action as BatchMemberAction),
  );
}

function detailRole(
  locale: Locale,
  node: TopologyNode,
  networkId: string | null,
): string {
  const membership = node.networks.find(
    (network) => network.network_id === networkId,
  );
  return membership
    ? label(locale, `join_request.role.${membership.role}`)
    : label(locale, "common.not_available");
}

function TopologyNodeDetail({
  node,
  locale,
  networkId,
  member,
  generatedAtUnixMs,
  onClose,
}: {
  node: TopologyNode | null;
  locale: Locale;
  networkId: string | null;
  member: NetworkMember | null;
  generatedAtUnixMs: number;
  onClose(): void;
}) {
  const drawerRef = useRef<HTMLElement>(null);
  const closeButtonRef = useRef<HTMLButtonElement>(null);
  useEffect(() => {
    if (!node) return;
    const previousFocus = document.activeElement as HTMLElement | null;
    closeButtonRef.current?.focus();
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.preventDefault();
        onClose();
        return;
      }
      if (event.key !== "Tab") return;
      const focusable = drawerRef.current?.querySelectorAll<HTMLElement>(
        'button:not(:disabled), [href], input:not(:disabled), select:not(:disabled), textarea:not(:disabled), [tabindex]:not([tabindex="-1"])',
      );
      if (!focusable || focusable.length === 0) return;
      const first = focusable[0];
      const last = focusable[focusable.length - 1];
      if (event.shiftKey && document.activeElement === first) {
        event.preventDefault();
        last?.focus();
      } else if (!event.shiftKey && document.activeElement === last) {
        event.preventDefault();
        first?.focus();
      }
    };
    document.addEventListener("keydown", onKeyDown);
    return () => {
      document.removeEventListener("keydown", onKeyDown);
      previousFocus?.focus();
    };
  }, [node, onClose]);
  if (!node) {
    return null;
  }
  const membership = node.networks.find(
    (network) => network.network_id === networkId,
  );
  return (
    <>
      <button
        className="topology-detail-backdrop"
        type="button"
        tabIndex={-1}
        aria-label={label(locale, "topology.detail.close")}
        onClick={onClose}
      />
      <section
        ref={drawerRef}
        className="topology-node-detail topology-detail-drawer"
        role="dialog"
        aria-modal="true"
        aria-labelledby="topology-detail-title"
        aria-live="polite"
        data-testid="topology-node-detail"
      >
      <div className="topology-node-detail-heading">
        <div>
          <span>{label(locale, "topology.detail.title")}</span>
          <h2 id="topology-detail-title">
            {node.display_name?.trim() || node.node_id}
          </h2>
          <code>{node.node_id}</code>
        </div>
        <div className="topology-detail-heading-actions">
          <span className={`status-pill ${node.health}`}>
            {label(locale, `topology.health.${node.health}`)}
          </span>
          <button
            ref={closeButtonRef}
            className="icon-button"
            type="button"
            aria-label={label(locale, "topology.detail.close")}
            onClick={onClose}
          >
            <X size={16} />
          </button>
        </div>
      </div>
      <dl className="topology-node-detail-grid">
        <div>
          <dt>{label(locale, "topology.detail.role")}</dt>
          <dd>{detailRole(locale, node, networkId)}</dd>
        </div>
        <div>
          <dt>{label(locale, "topology.detail.capabilities")}</dt>
          <dd>
            {node.capabilities && node.capabilities.length > 0
              ? node.capabilities
                  .map((capability) => label(locale, `capability.${capability}`))
                  .join(label(locale, "common.list_separator"))
              : label(locale, "status.capability.unknown")}
          </dd>
        </div>
        <div>
          <dt>{label(locale, "topology.detail.health")}</dt>
          <dd>{label(locale, `topology.health.${node.health}`)}</dd>
        </div>
        <div>
          <dt>{label(locale, "topology.detail.status")}</dt>
          <dd>
            {membership
              ? label(locale, `member_directory.status.${membership.status}`)
              : member
                ? label(locale, `member_directory.status.${member.status}`)
                : label(locale, "common.unknown")}
          </dd>
        </div>
        <div>
          <dt>{label(locale, "topology.detail.presence")}</dt>
          <dd>
            {membership
              ? label(
                  locale,
                  `member_directory.presence.${membership.presence.state}`,
                )
              : member
                ? label(locale, `member_directory.presence.${member.presence.state}`)
                : label(locale, "common.unknown")}
          </dd>
        </div>
        <div>
          <dt>{label(locale, "member_directory.exit.state")}</dt>
          <dd>
            <span
              className={`status-pill ${memberExitStateClass(
                member?.exit_availability,
                generatedAtUnixMs,
              )}`}
              data-testid="topology-member-exit-state"
            >
              {memberExitStateText(
                locale,
                member?.exit_availability,
                generatedAtUnixMs,
              )}
            </span>
          </dd>
        </div>
        <div>
          <dt>{label(locale, "member_directory.exit.source")}</dt>
          <dd>
            {memberExitSourceText(
              locale,
              member?.exit_availability,
              generatedAtUnixMs,
            )}
          </dd>
        </div>
        <div>
          <dt>{label(locale, "member_directory.exit.declared")}</dt>
          <dd>
            {memberExitDeclaredText(
              locale,
              member?.exit_availability,
              generatedAtUnixMs,
            )}
          </dd>
        </div>
        <div>
          <dt>{label(locale, "member_directory.exit.observed_at")}</dt>
          <dd>
            {member?.exit_availability?.observed_at_unix_ms == null
              ? label(locale, "common.unknown")
              : i18n().formatTimestamp(
                  locale,
                  member.exit_availability.observed_at_unix_ms,
                )}
          </dd>
        </div>
        <div>
          <dt>{label(locale, "member_directory.exit.freshness")}</dt>
          <dd>
            {memberExitFreshnessText(
              locale,
              member?.exit_availability,
              generatedAtUnixMs,
            )}
          </dd>
        </div>
        <div>
          <dt>{label(locale, "member_directory.exit.reason")}</dt>
          <dd>
            {memberExitReasonText(
              locale,
              member?.exit_availability,
              generatedAtUnixMs,
            )}
          </dd>
        </div>
        <div>
          <dt>{label(locale, "topology.detail.networks")}</dt>
          <dd>
            {node.networks
              .map(
                (network) =>
                  `${network.network_id} · ${label(
                    locale,
                    `join_request.role.${network.role}`,
                  )}`,
              )
              .join(label(locale, "common.list_separator"))}
          </dd>
        </div>
      </dl>
      </section>
    </>
  );
}

export function TopologyPage({
  topology,
  locale,
  preferredNetworkId,
  actions,
  onSubmitAction,
  presentationSignals,
  headingActions,
}: Props) {
  const [selectedNetworkId, setSelectedNetworkId] = useState<string | null>(null);
  const networkId = validNetworkId(
    topology,
    selectedNetworkId ?? preferredNetworkId,
  );
  const [filters, setFilters] = useState<TopologyFilters>({
    ...EMPTY_TOPOLOGY_FILTERS,
  });
  const [selected, setSelected] = useState<Set<string>>(() => new Set());
  const [detailNodeId, setDetailNodeId] = useState<string | null>(null);
  const [requestedBatchAction, setRequestedBatchAction] =
    useState<BatchMemberAction | null>(null);
  const [selectionNotice, setSelectionNotice] = useState("");
  const [directoryLoad, setDirectoryLoad] = useState<DirectoryLoad>({
    state: "idle",
  });
  const requestSequence = useRef(0);
  const directoryNetworkRef = useRef<string | null>(null);
  const submitActionRef = useRef(onSubmitAction);
  const listActionRef = useRef<NodeUiAction | undefined>(undefined);
  const localeRef = useRef(locale);
  const topologyRevision = topology?.revision ?? 0;
  const topologyGeneratedAtUnixMs = topology?.generated_at_unix_ms ?? 0;
  const topologyRevisionRef = useRef(topologyRevision);
  const topologyGeneratedAtRef = useRef(topologyGeneratedAtUnixMs);
  useEffect(() => {
    submitActionRef.current = onSubmitAction;
  }, [onSubmitAction]);
  useEffect(() => {
    topologyRevisionRef.current = topologyRevision;
    topologyGeneratedAtRef.current = topologyGeneratedAtUnixMs;
  }, [topologyGeneratedAtUnixMs, topologyRevision]);

  const listAction = useMemo(
    () => actions.find((action) => action.id === "list_network_members"),
    [actions],
  );
  const previewAction = useMemo(
    () => actions.find((action) => action.id === "preview_member_removal"),
    [actions],
  );
  const lifecycleAction = useMemo(
    () => actions.find((action) => action.id === "change_member_lifecycle"),
    [actions],
  );
  const listActionAvailable = Boolean(listAction);
  useEffect(() => {
    listActionRef.current = listAction;
    localeRef.current = locale;
  }, [listAction, locale]);

  const refreshDirectory = useCallback(async () => {
    const activeNetworkId = networkId;
    const request = ++requestSequence.current;
    const submitAction = submitActionRef.current;
    const activeListAction = listActionRef.current;
    const activeLocale = localeRef.current;
    const requestedTopologyRevision = topologyRevisionRef.current;
    const requestedTopologyGeneratedAt = topologyGeneratedAtRef.current;
    if (!activeNetworkId || !submitAction || !activeListAction) {
      setDirectoryLoad({ state: "idle" });
      return;
    }
    setDirectoryLoad({ state: "loading", networkId: activeNetworkId });
    try {
      const submission = await submitAction(
        activeListAction,
        { network_id: activeNetworkId },
        () => {},
      );
      if (
        request !== requestSequence.current ||
        networkId !== activeNetworkId
      ) {
        return;
      }
      if (
        submission.outcome?.status === "network_members" &&
        submission.outcome.data.network_id === activeNetworkId
      ) {
        setDirectoryLoad({
          state: "ready",
          networkId: activeNetworkId,
          directory: submission.outcome.data,
          topologyRevision: requestedTopologyRevision,
          topologyGeneratedAtUnixMs: requestedTopologyGeneratedAt,
        });
        return;
      }
      setDirectoryLoad({
        state: "error",
        networkId: activeNetworkId,
        message: submission.outcome
          ? outcomeSummary(submission.outcome, activeLocale)
          : label(activeLocale, "topology.batch.directory_failed"),
      });
    } catch (error) {
      if (
        request !== requestSequence.current ||
        networkId !== activeNetworkId
      ) {
        return;
      }
      setDirectoryLoad({
        state: "error",
        networkId: activeNetworkId,
        message: errorText(error, activeLocale),
      });
    }
  }, [networkId]);

  useEffect(() => {
    if (!networkId) {
      directoryNetworkRef.current = null;
      return;
    }
    if (!listActionAvailable) return;
    const needsInitialLoad = directoryNetworkRef.current !== networkId;
    const needsRefresh =
      directoryLoad.state === "ready" &&
      directoryLoad.networkId === networkId &&
      (directoryLoad.topologyRevision !== topologyRevision ||
        directoryLoad.topologyGeneratedAtUnixMs !== topologyGeneratedAtUnixMs);
    if (!needsInitialLoad && !needsRefresh) return;
    // Manifest refreshes may briefly replace the action objects after a
    // command. Latch the network so equivalent manifest generations cannot
    // recursively trigger another directory command. A newer topology token
    // still forces a directory refresh so revoked member actions disappear.
    directoryNetworkRef.current = networkId;
    const timer = window.setTimeout(() => {
      void refreshDirectory();
    }, 0);
    return () => window.clearTimeout(timer);
  }, [
    directoryLoad,
    listActionAvailable,
    networkId,
    refreshDirectory,
    topologyGeneratedAtUnixMs,
    topologyRevision,
  ]);

  const directory =
    listActionAvailable &&
    directoryLoad.state === "ready" &&
    directoryLoad.networkId === networkId
      ? directoryLoad.directory
      : null;
  const membersByNodeId = useMemo(
    () =>
      new Map(
        (directory?.members ?? []).map((member) => [member.node_id, member]),
      ),
    [directory],
  );
  const selectedSet = useMemo(
    () =>
      topology
        ? retainTopologySelection(topology, selected)
        : new Set<string>(),
    [selected, topology],
  );

  useEffect(() => {
    if (!topology) return;
    // Drop vanished identities after the committed render. Keeping this out of
    // the render path prevents a refresh from resurrecting a removed node when
    // that identity later reappears in a subsequent snapshot.
    const timer = window.setTimeout(() => {
      setSelected((current) => {
        const retained = retainTopologySelection(topology, current);
        return retained.size === current.size ? current : retained;
      });
    }, 0);
    return () => window.clearTimeout(timer);
  }, [topology]);
  const detailNode =
    topology?.nodes.find((node) => node.node_id === detailNodeId) ?? null;
  const networkNodes =
    topology?.nodes.filter(
      (node) =>
        !networkId ||
        node.networks.some((network) => network.network_id === networkId),
    ) ?? [];
  const networkConnections =
    topology?.connections.filter(
      (connection) =>
        !networkId || connection.networks?.includes(networkId),
    ) ?? [];
  const currentNode =
    networkNodes.find((node) => node.is_self) ?? networkNodes[0] ?? null;
  const exitNode =
    networkNodes.find((node) => node.capabilities?.includes("exit")) ??
    null;
  const relayNode =
    networkNodes.find(
      (node) =>
        node.node_id !== currentNode?.node_id &&
        node.node_id !== exitNode?.node_id &&
        node.capabilities?.includes("relay") &&
        (node.networks.some((network) => network.network_id === networkId) ||
          !networkId),
    ) ?? null;
  const pathNodes = [currentNode, relayNode, exitNode].filter(
    (node, index, nodes): node is TopologyNode =>
      Boolean(node) && nodes.findIndex((item) => item?.node_id === node?.node_id) === index,
  );
  const activeConnections =
    networkConnections.filter((connection) => connection.state === "active")
      .length;
  const healthyNodes =
    networkNodes.filter((node) => node.health === "healthy").length;
  const currentNetwork = topology?.boundaries.find(
    (boundary) => boundary.network_id === networkId,
  );
  const recentConnections = [...networkConnections]
    .sort(
      (left, right) =>
        (right.observed_at_unix_ms ?? 0) - (left.observed_at_unix_ms ?? 0),
    )
    .slice(0, 3);

  function selectNetwork(nextNetworkId: string): void {
    if (nextNetworkId === networkId) return;
    setSelectedNetworkId(nextNetworkId);
    setSelected(new Set());
    setDetailNodeId(null);
    setRequestedBatchAction(null);
    setSelectionNotice("");
    setFilters((current) => ({ ...current, networkId: "all" }));
  }

  const toggleNode = useCallback((nodeId: string): void => {
    setSelected((current) => {
      const next = new Set(current);
      if (next.has(nodeId)) next.delete(nodeId);
      else next.add(nodeId);
      return next;
    });
    setSelectionNotice("");
  }, []);

  const openNode = useCallback((nodeId: string): void => {
    setDetailNodeId(nodeId);
  }, []);

  const clearSelection = useCallback((): void => {
    setSelected(new Set());
    setSelectionNotice("");
  }, []);

  function changeFilters(next: TopologyFilters): void {
    const changed =
      next.search !== filters.search ||
      next.networkId !== filters.networkId ||
      next.role !== filters.role ||
      next.capability !== filters.capability ||
      next.health !== filters.health;
    if (!changed) return;
    if (selectedSet.size > 0) {
      setSelected(new Set());
      setDetailNodeId(null);
      setSelectionNotice(label(locale, "topology.selection.filter_changed"));
    }
    setFilters(next);
  }

  const prepareLifecycle = useCallback((
    nodeId: string,
    action: BatchMemberAction,
  ): void => {
    const governance = document.getElementById("topology-governance");
    const disclosure = governance?.closest("details");
    if (disclosure instanceof HTMLDetailsElement) disclosure.open = true;
    setSelected(new Set([nodeId]));
    setRequestedBatchAction(action);
    (disclosure ?? governance)?.scrollIntoView({ block: "nearest" });
  }, []);

  function lifecycleActionsFor(nodeId: string): MemberLifecycleAction[] {
    return memberLifecycleActions(
      membersByNodeId.get(nodeId),
      directoryLoad,
      networkId,
      topologyRevision,
      topologyGeneratedAtUnixMs,
      listActionAvailable,
      Boolean(lifecycleAction),
    );
  }

  if (!topology) {
    // No topology data yet: the workspace keeps its shape so the canvas stays
    // the visual centre, but nothing pretends to be a node or a link.
    return (
      <div className="topology-page">
        <section
          className="topology-state-band"
          aria-label={label(locale, "topology.hero.status")}
        >
          <div className="topology-state-primary">
            <span className="topology-state-copy">
              <strong>{label(locale, "topology.state.empty.title")}</strong>
              <span>{label(locale, "topology.empty.no_networks")}</span>
            </span>
          </div>
        </section>
        <div className="topology-stage topology-stage-empty">
          <div className="topology-floating-toolbar" aria-hidden="true">
            <span className="topology-zoom-status">100%</span>
          </div>
          <div className="topology-empty-card">
            <span className="topology-empty-title">
              {label(locale, "topology.list.self")}
            </span>
            <span className="topology-empty-detail">
              {label(locale, "topology.state.empty.title")}
            </span>
          </div>
          <div className="topology-canvas-footer">
            <div className="topology-legend" aria-hidden="true" />
          </div>
        </div>
        <div className="topology-insights">
          <section className="topology-insight-card">
            <div className="topology-insight-heading">
              <div>
                <h2>{label(locale, "topology.path.title")}</h2>
                <span>{label(locale, "topology.path.auto")}</span>
              </div>
            </div>
            <p className="topology-insight-empty">
              {label(locale, "topology.path.empty")}
            </p>
          </section>
          <section className="topology-insight-card">
            <div className="topology-insight-heading">
              <div>
                <h2>{label(locale, "topology.events.title")}</h2>
                <span>{label(locale, "topology.events.window")}</span>
              </div>
            </div>
            <p className="topology-insight-empty">
              {label(locale, "topology.events.empty")}
            </p>
          </section>
        </div>
      </div>
    );
  }

  return (
    <div className="topology-page">
      <header className="topology-hero">
        <div className="topology-hero-copy">
          <span className="topology-hero-icon" aria-hidden="true">
            <Network size={20} />
          </span>
          <div className="topology-hero-title">
            <span className="topology-eyebrow">
              {label(locale, "topology.hero.eyebrow")}
            </span>
            <h1 id="page-title">{label(locale, "page.topology")}</h1>
            <p>
              {tr(locale, "topology.hero.summary", {
                networks: String(topology.boundaries.length),
                nodes: String(networkNodes.length),
                exit: exitNode
                  ? (exitNode.display_name?.trim() || exitNode.node_id)
                  : label(locale, "common.none"),
              })}
            </p>
          </div>
        </div>
        <div className="topology-hero-actions">
          {headingActions}
          <button
            className="primary-icon-button"
            type="button"
            title={label(locale, "topology.hero.sync")}
            aria-label={label(locale, "topology.hero.sync")}
            disabled={directoryLoad.state === "loading" || !listAction}
            onClick={() => void refreshDirectory()}
          >
            <RefreshCw
              className={directoryLoad.state === "loading" ? "spin" : undefined}
              size={18}
            />
          </button>
        </div>
      </header>

      <section className="topology-state-band" aria-label={label(locale, "topology.hero.status")}>
        <div className="topology-state-primary">
          <span className={`topology-state-icon ${presentationSignals?.stale ? "warn" : "good"}`}>
            <Route size={20} aria-hidden="true" />
          </span>
          <span className="topology-state-copy">
            <strong>
              {presentationSignals?.stale
                ? label(locale, "state.stale.title")
                : label(locale, "topology.hero.verified")}
            </strong>
            <span>
              {pathNodes.length > 0
                ? tr(locale, "topology.hero.path", {
                    path: pathNodes
                      .map((node) => node.display_name?.trim() || node.node_id)
                      .join(" → "),
                  })
                : label(locale, "topology.path.empty")}
            </span>
          </span>
        </div>
        <div className="topology-stat">
          <span>{label(locale, "topology.metric.connections")}</span>
          <strong>{activeConnections}</strong>
          <small>{label(locale, "topology.connection.active")}</small>
        </div>
        <div className="topology-stat">
          <span>{label(locale, "topology.metric.nodes")}</span>
          <strong>{networkNodes.length}</strong>
          <small>
            {tr(locale, "topology.metric.healthy", {
              count: String(healthyNodes),
            })}
          </small>
        </div>
        <div className="topology-stat">
          <span>{label(locale, "topology.metric.network")}</span>
          <strong>{topology.boundaries.length}</strong>
          <small>{currentNetwork?.name ?? label(locale, "common.none")}</small>
        </div>
      </section>

      <TopologyView
        topology={topology}
        locale={locale}
        presentationSignals={presentationSignals}
        filters={filters}
        onFiltersChange={changeFilters}
        selectedNodeIds={selectedSet}
        onToggleNode={toggleNode}
        onOpenNode={openNode}
        networkId={networkId}
        membersByNodeId={membersByNodeId}
        lifecycleActions={lifecycleActionsFor}
        onPrepareLifecycle={prepareLifecycle}
        onClearSelection={clearSelection}
        onSelectNetwork={selectNetwork}
        selectionNotice={selectionNotice}
      />

      <div className="topology-insights">
        <section className="topology-insight-card">
          <div className="topology-insight-heading">
            <div>
              <h2>{label(locale, "topology.path.title")}</h2>
              <span>{label(locale, "topology.path.auto")}</span>
            </div>
            <Route size={16} aria-hidden="true" />
          </div>
          {pathNodes.length > 0 ? (
            <div className="topology-path">
              {pathNodes.map((node, index) => (
                <span className="topology-path-segment" key={node.node_id}>
                  {index > 0 ? (
                    <ArrowRight size={14} aria-hidden="true" />
                  ) : null}
                  <span className="topology-path-node">
                    <Network size={14} aria-hidden="true" />
                    <span>{node.display_name?.trim() || node.node_id}</span>
                  </span>
                </span>
              ))}
            </div>
          ) : (
            <p className="topology-insight-empty">
              {label(locale, "topology.path.empty")}
            </p>
          )}
        </section>

        <section className="topology-insight-card">
          <div className="topology-insight-heading">
            <div>
              <h2>{label(locale, "topology.events.title")}</h2>
              <span>{label(locale, "topology.events.window")}</span>
            </div>
            <Activity size={16} aria-hidden="true" />
          </div>
          {recentConnections.length > 0 ? (
            <ul className="topology-event-list">
              {recentConnections.map((connection, index) => (
                <li key={connection.connection_id}>
                  <span className={`topology-event-dot ${connection.state}`} />
                  <span>
                    <strong>
                      {tr(locale, "topology.events.connection", {
                        from:
                          topology.nodes.find(
                            (node) => node.node_id === connection.from,
                          )?.display_name ?? connection.from,
                        to:
                          topology.nodes.find(
                            (node) => node.node_id === connection.to,
                          )?.display_name ?? connection.to,
                        state: label(
                          locale,
                          `topology.connection.${connection.state}`,
                        ),
                      })}
                    </strong>
                    <small>
                      {connection.observed_at_unix_ms
                        ? i18n().formatTimestamp(
                            locale,
                            connection.observed_at_unix_ms,
                          )
                        : label(locale, "common.unknown")}
                    </small>
                  </span>
                  <time>{index + 1}</time>
                </li>
              ))}
            </ul>
          ) : (
            <p className="topology-insight-empty">
              {label(locale, "topology.events.empty")}
            </p>
          )}
        </section>
      </div>

      <details className="topology-governance-disclosure">
        <summary>
          <SlidersHorizontal size={16} aria-hidden="true" />
          <span>{label(locale, "topology.governance.more")}</span>
        </summary>
        <TopologyGovernancePanel
          key={networkId ?? "no-network"}
          topology={topology}
          locale={locale}
          networks={topology.boundaries}
          networkId={networkId}
          onNetworkIdChange={selectNetwork}
          selected={selectedSet}
          directoryLoad={directoryLoad}
          lifecycleAction={lifecycleAction}
          previewAction={previewAction}
          requestedAction={requestedBatchAction}
          onActionChange={setRequestedBatchAction}
          onSubmit={onSubmitAction}
          onRefreshDirectory={() => void refreshDirectory()}
          onSelectedChange={setSelected}
        />
      </details>

      {detailNode ? (
        <TopologyNodeDetail
          node={detailNode}
          locale={locale}
          networkId={networkId}
          member={membersByNodeId.get(detailNode.node_id) ?? null}
          generatedAtUnixMs={topologyGeneratedAtUnixMs}
          onClose={() => setDetailNodeId(null)}
        />
      ) : null}
    </div>
  );
}
