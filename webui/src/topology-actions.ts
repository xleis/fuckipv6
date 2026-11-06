import type {
  MemberLifecycleAction,
  NetworkMember,
  NetworkMemberDirectory,
  NodeCommandOutcome,
  NodeOperationSnapshot,
  NodeUiAction,
  TopologyNode,
  TopologyNodeHealth,
  TopologySnapshot,
} from "../../contracts/typescript/node-contract-v1";
import type { ActionValues } from "./contract/invariants";

/// Lifecycle actions that make sense for an existing, visible topology member.
/// Invitation targets non-members; exit is self-service only and is handled by
/// the member's own session rather than a manager batch.
export const BATCH_MEMBER_ACTIONS = [
  "approve",
  "reject",
  "suspend",
  "resume",
  "remove",
  "undo_remove",
] as const satisfies readonly MemberLifecycleAction[];

export type BatchMemberAction = (typeof BATCH_MEMBER_ACTIONS)[number];

export type TopologyActionSubmit = (
  action: NodeUiAction,
  values: ActionValues,
  onOperation: (operation: NodeOperationSnapshot) => void,
) => Promise<{
  outcome?: NodeCommandOutcome;
  operation?: NodeOperationSnapshot;
}>;

export type DirectoryLoad =
  | { state: "idle" }
  | { state: "loading"; networkId: string }
  | {
      state: "ready";
      networkId: string;
      directory: NetworkMemberDirectory;
      topologyRevision: number;
    }
  | { state: "error"; networkId: string; message: string };

export type TopologyFilters = {
  search: string;
  networkId: string | "all";
  role: string | "all";
  capability: string | "all";
  health: TopologyNodeHealth | "all";
};

export const EMPTY_TOPOLOGY_FILTERS: TopologyFilters = {
  search: "",
  networkId: "all",
  role: "all",
  capability: "all",
  health: "all",
};

export type BatchSkipReason =
  | "member_missing"
  | "not_manageable"
  | "action_unavailable";

export type BatchCandidate = {
  nodeId: string;
  name: string;
  member: NetworkMember | null;
  eligible: boolean;
  skipReason?: BatchSkipReason;
};

export type BatchPlan = {
  candidates: BatchCandidate[];
  eligible: BatchCandidate[];
  skipped: BatchCandidate[];
  actionOptions: Array<{
    action: BatchMemberAction;
    eligibleCount: number;
    totalCount: number;
  }>;
};

/** Keep selected objects only while their identities remain in the snapshot. */
export function retainTopologySelection(
  snapshot: TopologySnapshot,
  selected: ReadonlySet<string>,
): Set<string> {
  const existing = new Set(snapshot.nodes.map((node) => node.node_id));
  return new Set([...selected].filter((nodeId) => existing.has(nodeId)));
}

/** Shared graph/list query so both views cannot disagree about visible nodes. */
export function queryTopology(
  snapshot: TopologySnapshot,
  filters: TopologyFilters,
): TopologyNode[] {
  const needle = filters.search.trim().toLocaleLowerCase();
  return snapshot.nodes.filter((node) => {
    if (
      filters.networkId !== "all" &&
      !node.networks.some(
        (network) => network.network_id === filters.networkId,
      )
    ) {
      return false;
    }
    if (
      filters.role !== "all" &&
      !node.networks.some((network) => network.role === filters.role)
    ) {
      return false;
    }
    if (
      filters.capability !== "all" &&
      !(node.capabilities ?? []).some(
        (capability) => capability === filters.capability,
      )
    ) {
      return false;
    }
    if (filters.health !== "all" && node.health !== filters.health) {
      return false;
    }
    if (!needle) return true;
    return (
      (node.display_name ?? "").toLocaleLowerCase().includes(needle) ||
      node.node_id.toLocaleLowerCase().startsWith(needle)
    );
  });
}

export function filteredTopologySnapshot(
  snapshot: TopologySnapshot,
  filters: TopologyFilters,
): TopologySnapshot {
  const nodes = queryTopology(snapshot, filters);
  const visible = new Set(nodes.map((node) => node.node_id));
  return {
    ...snapshot,
    nodes,
    connections: snapshot.connections.filter(
      (connection) =>
        visible.has(connection.from) && visible.has(connection.to),
    ),
  };
}

/**
 * Preflight is deliberately server-data driven. A selected node is only
 * eligible when the current directory both grants this viewer management
 * rights and advertises the exact lifecycle action for that member.
 */
export function buildBatchMemberPlan(
  snapshot: TopologySnapshot,
  selected: ReadonlySet<string>,
  networkId: string | null,
  directory: NetworkMemberDirectory | null,
): BatchPlan {
  if (!networkId) {
    return { candidates: [], eligible: [], skipped: [], actionOptions: [] };
  }
  const directoryMembers = new Map(
    (directory?.members ?? []).map((member) => [member.node_id, member]),
  );
  const selectedNodes = snapshot.nodes.filter(
    (node) =>
      selected.has(node.node_id) &&
      node.networks.some((network) => network.network_id === networkId),
  );
  const baseCandidates = selectedNodes.map((node) => {
    const member = directoryMembers.get(node.node_id) ?? null;
    let skipReason: BatchSkipReason | undefined;
    if (!directory?.can_manage_members || directory.viewer_role === "member") {
      skipReason = "not_manageable";
    } else if (!member) {
      skipReason = "member_missing";
    }
    return {
      nodeId: node.node_id,
      name: node.display_name?.trim() || node.node_id,
      member,
      skipReason,
    };
  });
  const actionOptions =
    directory?.can_manage_members && directory.viewer_role !== "member"
      ? BATCH_MEMBER_ACTIONS.map((action) => {
          let eligibleCount = 0;
          for (const candidate of baseCandidates) {
            if (candidate.member?.available_actions?.includes(action)) {
              eligibleCount += 1;
            }
          }
          return {
            action,
            eligibleCount,
            totalCount: baseCandidates.length,
          };
        }).filter((option) => option.eligibleCount > 0)
      : [];
  const candidates = baseCandidates.map((candidate) => ({
    ...candidate,
    eligible: candidate.skipReason === undefined,
  }));
  return {
    candidates,
    eligible: candidates.filter((candidate) => candidate.eligible),
    skipped: candidates.filter((candidate) => !candidate.eligible),
    actionOptions,
  };
}

/** Narrow the plan to the action selected in the batch confirmation dialog. */
export function planForBatchAction(
  plan: BatchPlan,
  action: BatchMemberAction,
): BatchPlan {
  const candidates = plan.candidates.map((candidate) => {
    const skipReason =
      candidate.skipReason ??
      (candidate.member?.available_actions?.includes(action)
        ? undefined
        : "action_unavailable");
    return {
      ...candidate,
      skipReason,
      eligible: skipReason === undefined,
    };
  });
  return {
    ...plan,
    candidates,
    eligible: candidates.filter((candidate) => candidate.eligible),
    skipped: candidates.filter((candidate) => !candidate.eligible),
  };
}
