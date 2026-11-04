import type {
  CapabilityKind,
  NetworkDiscoveryDocument,
  NetworkJoinMode,
  NetworkVisibility,
  NodeDiscoveryIndexErrorCategory,
  NodeDiscoveryIndexMetrics,
  NodeDiscoveryIndexSnapshot,
  NodeDiscoveryIndexSource,
  NodeSnapshot,
} from "../../contracts/typescript/node-contract-v1";

/** Mirrors crates/client/src/network_discovery.rs. */
export const DISCOVERY_RESULT_STALE_AFTER_MS = 10 * 60 * 1000;
export const DISCOVERY_SCHEMA_VERSION = 3;

export type DiscoveryVisibilityFilter = "public" | "private";

export type DiscoveryFilters = {
  visibility: DiscoveryVisibilityFilter | "all";
  join_mode: NetworkJoinMode | "all";
  language: string;
  region: string;
};

export type DiscoveryResult = {
  networkId: string;
  title: string;
  purpose: string;
  language: string;
  region: string;
  tags: string[];
  capabilities: CapabilityKind[];
  visibility: NetworkVisibility;
  joinMode: NetworkJoinMode;
  verifiedAtUnixMs: number;
  stale: boolean;
};

export type DiscoveryProjectionState =
  | "idle"
  | "pending"
  | "ready"
  | "empty"
  | "no_matches"
  | "degraded"
  | "failed"
  | "unsupported";

export type DiscoveryProjection = {
  state: DiscoveryProjectionState;
  results: DiscoveryResult[];
  validRecordCount: number;
  rejectedRecordCount: number;
  partial: boolean;
  noMoreResults: boolean;
  lastSuccessUnixMs: number | null;
  lastAttemptUnixMs: number | null;
  source: NodeDiscoveryIndexSource;
  metrics: NodeDiscoveryIndexMetrics;
  errorCategory: NodeDiscoveryIndexErrorCategory | null;
};

export type DiscoveryFacets = {
  languages: string[];
  regions: string[];
};

type VerifiedRecord = {
  document: NetworkDiscoveryDocument & {
    language: string;
    region: string;
    tags: string[];
  };
  acceptedAtUnixMs: number;
};

export const EMPTY_DISCOVERY_FILTERS: DiscoveryFilters = {
  visibility: "all",
  join_mode: "all",
  language: "all",
  region: "all",
};

function validDocument(
  document: NetworkDiscoveryDocument,
): document is NetworkDiscoveryDocument & {
  language: string;
  region: string;
  tags: string[];
} {
  return (
    document.schema_version === DISCOVERY_SCHEMA_VERSION &&
    typeof document.language === "string" &&
    document.language.length > 0 &&
    typeof document.region === "string" &&
    document.region.length > 0 &&
    Array.isArray(document.tags)
  );
}

function verifiedRecords(
  index: NodeDiscoveryIndexSnapshot | undefined,
  nowUnixMs: number,
): VerifiedRecord[] {
  if (!index) return [];
  return index.records.flatMap((entry) => {
    const document = entry.record.document;
    if (
      !entry.discoverable ||
      entry.invalid_reason != null ||
      document.revoked_at_unix_ms != null ||
      !validDocument(document) ||
      document.issued_at_unix_ms > nowUnixMs ||
      document.expires_at_unix_ms <= nowUnixMs
    ) {
      return [];
    }
    return [{ document, acceptedAtUnixMs: entry.accepted_at_unix_ms }];
  });
}

function lower(value: string | undefined): string {
  return (value ?? "").toLowerCase();
}

function joinModeSearchTerms(joinMode: NetworkJoinMode): string {
  switch (joinMode) {
    case "open":
      return "open";
    case "approval_required":
      return "approval_required approval request";
    case "invite_only":
      return "invite_only invite invitation";
  }
}

function matchRank(
  document: NetworkDiscoveryDocument,
  query: string,
): number | null {
  if (!query) return 3;
  const title = lower(document.name);
  if (title === query) return 0;
  if (title.startsWith(query)) return 1;
  if (
    title
      .split(/[^a-z0-9]+/)
      .filter(Boolean)
      .some((word) => word.startsWith(query))
  ) {
    return 2;
  }
  const haystack = [
    document.name,
    document.purpose,
    ...(document.tags ?? []),
    ...document.capability_summary.map((capability) => capability),
    joinModeSearchTerms(document.join_mode),
    document.language,
    document.region,
  ]
    .join(" ")
    .toLowerCase();
  return haystack.includes(query) ? 3 : null;
}

function matchesFilters(
  document: NetworkDiscoveryDocument,
  filters: DiscoveryFilters,
): boolean {
  if (filters.visibility !== "all") {
    const isPublic = document.visibility === "public";
    if ((filters.visibility === "public") !== isPublic) return false;
  }
  if (filters.join_mode !== "all" && document.join_mode !== filters.join_mode) {
    return false;
  }
  if (
    filters.language !== "all" &&
    !lower(document.language).includes(lower(filters.language).trim())
  ) {
    return false;
  }
  if (
    filters.region !== "all" &&
    !lower(document.region).includes(lower(filters.region).trim())
  ) {
    return false;
  }
  return true;
}

export function projectDiscovery(
  snapshot: NodeSnapshot,
  query: string,
  filters: DiscoveryFilters = EMPTY_DISCOVERY_FILTERS,
  nowUnixMs: number = Date.now(),
): DiscoveryProjection {
  const index = snapshot.discovery_index;
  const stateValue = index?.state ?? "idle";
  const valid = verifiedRecords(index, nowUnixMs);
  const normalizedQuery = query.trim().toLowerCase();
  const ranked = valid
    .filter(({ document }) => matchesFilters(document, filters))
    .flatMap((record) => {
      const rank = matchRank(record.document, normalizedQuery);
      return rank === null ? [] : [{ rank, record }];
    })
    .sort(
      (left, right) =>
        left.rank - right.rank ||
        left.record.document.name.localeCompare(right.record.document.name) ||
        left.record.document.network_id.localeCompare(
          right.record.document.network_id,
        ),
    );

  const results = ranked.map(({ record }) => ({
    networkId: record.document.network_id,
    title: record.document.name,
    purpose: record.document.purpose,
    language: record.document.language,
    region: record.document.region,
    tags: [...record.document.tags],
    capabilities: [...record.document.capability_summary],
    visibility: record.document.visibility,
    joinMode: record.document.join_mode,
    verifiedAtUnixMs: record.acceptedAtUnixMs,
    stale:
      stateValue === "degraded" ||
      nowUnixMs - record.acceptedAtUnixMs > DISCOVERY_RESULT_STALE_AFTER_MS,
  }));

  const rejectedRecordCount = index?.rejected_record_count ?? 0;
  const degraded = stateValue === "degraded";
  const partial = degraded || rejectedRecordCount > 0;
  let state: DiscoveryProjectionState;
  switch (stateValue) {
    case "idle":
      state = "idle";
      break;
    case "pending":
      state = "pending";
      break;
    case "failed":
      state = "failed";
      break;
    case "unsupported":
      state = "unsupported";
      break;
    case "degraded":
      state = "degraded";
      break;
    case "ready":
      if (partial) state = "degraded";
      else if (valid.length === 0) state = "empty";
      else if (results.length === 0) state = "no_matches";
      else state = "ready";
      break;
  }

  return {
    state,
    results,
    validRecordCount: valid.length,
    rejectedRecordCount,
    partial,
    noMoreResults: state === "ready" && results.length > 0,
    lastSuccessUnixMs: index?.last_success_unix_ms ?? null,
    lastAttemptUnixMs: index?.last_attempt_unix_ms ?? null,
    source: index?.source ?? "none",
    metrics: index?.metrics ?? {
      request_attempts: 0,
      requests_succeeded: 0,
      requests_failed: 0,
      retries_scheduled: 0,
      cancellations: 0,
      records_accepted: 0,
      records_rejected: 0,
    },
    errorCategory: index?.last_error?.category ?? null,
  };
}

export function discoveryFacets(
  snapshot: NodeSnapshot,
  nowUnixMs: number = Date.now(),
): DiscoveryFacets {
  const languages = new Set<string>();
  const regions = new Set<string>();
  for (const { document } of verifiedRecords(
    snapshot.discovery_index,
    nowUnixMs,
  )) {
    languages.add(document.language);
    regions.add(document.region);
  }
  return {
    languages: [...languages].sort((left, right) => left.localeCompare(right)),
    regions: [...regions].sort((left, right) => left.localeCompare(right)),
  };
}

export function discoveryHasStaleResults(
  projection: DiscoveryProjection,
): boolean {
  return projection.results.some((result) => result.stale);
}
