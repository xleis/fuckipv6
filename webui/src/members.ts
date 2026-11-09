import type {
  CapabilityKind,
  ExitAvailabilitySource,
  ExitAvailabilityState,
  ExitContributionAvailability,
  MembershipRole,
  NetworkMember,
  NetworkMemberGroup,
  NetworkMemberOnlineState,
  NetworkMemberStatus,
  NodeNetworkMembershipSnapshot,
} from "../../contracts/typescript/node-contract-v1";
import { i18n, label } from "./contract/invariants";
import { tr } from "./i18n";

export type MemberHealthFilter =
  | "all"
  | `status:${NetworkMemberStatus}`
  | `presence:${NetworkMemberOnlineState}`;

export type MemberFilters = {
  role: MembershipRole | "all";
  group: NetworkMemberGroup | "all";
  capability: CapabilityKind | "unknown" | "all";
  health: MemberHealthFilter;
};

export const EMPTY_MEMBER_FILTERS: MemberFilters = {
  role: "all",
  group: "all",
  capability: "all",
  health: "all",
};

export const EXIT_AVAILABILITY_MAX_AGE_MS = 90_000;

const EXIT_AVAILABILITY_REASON_CODES = [
  "exit.availability.available_but_not_declared",
  "exit.availability.cleaning",
  "exit.availability.declared_but_unavailable",
  "exit.availability.draining",
  "exit.availability.forwarder_unhealthy",
  "exit.availability.health_checking",
  "exit.availability.network_not_allowed",
  "exit.availability.not_observed",
  "exit.availability.owner_disabled",
  "exit.availability.stale",
  "exit.availability.waiting_for_network_authorization",
] as const;

export type ExitAvailabilityReasonCode =
  (typeof EXIT_AVAILABILITY_REASON_CODES)[number];
export type ExitAvailabilityFreshness = "fresh" | "stale" | "not_observed";

export type NormalizedExitAvailability = {
  state: ExitAvailabilityState;
  source: ExitAvailabilitySource;
  declaredAvailable: boolean | null;
  observedAtUnixMs: number | null;
  freshness: ExitAvailabilityFreshness;
  reasonCode: ExitAvailabilityReasonCode | null;
};

function isExitAvailabilityState(
  value: unknown,
): value is ExitAvailabilityState {
  return (
    value === "unknown" ||
    value === "available" ||
    value === "degraded" ||
    value === "unavailable"
  );
}

function isExitAvailabilitySource(
  value: unknown,
): value is ExitAvailabilitySource {
  return (
    value === "not_observed" ||
    value === "local_runtime" ||
    value === "coordinator_health" ||
    value === "remote_peer_report"
  );
}

function allowedExitReason(value: unknown): ExitAvailabilityReasonCode | null {
  return typeof value === "string" &&
    (EXIT_AVAILABILITY_REASON_CODES as readonly string[]).includes(value)
    ? (value as ExitAvailabilityReasonCode)
    : null;
}

/** Fail closed: unknown, stale, or contradictory evidence never becomes healthy. */
export function normalizeExitAvailability(
  availability: ExitContributionAvailability | null | undefined,
  nowUnixMs: number,
): NormalizedExitAvailability {
  if (!availability) {
    return {
      state: "unknown",
      source: "not_observed",
      declaredAvailable: null,
      observedAtUnixMs: null,
      freshness: "not_observed",
      reasonCode: "exit.availability.not_observed",
    };
  }

  const source = isExitAvailabilitySource(availability.source)
    ? availability.source
    : "not_observed";
  const observedAtUnixMs =
    typeof availability.observed_at_unix_ms === "number" &&
    Number.isFinite(availability.observed_at_unix_ms)
      ? availability.observed_at_unix_ms
      : null;
  const declaredAvailable =
    typeof availability.declared_available === "boolean"
      ? availability.declared_available
      : null;
  const suppliedReason = allowedExitReason(availability.reason_code);

  if (observedAtUnixMs == null) {
    const notObserved = source === "not_observed" && availability.state === "unknown";
    return {
      state: "unknown",
      source,
      declaredAvailable,
      observedAtUnixMs: null,
      freshness: notObserved ? "not_observed" : "stale",
      reasonCode:
        suppliedReason ??
        (notObserved
          ? "exit.availability.not_observed"
          : "exit.availability.stale"),
    };
  }

  if (nowUnixMs > observedAtUnixMs + EXIT_AVAILABILITY_MAX_AGE_MS) {
    return {
      state: "unknown",
      source,
      declaredAvailable,
      observedAtUnixMs,
      freshness: "stale",
      reasonCode: "exit.availability.stale",
    };
  }

  const reportedState = isExitAvailabilityState(availability.state)
    ? availability.state
    : "unknown";
  const contradictoryAvailability =
    reportedState === "available" && declaredAvailable === false;
  return {
    state: contradictoryAvailability ? "degraded" : reportedState,
    source,
    declaredAvailable,
    observedAtUnixMs,
    freshness: "fresh",
    reasonCode: contradictoryAvailability
      ? "exit.availability.available_but_not_declared"
      : suppliedReason,
  };
}

export function memberExitStateClass(
  availability: ExitContributionAvailability | null | undefined,
  nowUnixMs: number,
): "good" | "warn" | "bad" | "neutral" {
  switch (normalizeExitAvailability(availability, nowUnixMs).state) {
    case "available":
      return "good";
    case "degraded":
      return "warn";
    case "unavailable":
      return "bad";
    default:
      return "neutral";
  }
}

export function memberExitStateText(
  locale: Locale,
  availability: ExitContributionAvailability | null | undefined,
  nowUnixMs: number,
): string {
  return label(
    locale,
    `member_directory.exit.state.${normalizeExitAvailability(availability, nowUnixMs).state}`,
  );
}

export function memberExitSourceText(
  locale: Locale,
  availability: ExitContributionAvailability | null | undefined,
  nowUnixMs: number,
): string {
  return label(
    locale,
    `member_directory.exit.source.${normalizeExitAvailability(availability, nowUnixMs).source}`,
  );
}

export function memberExitDeclaredText(
  locale: Locale,
  availability: ExitContributionAvailability | null | undefined,
  nowUnixMs: number,
): string {
  const declared = normalizeExitAvailability(availability, nowUnixMs).declaredAvailable;
  if (declared == null) return label(locale, "member_directory.exit.declared.unknown");
  return label(
    locale,
    declared
      ? "member_directory.exit.declared.available"
      : "member_directory.exit.declared.unavailable",
  );
}

export function memberExitFreshnessText(
  locale: Locale,
  availability: ExitContributionAvailability | null | undefined,
  nowUnixMs: number,
): string {
  const normalized = normalizeExitAvailability(availability, nowUnixMs);
  if (normalized.freshness !== "fresh") {
    return label(
      locale,
      `member_directory.exit.freshness.${normalized.freshness}`,
    );
  }
  return memberFreshnessText(locale, normalized.observedAtUnixMs, nowUnixMs);
}

export function memberExitReasonText(
  locale: Locale,
  availability: ExitContributionAvailability | null | undefined,
  nowUnixMs: number,
): string {
  const reason = normalizeExitAvailability(availability, nowUnixMs).reasonCode;
  if (!reason) return label(locale, "common.unknown");
  return label(
    locale,
    `member_directory.exit.reason.${reason.slice("exit.availability.".length)}`,
  );
}

/** Stable presentation order; never depend on response or refresh order. */
export function sortMembers(members: NetworkMember[]): NetworkMember[] {
  return [...members].sort((left, right) => {
    const byNodeId = left.node_id.localeCompare(right.node_id);
    if (byNodeId !== 0) return byNodeId;
    const byStatus = left.status.localeCompare(right.status);
    return byStatus !== 0
      ? byStatus
      : left.presence.state.localeCompare(right.presence.state);
  });
}

function matchesHealth(member: NetworkMember, health: MemberHealthFilter): boolean {
  if (health === "all") return true;
  if (health.startsWith("status:")) {
    return member.status === health.slice("status:".length);
  }
  return member.presence.state === health.slice("presence:".length);
}

/** The returned rows are the complete source for both rendering and export. */
export function filterMembers(
  members: NetworkMember[],
  filters: MemberFilters,
): NetworkMember[] {
  return sortMembers(
    members.filter((member) => {
      if (filters.role !== "all" && member.role !== filters.role) return false;
      if (filters.group !== "all" && !member.groups.includes(filters.group))
        return false;
      if (filters.capability === "unknown") {
        if (member.capabilities != null) return false;
      } else if (
        filters.capability !== "all" &&
        !(member.capabilities ?? []).includes(filters.capability)
      ) {
        return false;
      }
      return matchesHealth(member, filters.health);
    }),
  );
}

export function retainMemberSelection(
  selectedNodeId: string | null,
  members: NetworkMember[],
): string | null {
  if (!selectedNodeId) return null;
  return members.some((member) => member.node_id === selectedNodeId)
    ? selectedNodeId
    : null;
}

export function resolveNetworkMembershipId(
  memberships: NodeNetworkMembershipSnapshot[] | null | undefined,
  networkName: string,
): string | null {
  return (
    memberships?.find((membership) => membership.network_name === networkName)
      ?.network_id ?? null
  );
}

export function memberDisplayName(member: NetworkMember): string {
  return member.display_name ?? member.node_id;
}

export function memberRoleText(locale: Locale, member: NetworkMember): string {
  return label(locale, `join_request.role.${member.role}`);
}

export function memberGroupsText(locale: Locale, member: NetworkMember): string {
  if (member.groups.length === 0) return label(locale, "common.none");
  return member.groups
    .map((group) => label(locale, `member_directory.group.${group}`))
    .join(label(locale, "common.list_separator"));
}

export function memberCapabilitiesText(
  locale: Locale,
  member: NetworkMember,
): string {
  if (member.capabilities == null)
    return label(locale, "member_directory.capabilities_unknown");
  if (member.capabilities.length === 0) return label(locale, "common.none");
  return member.capabilities
    .map((capability) => label(locale, `capability.${capability}`))
    .join(label(locale, "common.list_separator"));
}

export function memberFingerprintText(
  locale: Locale,
  member: NetworkMember,
): string {
  return (
    member.identity_fingerprint ?? label(locale, "member_directory.identity_unknown")
  );
}

export function memberJoinedAtText(
  locale: Locale,
  member: NetworkMember,
): string {
  return member.joined_at_unix_ms == null
    ? label(locale, "member_directory.joined_at_unknown")
    : i18n().formatTimestamp(locale, member.joined_at_unix_ms);
}

export function memberStatusText(locale: Locale, member: NetworkMember): string {
  return label(locale, `member_directory.status.${member.status}`);
}

export function memberPresenceText(
  locale: Locale,
  member: NetworkMember,
): string {
  return label(locale, `member_directory.presence.${member.presence.state}`);
}

export function memberPresenceSourceText(
  locale: Locale,
  member: NetworkMember,
): string {
  return label(locale, `member_directory.source.${member.presence.source}`);
}

export function memberFreshnessText(
  locale: Locale,
  observedAtUnixMs: number | null | undefined,
  nowUnixMs: number,
): string {
  if (observedAtUnixMs == null) return "";
  const elapsedSeconds = Math.max(
    0,
    Math.floor((nowUnixMs - observedAtUnixMs) / 1000),
  );
  if (elapsedSeconds < 60)
    return label(locale, "member_directory.freshness.just_now");
  const minutes = Math.floor(elapsedSeconds / 60);
  if (minutes < 60) {
    return tr(locale, "member_directory.freshness.minutes", {
      count: i18n().formatInteger(locale, minutes),
    });
  }
  const hours = Math.floor(minutes / 60);
  if (hours < 24) {
    return tr(locale, "member_directory.freshness.hours", {
      count: i18n().formatInteger(locale, hours),
    });
  }
  return tr(locale, "member_directory.freshness.days", {
    count: i18n().formatInteger(locale, Math.floor(hours / 24)),
  });
}

function tsvCell(value: string): string {
  return value.replace(/[\t\r\n]+/g, " ").trim();
}

/**
 * Serialize only fields already present in the sanitized response. The caller
 * must pass the exact filtered rows rendered by the directory.
 */
export function exportMemberDirectory(
  members: NetworkMember[],
  locale: Locale,
  nowUnixMs: number = Date.now(),
): string {
  const columns = [
    "member_directory.display_name",
    "member_directory.node_id",
    "member_directory.fingerprint",
    "member_directory.role",
    "member_directory.groups",
    "member_directory.capabilities",
    "member_directory.status",
    "member_directory.presence",
    "member_directory.presence_source",
    "member_directory.observed_at",
    "member_directory.freshness",
    "member_directory.joined_at",
    "member_directory.exit.state",
    "member_directory.exit.source",
    "member_directory.exit.declared",
    "member_directory.exit.observed_at",
    "member_directory.exit.freshness",
    "member_directory.exit.reason",
  ].map((key) => tsvCell(label(locale, key)));
  const rows = sortMembers(members).map((member) => [
    tsvCell(memberDisplayName(member)),
    tsvCell(member.node_id),
    tsvCell(memberFingerprintText(locale, member)),
    tsvCell(memberRoleText(locale, member)),
    tsvCell(memberGroupsText(locale, member)),
    tsvCell(memberCapabilitiesText(locale, member)),
    tsvCell(memberStatusText(locale, member)),
    tsvCell(memberPresenceText(locale, member)),
    tsvCell(memberPresenceSourceText(locale, member)),
    member.presence.observed_at_unix_ms == null
      ? ""
      : i18n().formatTimestamp(locale, member.presence.observed_at_unix_ms),
    tsvCell(
      memberFreshnessText(
        locale,
        member.presence.observed_at_unix_ms,
        nowUnixMs,
      ),
    ),
    tsvCell(memberJoinedAtText(locale, member)),
    tsvCell(memberExitStateText(locale, member.exit_availability, nowUnixMs)),
    tsvCell(memberExitSourceText(locale, member.exit_availability, nowUnixMs)),
    tsvCell(memberExitDeclaredText(locale, member.exit_availability, nowUnixMs)),
    member.exit_availability?.observed_at_unix_ms == null
      ? ""
      : i18n().formatTimestamp(
          locale,
          member.exit_availability.observed_at_unix_ms,
        ),
    tsvCell(memberExitFreshnessText(locale, member.exit_availability, nowUnixMs)),
    tsvCell(memberExitReasonText(locale, member.exit_availability, nowUnixMs)),
  ]);
  return [columns, ...rows].map((row) => row.join("\t")).join("\n");
}
