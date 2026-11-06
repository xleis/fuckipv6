import type { TopologySnapshot } from "../../contracts/typescript/node-contract-v1";

export type TopologyPresentationCondition =
  | "ready"
  | "initial_loading"
  | "partial"
  | "stale"
  | "offline"
  | "empty"
  | "error"
  | "permission_denied"
  | "refreshing"
  | "unavailable";

export type TopologyPresentationSignals = {
  initialLoading?: boolean;
  refreshing?: boolean;
  offline?: boolean;
  stale?: boolean;
  error?: string | null;
  permissionDenied?: boolean;
};

export type TopologyPresentation = {
  kind: TopologyPresentationCondition;
  conditions: TopologyPresentationCondition[];
  tone: "neutral" | "info" | "warn" | "danger" | "success";
  hasContent: boolean;
  canRender: boolean;
  titleKey: string;
  detailKey: string;
};

type PresentationDefinition = Omit<
  TopologyPresentation,
  "kind" | "conditions" | "hasContent" | "canRender"
> & { priority: number };

const DEFINITIONS: Record<
  TopologyPresentationCondition,
  PresentationDefinition
> = {
  error: {
    priority: 10,
    tone: "danger",
    titleKey: "state.error.title",
    detailKey: "state.error.detail",
  },
  permission_denied: {
    priority: 20,
    tone: "warn",
    titleKey: "state.permission_denied.title",
    detailKey: "state.permission_denied.detail",
  },
  initial_loading: {
    priority: 30,
    tone: "info",
    titleKey: "state.initial_loading.title",
    detailKey: "state.initial_loading.detail",
  },
  offline: {
    priority: 40,
    tone: "danger",
    titleKey: "state.offline.title",
    detailKey: "state.offline.detail",
  },
  stale: {
    priority: 50,
    tone: "warn",
    titleKey: "state.stale.title",
    detailKey: "state.stale.detail",
  },
  refreshing: {
    priority: 60,
    tone: "info",
    titleKey: "state.refreshing.title",
    detailKey: "state.refreshing.detail",
  },
  partial: {
    priority: 70,
    tone: "warn",
    titleKey: "topology.state.partial.title",
    detailKey: "topology.state.partial.detail",
  },
  unavailable: {
    priority: 80,
    tone: "neutral",
    titleKey: "topology.state.unavailable.title",
    detailKey: "topology.empty.unavailable",
  },
  empty: {
    priority: 90,
    tone: "neutral",
    titleKey: "topology.state.empty.title",
    detailKey: "topology.empty.no_networks",
  },
  ready: {
    priority: 1_000,
    tone: "success",
    titleKey: "state.ready.title",
    detailKey: "state.ready.detail",
  },
};

function activeConditions(
  topology: TopologySnapshot | null,
  signals: TopologyPresentationSignals,
): TopologyPresentationCondition[] {
  const conditions: TopologyPresentationCondition[] = [];
  if (signals.error) conditions.push("error");
  if (signals.permissionDenied) conditions.push("permission_denied");
  if (signals.initialLoading) conditions.push("initial_loading");
  if (signals.offline) conditions.push("offline");
  if (signals.stale) conditions.push("stale");
  if (signals.refreshing) conditions.push("refreshing");
  if (topology?.truncated) conditions.push("partial");
  if (topology === null) conditions.push("unavailable");
  else if (topology.boundaries.length === 0) conditions.push("empty");
  return conditions.length > 0 ? conditions : ["ready"];
}

export function topologyPresentation(
  topology: TopologySnapshot | null,
  signals: TopologyPresentationSignals = {},
): TopologyPresentation {
  const conditions = activeConditions(topology, signals).sort(
    (left, right) => DEFINITIONS[left].priority - DEFINITIONS[right].priority,
  );
  const kind = conditions[0] ?? "ready";
  const definition = DEFINITIONS[kind];
  const hasContent = topology !== null && topology.boundaries.length > 0;
  return {
    kind,
    conditions,
    tone: definition.tone,
    hasContent,
    canRender:
      hasContent && kind !== "permission_denied" && kind !== "initial_loading",
    titleKey: definition.titleKey,
    detailKey: definition.detailKey,
  };
}

export function topologyConditionCopy(
  condition: TopologyPresentationCondition,
): Pick<TopologyPresentation, "titleKey" | "detailKey" | "tone"> {
  const definition = DEFINITIONS[condition];
  return {
    titleKey: definition.titleKey,
    detailKey: definition.detailKey,
    tone: definition.tone,
  };
}
