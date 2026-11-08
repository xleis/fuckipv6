import type {
  ActivityCategory,
  ActivityEntry,
  ActivitySeverity,
  AuditOutcome,
  NodeSnapshot,
} from "../../contracts/typescript/node-contract-v1";

export type ActivityFilters = {
  networkId: string;
  objectKind: string;
  objectId: string;
  actorRole: string;
  category: "" | ActivityCategory;
  severity: "" | ActivitySeverity;
  outcome: "" | AuditOutcome;
  fromUnixMs?: number;
  toUnixMs?: number;
};

export type ActivityFilterOptions = {
  networks: string[];
  objectKinds: string[];
  objectIds: string[];
  actorRoles: string[];
};

export type ActivityObjectTarget = {
  pageId: string;
  networkName?: string;
};

/** Presentation-only state supplied by the live snapshot transport. */
export type ActivityPresentationSignals = {
  refreshing?: boolean;
  stale?: boolean;
  offline?: boolean;
  error?: string | null;
  permissionDenied?: boolean;
};

export const EMPTY_ACTIVITY_FILTERS: ActivityFilters = {
  networkId: "",
  objectKind: "",
  objectId: "",
  actorRole: "",
  category: "",
  severity: "",
  outcome: "",
};

/** Compatibility projection for snapshots produced before category/severity. */
export function activityCategory(entry: ActivityEntry): ActivityCategory {
  if (entry.category) return entry.category;
  if (entry.outcome === "denied") return "security_event";
  if (entry.outcome === "failed") return "operation_failure";
  return "user_activity";
}

/** Compatibility projection for snapshots produced before category/severity. */
export function activitySeverity(entry: ActivityEntry): ActivitySeverity {
  if (entry.severity) return entry.severity;
  return entry.outcome === "succeeded" ? "info" : "warning";
}

/**
 * Activity is a projection of authoritative audit events. Replayed events are
 * collapsed by stable audit ID and ordered once for every product surface.
 */
export function orderedActivityEntries(
  entries: readonly ActivityEntry[] | undefined,
): ActivityEntry[] {
  const byId = new Map<string, ActivityEntry>();
  for (const entry of entries ?? []) byId.set(entry.audit_id, entry);
  return [...byId.values()].sort(
    (left, right) =>
      right.occurred_at_unix_ms - left.occurred_at_unix_ms ||
      right.audit_id.localeCompare(left.audit_id),
  );
}

export function activityFilterOptions(
  entries: readonly ActivityEntry[] | undefined,
): ActivityFilterOptions {
  const uniqueSorted = (values: Iterable<string>) =>
    [...new Set(values)].filter(Boolean).sort((left, right) => left.localeCompare(right));
  const ordered = orderedActivityEntries(entries);
  return {
    networks: uniqueSorted(
      ordered.flatMap((entry) => (entry.network_id ? [entry.network_id] : [])),
    ),
    objectKinds: uniqueSorted(
      ordered.flatMap((entry) => (entry.object_kind ? [entry.object_kind] : [])),
    ),
    objectIds: uniqueSorted(
      ordered.flatMap((entry) => (entry.object_id ? [entry.object_id] : [])),
    ),
    actorRoles: uniqueSorted(
      ordered.flatMap((entry) => (entry.actor_role ? [entry.actor_role] : [])),
    ),
  };
}

/** Time bounds are inclusive, matching the GPUI projection. */
export function filterActivityEntries(
  entries: readonly ActivityEntry[] | undefined,
  filters: ActivityFilters,
): ActivityEntry[] {
  return orderedActivityEntries(entries).filter((entry) => {
    if (filters.networkId && entry.network_id !== filters.networkId) return false;
    if (filters.objectKind && entry.object_kind !== filters.objectKind) return false;
    if (filters.objectId && entry.object_id !== filters.objectId) return false;
    if (filters.actorRole && entry.actor_role !== filters.actorRole) return false;
    if (filters.category && activityCategory(entry) !== filters.category) return false;
    if (filters.severity && activitySeverity(entry) !== filters.severity) return false;
    if (filters.outcome && entry.outcome !== filters.outcome) return false;
    if (filters.fromUnixMs !== undefined && entry.occurred_at_unix_ms < filters.fromUnixMs) {
      return false;
    }
    if (filters.toUnixMs !== undefined && entry.occurred_at_unix_ms > filters.toUnixMs) {
      return false;
    }
    return true;
  });
}

/** Converts a datetime-local value to an inclusive Unix millisecond bound. */
export function activityTimeBound(value: string): number | undefined {
  if (!value) return undefined;
  const parsed = Date.parse(value);
  return Number.isFinite(parsed) ? parsed : undefined;
}

/**
 * Historical activity never grants access that the current snapshot no longer
 * provides. Both the current view permission and object existence are checked.
 */
export function activityObjectTarget(
  snapshot: NodeSnapshot,
  entry: ActivityEntry,
): ActivityObjectTarget | null {
  if (!snapshot.permissions?.permissions.includes("view_state")) return null;
  const objectId = entry.object_id;
  if (!objectId) return null;

  const networkName = (networkId: string) =>
    snapshot.memberships?.find((membership) => membership.network_id === networkId)
      ?.network_name ??
    snapshot.network_profiles?.find((profile) => profile.network_id === networkId)?.name ??
    snapshot.networks.find((network) => network.name === networkId)?.name;

  if (entry.object_kind === "network") {
    const name = networkName(objectId);
    return name ? { pageId: "topology", networkName: name } : null;
  }

  if (entry.object_kind === "member" || entry.object_kind === "topology_node") {
    const node = snapshot.topology?.nodes.find((candidate) => candidate.node_id === objectId);
    if (!node) return null;
    if (
      entry.network_id &&
      !node.networks.some((membership) => membership.network_id === entry.network_id)
    ) {
      return null;
    }
    const nodeNetworkId = entry.network_id ?? node.networks[0]?.network_id;
    const name = nodeNetworkId ? networkName(nodeNetworkId) : undefined;
    return { pageId: "topology", networkName: name };
  }

  if (entry.object_kind === "node") {
    return snapshot.identity?.node_id === objectId ? { pageId: "identity" } : null;
  }

  return null;
}
