import type { NodeSnapshot } from "../../contracts/typescript/node-contract-v1";

export type PageStateKind =
  | "initial_loading"
  | "error"
  | "maintenance"
  | "permission_denied"
  | "unsupported"
  | "offline"
  | "control_unavailable"
  | "node_failed"
  | "degraded"
  | "stale"
  | "refreshing"
  | "empty"
  | "ready";

export type PageStateTone = "neutral" | "info" | "warn" | "danger" | "success";

export type PageState = {
  kind: PageStateKind;
  priority: number;
  tone: PageStateTone;
  blocking: boolean;
  titleKey: string;
  detailKey: string;
};

export type PageStateSignals = {
  initialLoading?: boolean;
  refreshing?: boolean;
  error?: string | null;
  maintenance?: boolean;
  permissionDenied?: boolean;
  unsupported?: boolean;
  browserOnline?: boolean;
  controlHealth?: "available" | "unavailable" | "unknown";
  nodePhase?: string;
  degraded?: boolean;
  stale?: boolean;
  empty?: boolean;
  lastAuthoritativeAt?: number | null;
};

const DEFINITIONS: Record<
  PageStateKind,
  Omit<PageState, "kind"> & { when(signals: PageStateSignals): boolean }
> = {
  initial_loading: {
    // Initial loading must not mask an actual error, but it remains stronger
    // than the empty/ready fallbacks while the first authoritative read is in
    // flight.
    priority: 20,
    tone: "info",
    blocking: true,
    titleKey: "state.initial_loading.title",
    detailKey: "state.initial_loading.detail",
    when: (signals) => signals.initialLoading === true,
  },
  error: {
    priority: 10,
    tone: "danger",
    blocking: true,
    titleKey: "state.error.title",
    detailKey: "state.error.detail",
    when: (signals) => Boolean(signals.error),
  },
  maintenance: {
    priority: 40,
    tone: "warn",
    blocking: true,
    titleKey: "state.maintenance.title",
    detailKey: "state.maintenance.detail",
    when: (signals) => signals.maintenance === true,
  },
  permission_denied: {
    priority: 50,
    tone: "warn",
    blocking: true,
    titleKey: "state.permission_denied.title",
    detailKey: "state.permission_denied.detail",
    when: (signals) => signals.permissionDenied === true,
  },
  unsupported: {
    priority: 60,
    tone: "neutral",
    blocking: true,
    titleKey: "state.unsupported.title",
    detailKey: "state.unsupported.detail",
    when: (signals) => signals.unsupported === true,
  },
  offline: {
    priority: 30,
    tone: "danger",
    blocking: true,
    titleKey: "state.offline.title",
    detailKey: "state.offline.detail",
    when: (signals) => signals.browserOnline === false,
  },
  control_unavailable: {
    priority: 90,
    tone: "danger",
    blocking: true,
    titleKey: "state.control_unavailable.title",
    detailKey: "state.control_unavailable.detail",
    when: (signals) => signals.controlHealth === "unavailable",
  },
  node_failed: {
    priority: 70,
    tone: "danger",
    blocking: true,
    titleKey: "state.node_failed.title",
    detailKey: "state.node_failed.detail",
    when: (signals) => signals.nodePhase === "failed",
  },
  degraded: {
    priority: 80,
    tone: "warn",
    blocking: false,
    titleKey: "state.degraded.title",
    detailKey: "state.degraded.detail",
    when: (signals) => signals.degraded === true,
  },
  stale: {
    priority: 100,
    tone: "warn",
    blocking: true,
    titleKey: "state.stale.title",
    detailKey: "state.stale.detail",
    when: (signals) => signals.stale === true,
  },
  refreshing: {
    priority: 110,
    tone: "info",
    blocking: false,
    titleKey: "state.refreshing.title",
    detailKey: "state.refreshing.detail",
    when: (signals) => signals.refreshing === true,
  },
  empty: {
    priority: 120,
    tone: "neutral",
    blocking: false,
    titleKey: "state.empty.title",
    detailKey: "state.empty.detail",
    when: (signals) => signals.empty === true,
  },
  ready: {
    priority: 1000,
    tone: "success",
    blocking: false,
    titleKey: "state.ready.title",
    detailKey: "state.ready.detail",
    when: () => true,
  },
};

export function primaryPageState(signals: PageStateSignals): PageState {
  const kind = (Object.keys(DEFINITIONS) as PageStateKind[])
    .filter((candidate) => candidate === "ready" || DEFINITIONS[candidate].when(signals))
    .sort(
      (left, right) =>
        DEFINITIONS[left].priority - DEFINITIONS[right].priority,
    )[0];
  return pageStateDefinition(kind ?? "ready");
}

export function pageStateDefinition(kind: PageStateKind): PageState {
  const definition = DEFINITIONS[kind];
  return {
    kind,
    priority: definition.priority,
    tone: definition.tone,
    blocking: definition.blocking,
    titleKey: definition.titleKey,
    detailKey: definition.detailKey,
  };
}

export function interactionBlockReason(
  signals: PageStateSignals,
): PageStateKind | null {
  const state = primaryPageState(signals);
  return state.blocking && state.kind !== "initial_loading" ? state.kind : null;
}

export function pageUnsupported(
  snapshot: NodeSnapshot,
  pageId: string,
): boolean {
  const kind =
    pageId === "exit_node"
      ? "exit"
      : pageId === "relay"
        ? "relay"
        : null;
  if (!kind) return false;
  const capability = snapshot.capabilities?.capabilities.find(
    (candidate) => candidate.kind === kind,
  );
  if (capability?.state === "unsupported") return true;
  if (capability?.state !== "unknown") return false;
  return kind === "exit"
    ? snapshot.exit_node_capabilities?.supported === false
    : false;
}

export function pageHasContent(
  snapshot: NodeSnapshot,
  pageId: string,
): boolean {
  switch (pageId) {
    case "networks":
    case "topology":
    case "traffic":
      return snapshot.networks.length > 0;
    case "peers":
      return snapshot.peers.length > 0;
    case "routing":
    case "exit_node":
    case "relay":
      return (snapshot.configuration?.networks.length ?? 0) > 0;
    case "logs":
      return snapshot.logs.length > 0;
    case "identity":
      return snapshot.identity != null;
    case "overview":
    case "node_runtime":
    case "settings":
    default:
      return true;
  }
}

export function pageStateSignals(
  snapshot: NodeSnapshot,
  pageId: string,
  options: {
    browserOnline?: boolean;
    refreshing?: boolean;
    stale?: boolean;
    lastAuthoritativeAt?: number | null;
    permissionDenied?: boolean;
    unsupported?: boolean;
    maintenance?: boolean;
    error?: string | null;
  } = {},
): PageStateSignals {
  return {
    browserOnline: options.browserOnline,
    refreshing: options.refreshing,
    stale: options.stale,
    empty: !pageHasContent(snapshot, pageId),
    error: options.error,
    permissionDenied: options.permissionDenied,
    unsupported: options.unsupported ?? pageUnsupported(snapshot, pageId),
    maintenance: options.maintenance,
    controlHealth: snapshot.control_health ?? "unknown",
    nodePhase: snapshot.lifecycle.phase,
    degraded:
      snapshot.last_error != null ||
      snapshot.lifecycle.failed_networks.length > 0 ||
      snapshot.networks.some((network) =>
        ["connecting", "degraded", "stopping", "failed"].includes(network.status),
      ) ||
      (snapshot.embedded_services ?? []).some((service) =>
        ["starting", "stopping", "failed"].includes(service.status),
      ),
    lastAuthoritativeAt: options.lastAuthoritativeAt,
  };
}
