import { Download, ShieldCheck, UserRound } from "lucide-react";
import { useVirtualizer } from "@tanstack/react-virtual";
import {
  useEffect,
  useId,
  useMemo,
  useRef,
  useState,
  type CSSProperties,
} from "react";
import type {
  CapabilityKind,
  MembershipRole,
  NetworkMember,
  NetworkMemberDirectory,
  NetworkMemberGroup,
  NetworkMemberOnlineState,
  NetworkMemberStatus,
} from "../../contracts/typescript/node-contract-v1";
import { i18n, label } from "./contract/invariants";
import {
  EMPTY_MEMBER_FILTERS,
  exportMemberDirectory,
  filterMembers,
  memberCapabilitiesText,
  memberDisplayName,
  memberFingerprintText,
  memberFreshnessText,
  memberGroupsText,
  memberJoinedAtText,
  memberPresenceSourceText,
  memberPresenceText,
  memberRoleText,
  memberStatusText,
  retainMemberSelection,
  type MemberFilters,
  type MemberHealthFilter,
} from "./members";
import { tr } from "./i18n";

const MEMBERSHIP_ROLES: MembershipRole[] = [
  "owner",
  "administrator",
  "member",
];
const MEMBER_GROUPS: NetworkMemberGroup[] = [
  "owner",
  "administrator",
  "member",
];
const CAPABILITIES: CapabilityKind[] = [
  "coordinator",
  "relay",
  "exit",
  "file",
];
const STATUSES: NetworkMemberStatus[] = [
  "pending",
  "approved",
  "active",
  "degraded",
  "suspended",
  "exited",
  "removed",
  "expired",
];
const PRESENCE_STATES: NetworkMemberOnlineState[] = [
  "online",
  "offline",
  "unknown",
];

function memberStatusClass(member: NetworkMember): string {
  if (member.status === "active" || member.status === "approved") return "good";
  if (member.status === "degraded" || member.status === "pending") return "warn";
  if (
    member.status === "suspended" ||
    member.status === "exited" ||
    member.status === "removed" ||
    member.status === "expired"
  ) {
    return "bad";
  }
  return "neutral";
}

function MemberDetail({
  member,
  locale,
  generatedAtUnixMs,
}: {
  member: NetworkMember | null;
  locale: Locale;
  generatedAtUnixMs: number;
}) {
  if (!member) {
    return (
      <section className="member-detail member-detail-empty">
        <UserRound size={22} />
        <strong>{label(locale, "member_directory.detail_title")}</strong>
        <p>{label(locale, "member_directory.detail_hint")}</p>
      </section>
    );
  }
  const observedAt = member.presence.observed_at_unix_ms;
  const freshness = memberFreshnessText(locale, observedAt, generatedAtUnixMs);
  return (
    <section
      className="member-detail"
      aria-live="polite"
      data-testid="member-detail"
    >
      <div className="member-detail-heading">
        <div>
          <span>{label(locale, "member_directory.detail_title")}</span>
          <h4>{memberDisplayName(member)}</h4>
        </div>
        <span
          className={`status-pill ${memberStatusClass(member)}`}
          data-testid="member-detail-presence"
        >
          {memberPresenceText(locale, member)}
        </span>
      </div>
      <dl className="member-detail-grid">
        <div>
          <dt>{label(locale, "member_directory.display_name")}</dt>
          <dd>{member.display_name ?? label(locale, "common.unknown")}</dd>
        </div>
        <div>
          <dt>{label(locale, "member_directory.node_id")}</dt>
          <dd>
            <code>{member.node_id}</code>
          </dd>
        </div>
        <div>
          <dt>{label(locale, "member_directory.fingerprint")}</dt>
          <dd>
            <code>{memberFingerprintText(locale, member)}</code>
          </dd>
        </div>
        <div>
          <dt>{label(locale, "member_directory.role")}</dt>
          <dd>{memberRoleText(locale, member)}</dd>
        </div>
        <div>
          <dt>{label(locale, "member_directory.groups")}</dt>
          <dd>{memberGroupsText(locale, member)}</dd>
        </div>
        <div>
          <dt>{label(locale, "member_directory.capabilities")}</dt>
          <dd>{memberCapabilitiesText(locale, member)}</dd>
        </div>
        <div>
          <dt>{label(locale, "member_directory.status")}</dt>
          <dd>{memberStatusText(locale, member)}</dd>
        </div>
        <div>
          <dt>{label(locale, "member_directory.presence")}</dt>
          <dd>{memberPresenceText(locale, member)}</dd>
        </div>
        <div>
          <dt>{label(locale, "member_directory.presence_source")}</dt>
          <dd>{memberPresenceSourceText(locale, member)}</dd>
        </div>
        <div>
          <dt>{label(locale, "member_directory.observed_at")}</dt>
          <dd>
            {observedAt == null
              ? label(locale, "common.unknown")
              : i18n().formatTimestamp(locale, observedAt)}
          </dd>
        </div>
        <div>
          <dt>{label(locale, "member_directory.freshness")}</dt>
          <dd>{freshness || label(locale, "common.unknown")}</dd>
        </div>
        <div>
          <dt>{label(locale, "member_directory.joined_at")}</dt>
          <dd>{memberJoinedAtText(locale, member)}</dd>
        </div>
      </dl>
    </section>
  );
}

export function MemberDirectoryCard({
  directory,
  locale,
}: {
  directory: NetworkMemberDirectory;
  locale: Locale;
}) {
  const [filters, setFilters] = useState<MemberFilters>({
    ...EMPTY_MEMBER_FILTERS,
  });
  const [selectedNodeId, setSelectedNodeId] = useState<string | null>(null);
  const [exportFeedback, setExportFeedback] = useState("");
  const [exportFailed, setExportFailed] = useState(false);
  const scrollRef = useRef<HTMLDivElement>(null);
  const titleId = useId();
  const filteredMembers = useMemo(
    () => filterMembers(directory.members, filters),
    [directory.members, filters],
  );
  const selectedMember =
    filteredMembers.find((member) => member.node_id === selectedNodeId) ?? null;
  // TanStack Virtual intentionally exposes non-memoizable callbacks. The
  // virtualizer is scoped to this component and never crosses a memo boundary.
  // eslint-disable-next-line react-hooks/incompatible-library
  const virtualizer = useVirtualizer({
    count: filteredMembers.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => 44,
    overscan: 10,
    initialRect: { width: 680, height: 360 },
  });

  useEffect(() => {
    setSelectedNodeId((current) =>
      retainMemberSelection(current, filteredMembers),
    );
  }, [filteredMembers]);

  function updateFilter<Key extends keyof MemberFilters>(
    key: Key,
    value: MemberFilters[Key],
  ): void {
    setFilters((current) => ({ ...current, [key]: value }));
    setExportFeedback("");
    setExportFailed(false);
  }

  async function exportView(): Promise<void> {
    try {
      await navigator.clipboard.writeText(
        exportMemberDirectory(
          filteredMembers,
          locale,
          directory.generated_at_unix_ms,
        ),
      );
      setExportFailed(false);
      setExportFeedback(
        tr(locale, "member_directory.export_succeeded", {
          count: i18n().formatInteger(locale, filteredMembers.length),
        }),
      );
    } catch {
      setExportFeedback(label(locale, "member_directory.export_failed"));
      setExportFailed(true);
    }
  }

  const healthOptions: Array<{ value: MemberHealthFilter; text: string }> = [
    { value: "all", text: label(locale, "member_directory.filter.all") },
    ...STATUSES.map((status) => ({
      value: `status:${status}` as const,
      text: tr(locale, "member_directory.filter.status", {
        status: label(locale, `member_directory.status.${status}`),
      }),
    })),
    ...PRESENCE_STATES.map((presence) => ({
      value: `presence:${presence}` as const,
      text: tr(locale, "member_directory.filter.presence", {
        presence: label(locale, `member_directory.presence.${presence}`),
      }),
    })),
  ];

  return (
    <section className="member-directory" aria-labelledby={titleId}>
      <div className="member-directory-heading">
        <div>
          <h4 id={titleId}>{label(locale, "member_directory.title")}</h4>
          <p>{label(locale, "member_directory.privacy_boundary")}</p>
        </div>
        <div className="member-directory-meta">
          <span>
            {tr(locale, "member_directory.count", {
              count: i18n().formatInteger(locale, filteredMembers.length),
            })}
          </span>
          <span>
            {label(locale, "member_directory.generated_at")}:{" "}
            {i18n().formatTimestamp(locale, directory.generated_at_unix_ms)}
          </span>
          <button
            className="secondary-button"
            type="button"
            onClick={() => void exportView()}
          >
            <Download size={14} />
            {label(locale, "member_directory.export")}
          </button>
        </div>
      </div>

      <div className="member-filter-bar" aria-label={label(locale, "member_directory.title")}>
        <label>
          <span>{label(locale, "member_directory.filter.role")}</span>
          <select
            value={filters.role}
            onChange={(event) =>
              updateFilter("role", event.target.value as MemberFilters["role"])
            }
          >
            <option value="all">
              {label(locale, "member_directory.filter.all")}
            </option>
            {MEMBERSHIP_ROLES.map((role) => (
              <option key={role} value={role}>
                {label(locale, `join_request.role.${role}`)}
              </option>
            ))}
          </select>
        </label>
        <label>
          <span>{label(locale, "member_directory.filter.group")}</span>
          <select
            value={filters.group}
            onChange={(event) =>
              updateFilter("group", event.target.value as MemberFilters["group"])
            }
          >
            <option value="all">
              {label(locale, "member_directory.filter.all")}
            </option>
            {MEMBER_GROUPS.map((group) => (
              <option key={group} value={group}>
                {label(locale, `member_directory.group.${group}`)}
              </option>
            ))}
          </select>
        </label>
        <label>
          <span>{label(locale, "member_directory.filter.capability")}</span>
          <select
            value={filters.capability}
            onChange={(event) =>
              updateFilter(
                "capability",
                event.target.value as MemberFilters["capability"],
              )
            }
          >
            <option value="all">
              {label(locale, "member_directory.filter.all")}
            </option>
            {CAPABILITIES.map((capability) => (
              <option key={capability} value={capability}>
                {label(locale, `capability.${capability}`)}
              </option>
            ))}
            <option value="unknown">
              {label(locale, "member_directory.capabilities_unknown")}
            </option>
          </select>
        </label>
        <label>
          <span>{label(locale, "member_directory.filter.health")}</span>
          <select
            value={filters.health}
            onChange={(event) =>
              updateFilter("health", event.target.value as MemberHealthFilter)
            }
          >
            {healthOptions.map((option) => (
              <option key={option.value} value={option.value}>
                {option.text}
              </option>
            ))}
          </select>
        </label>
      </div>

      <div className="member-directory-layout">
        <div className="member-list-panel">
          <div
            ref={scrollRef}
            className="member-list-viewport"
            role="listbox"
            aria-label={label(locale, "member_directory.member")}
            data-testid="member-list"
          >
            {filteredMembers.length === 0 ? (
              <div className="empty-state member-list-empty">
                {label(locale, "member_directory.empty")}
              </div>
            ) : (
              <div
                className="member-list-virtual"
                style={{ height: `${virtualizer.getTotalSize()}px` }}
              >
                {virtualizer.getVirtualItems().map((virtualRow) => {
                  const member = filteredMembers[virtualRow.index];
                  const style = {
                    height: `${virtualRow.size}px`,
                    transform: `translateY(${virtualRow.start}px)`,
                  } satisfies CSSProperties;
                  const labelText = `${memberDisplayName(member)} · ${memberStatusText(
                    locale,
                    member,
                  )} · ${memberPresenceText(locale, member)}`;
                  return (
                    <button
                      key={virtualRow.key}
                      id={`member-option-${virtualRow.index}`}
                      className={`member-row ${
                        selectedNodeId === member.node_id ? "selected" : ""
                      }`}
                      type="button"
                      role="option"
                      aria-selected={selectedNodeId === member.node_id}
                      aria-label={labelText}
                      style={style}
                      onClick={() => setSelectedNodeId(member.node_id)}
                    >
                      <span className="member-row-main">
                        <span className="member-avatar" aria-hidden="true">
                          {memberDisplayName(member).slice(0, 1).toUpperCase()}
                        </span>
                        <span className="member-row-name">
                          <strong>{memberDisplayName(member)}</strong>
                          <code>{member.node_id}</code>
                        </span>
                      </span>
                      <span className="member-row-badges">
                        <span className={`status-pill ${memberStatusClass(member)}`}>
                          {memberStatusText(locale, member)}
                        </span>
                        <span
                          className={`presence-dot ${member.presence.state}`}
                          aria-label={memberPresenceText(locale, member)}
                        />
                      </span>
                    </button>
                  );
                })}
              </div>
            )}
          </div>
        </div>
        <MemberDetail
          member={selectedMember}
          locale={locale}
          generatedAtUnixMs={directory.generated_at_unix_ms}
        />
      </div>

      <p className="member-directory-boundary">
        <ShieldCheck size={14} />
        <span>{label(locale, "member_directory.privacy_boundary")}</span>
      </p>
      {exportFeedback ? (
        <p
          className={exportFailed ? "form-error" : "form-success"}
          role={exportFailed ? "alert" : "status"}
        >
          {exportFeedback}
        </p>
      ) : null}
    </section>
  );
}
