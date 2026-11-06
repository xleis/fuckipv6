import { useCallback, useEffect, useMemo, useRef, useState } from "react";
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
import { label, outcomeSummary } from "../contract/invariants";
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
    !directoryLoad.directory.can_manage_members ||
    directoryLoad.directory.viewer_role === "member"
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
}: {
  node: TopologyNode | null;
  locale: Locale;
  networkId: string | null;
  member: NetworkMember | null;
}) {
  if (!node) {
    return (
      <section className="topology-node-detail topology-node-detail-empty">
        <strong>{label(locale, "topology.detail.empty_title")}</strong>
        <p>{label(locale, "topology.detail.empty_hint")}</p>
      </section>
    );
  }
  const membership = node.networks.find(
    (network) => network.network_id === networkId,
  );
  return (
    <section
      className="topology-node-detail"
      aria-live="polite"
      data-testid="topology-node-detail"
    >
      <div className="topology-node-detail-heading">
        <div>
          <span>{label(locale, "topology.detail.title")}</span>
          <h2>{node.display_name?.trim() || node.node_id}</h2>
          <code>{node.node_id}</code>
        </div>
        <span className={`status-pill ${node.health}`}>
          {label(locale, `topology.health.${node.health}`)}
        </span>
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
  );
}

export function TopologyPage({
  topology,
  locale,
  preferredNetworkId,
  actions,
  onSubmitAction,
  presentationSignals,
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
    setDetailNodeId(nodeId);
    setSelectionNotice("");
  }, []);

  const openNode = useCallback((nodeId: string): void => {
    setDetailNodeId(nodeId);
    document
      .getElementById("topology-node-detail")
      ?.scrollIntoView({ block: "nearest" });
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
    setSelected(new Set([nodeId]));
    setDetailNodeId(nodeId);
    setRequestedBatchAction(action);
    document
      .getElementById("topology-governance")
      ?.scrollIntoView({ block: "nearest" });
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
    return (
      <TopologyView
        topology={null}
        locale={locale}
        presentationSignals={presentationSignals}
        filters={filters}
        onFiltersChange={setFilters}
      />
    );
  }

  return (
    <div className="topology-page">
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
        selectionNotice={selectionNotice}
      />
      <section id="topology-node-detail">
        <TopologyNodeDetail
          node={detailNode}
          locale={locale}
          networkId={networkId}
          member={
            detailNode ? (membersByNodeId.get(detailNode.node_id) ?? null) : null
          }
        />
      </section>
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
    </div>
  );
}
