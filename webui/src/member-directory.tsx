import {
  AlertTriangle,
  Check,
  Download,
  ShieldCheck,
  UserRound,
} from "lucide-react";
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
  MemberLifecycleAction,
  MemberLifecycleChange,
  MemberRemovalPreview,
  MembershipRole,
  NetworkMember,
  NetworkMemberDirectory,
  NetworkMemberGroup,
  NetworkMemberOnlineState,
  NetworkMemberStatus,
  NodeCommandOutcome,
  NodeOperationSnapshot,
  NodeUiAction,
} from "../../contracts/typescript/node-contract-v1";
import {
  i18n,
  label,
  outcomeSummary,
  validateActionValues,
  type ActionValues,
} from "./contract/invariants";
import {
  EMPTY_MEMBER_FILTERS,
  exportMemberDirectory,
  filterMembers,
  memberCapabilitiesText,
  memberDisplayName,
  memberExitDeclaredText,
  memberExitFreshnessText,
  memberExitReasonText,
  memberExitSourceText,
  memberExitStateClass,
  memberExitStateText,
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

export type MemberGovernanceSubmission = {
  outcome?: NodeCommandOutcome;
  operation?: NodeOperationSnapshot;
};

export type MemberGovernanceSubmit = (
  action: NodeUiAction,
  values: ActionValues,
  onOperation: (operation: NodeOperationSnapshot) => void,
) => Promise<MemberGovernanceSubmission>;

type GovernanceDialog =
  | {
      member: NetworkMember;
      action: MemberLifecycleAction;
      preview: null;
    }
  | {
      member: NetworkMember;
      action: "remove";
      preview: MemberRemovalPreview;
    };

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

function operationFailureText(
  operation: NodeOperationSnapshot,
  locale: Locale,
): string {
  const error = operation.error;
  if (!error) return label(locale, "common.operation_failed");
  const message = tr(locale, error.message_key, error.params ?? {});
  return error.recovery
    ? `${message} · ${label(locale, error.recovery.hint_key)}`
    : message;
}

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
  const exitObservedAt = member.exit_availability?.observed_at_unix_ms ?? null;
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
          <dt>{label(locale, "member_directory.lifecycle_state")}</dt>
          <dd>
            {label(
              locale,
              `member_directory.lifecycle_state.${member.lifecycle_state}`,
            )}
          </dd>
        </div>
        <div>
          <dt>{label(locale, "member_directory.revision")}</dt>
          <dd>{i18n().formatInteger(locale, member.revision)}</dd>
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
      <section
        className="member-exit-availability"
        aria-label={label(locale, "member_directory.exit.title")}
        data-testid="member-exit-availability"
      >
        <div className="member-exit-availability-heading">
          <strong>{label(locale, "member_directory.exit.title")}</strong>
          <span
            className={`status-pill ${memberExitStateClass(
              member.exit_availability,
              generatedAtUnixMs,
            )}`}
          >
            {memberExitStateText(
              locale,
              member.exit_availability,
              generatedAtUnixMs,
            )}
          </span>
        </div>
        <dl className="member-detail-grid">
          <div>
            <dt>{label(locale, "member_directory.exit.state")}</dt>
            <dd>
              {memberExitStateText(
                locale,
                member.exit_availability,
                generatedAtUnixMs,
              )}
            </dd>
          </div>
          <div>
            <dt>{label(locale, "member_directory.exit.source")}</dt>
            <dd>
              {memberExitSourceText(
                locale,
                member.exit_availability,
                generatedAtUnixMs,
              )}
            </dd>
          </div>
          <div>
            <dt>{label(locale, "member_directory.exit.declared")}</dt>
            <dd>
              {memberExitDeclaredText(
                locale,
                member.exit_availability,
                generatedAtUnixMs,
              )}
            </dd>
          </div>
          <div>
            <dt>{label(locale, "member_directory.exit.observed_at")}</dt>
            <dd>
              {exitObservedAt == null
                ? label(locale, "common.unknown")
                : i18n().formatTimestamp(locale, exitObservedAt)}
            </dd>
          </div>
          <div>
            <dt>{label(locale, "member_directory.exit.freshness")}</dt>
            <dd>
              {memberExitFreshnessText(
                locale,
                member.exit_availability,
                generatedAtUnixMs,
              )}
            </dd>
          </div>
          <div>
            <dt>{label(locale, "member_directory.exit.reason")}</dt>
            <dd>
              {memberExitReasonText(
                locale,
                member.exit_availability,
                generatedAtUnixMs,
              )}
            </dd>
          </div>
        </dl>
      </section>
    </section>
  );
}

export function MemberDirectoryCard({
  directory,
  locale,
  previewRemovalAction,
  changeLifecycleAction,
  activePendingAction = null,
  onSubmit,
  onLifecycleChanged,
}: {
  directory: NetworkMemberDirectory;
  locale: Locale;
  previewRemovalAction?: NodeUiAction;
  changeLifecycleAction?: NodeUiAction;
  activePendingAction?: string | null;
  onSubmit?: MemberGovernanceSubmit;
  onLifecycleChanged?(change: MemberLifecycleChange): void;
}) {
  const [filters, setFilters] = useState<MemberFilters>({
    ...EMPTY_MEMBER_FILTERS,
  });
  const [selectedNodeId, setSelectedNodeId] = useState<string | null>(null);
  const [exportFeedback, setExportFeedback] = useState("");
  const [exportFailed, setExportFailed] = useState(false);
  const [dialog, setDialog] = useState<GovernanceDialog | null>(null);
  const [reason, setReason] = useState("");
  const [governanceErrors, setGovernanceErrors] = useState<string[]>([]);
  const [governanceFailure, setGovernanceFailure] = useState("");
  const [governanceFeedback, setGovernanceFeedback] = useState("");
  const [governing, setGoverning] = useState(false);
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

  const governanceActions = selectedMember?.available_actions ?? [];
  const canGovern =
    Boolean(onSubmit) &&
    directory.can_manage_members &&
    Boolean(changeLifecycleAction) &&
    governanceActions.length > 0;
  const governanceBusy =
    governing ||
    activePendingAction === "preview_member_removal" ||
    activePendingAction === "change_member_lifecycle";

  function resetGovernanceMessages(): void {
    setGovernanceErrors([]);
    setGovernanceFailure("");
  }

  async function beginLifecycle(
    member: NetworkMember,
    lifecycleAction: MemberLifecycleAction,
  ): Promise<void> {
    if (!onSubmit || governanceBusy) return;
    resetGovernanceMessages();
    setGovernanceFeedback("");
    setReason("");
    if (lifecycleAction !== "remove") {
      setDialog({ member, action: lifecycleAction, preview: null });
      return;
    }
    if (!previewRemovalAction) {
      setGovernanceFailure(
        label(locale, "member_directory.governance.unavailable"),
      );
      return;
    }
    setGoverning(true);
    try {
      const submission = await onSubmit(
        previewRemovalAction,
        {
          network_id: directory.network_id,
          target_node_id: member.node_id,
        },
        () => {},
      );
      if (submission.outcome?.status === "member_removal_preview") {
        setDialog({
          member,
          action: "remove",
          preview: submission.outcome.data,
        });
      } else if (submission.outcome?.status === "rejected") {
        setGovernanceFailure(outcomeSummary(submission.outcome, locale));
      } else if (submission.operation) {
        setGovernanceFailure(
          operationFailureText(submission.operation, locale),
        );
      } else {
        setGovernanceFailure(label(locale, "common.operation_failed"));
      }
    } catch (error) {
      setGovernanceFailure(
        error instanceof Error ? error.message : String(error),
      );
    } finally {
      setGoverning(false);
    }
  }

  async function submitLifecycle(): Promise<void> {
    if (!dialog || !onSubmit || !changeLifecycleAction || governing) return;
    const member = dialog.member;
    const expectedRevision = (
      dialog.preview?.revision ?? member.revision
    ).toString();
    const values: ActionValues = {
      network_id: directory.network_id,
      target_node_id: member.node_id,
      action: dialog.action,
      role: member.role,
      expected_revision: expectedRevision,
      ...(reason.trim() ? { reason: reason.trim() } : {}),
    };
    const validation = validateActionValues(
      changeLifecycleAction,
      values,
      locale,
    );
    if (validation.length > 0) {
      setGovernanceErrors(validation);
      return;
    }
    setGoverning(true);
    setGovernanceErrors([]);
    setGovernanceFailure("");
    try {
      const submission = await onSubmit(changeLifecycleAction, values, () => {});
      if (submission.outcome?.status === "member_lifecycle_changed") {
        setGovernanceFeedback(
          tr(locale, "member_directory.governance.changed", {
            action: label(
              locale,
              `choice.member_lifecycle.${submission.outcome.data.action}`,
            ),
            member: memberDisplayName(member),
            revision: i18n().formatInteger(
              locale,
              submission.outcome.data.revision,
            ),
          }),
        );
        setDialog(null);
        setReason("");
        onLifecycleChanged?.(submission.outcome.data);
      } else if (submission.outcome?.status === "rejected") {
        setGovernanceFailure(outcomeSummary(submission.outcome, locale));
      } else if (submission.operation) {
        setGovernanceFailure(
          operationFailureText(submission.operation, locale),
        );
      } else {
        setGovernanceFailure(label(locale, "common.operation_failed"));
      }
    } catch (error) {
      setGovernanceFailure(
        error instanceof Error ? error.message : String(error),
      );
    } finally {
      setGoverning(false);
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
                  const exitState = memberExitStateText(
                    locale,
                    member.exit_availability,
                    directory.generated_at_unix_ms,
                  );
                  const labelText = `${memberDisplayName(member)} · ${memberStatusText(
                    locale,
                    member,
                  )} · ${memberPresenceText(locale, member)} · ${label(
                    locale,
                    "member_directory.exit.state",
                  )}: ${exitState}`;
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
                        <span
                          className={`status-pill ${memberExitStateClass(
                            member.exit_availability,
                            directory.generated_at_unix_ms,
                          )}`}
                          data-testid="member-exit-badge"
                        >
                          {exitState}
                        </span>
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

      {canGovern ? (
        <section
          className="member-governance"
          aria-label={label(locale, "member_directory.governance.title")}
          data-testid="member-governance"
        >
          <div className="member-governance-heading">
            <ShieldCheck size={16} />
            <div>
              <strong>
                {label(locale, "member_directory.governance.title")}
              </strong>
              <p>{label(locale, "member_directory.governance.hint")}</p>
            </div>
          </div>
          <div className="member-governance-actions">
            {governanceActions.map((lifecycleAction) => (
              <button
                key={lifecycleAction}
                type="button"
                className="secondary-button"
                disabled={governanceBusy}
                data-testid={`member-governance-action-${lifecycleAction}`}
                onClick={() =>
                  void beginLifecycle(selectedMember!, lifecycleAction)
                }
              >
                {label(locale, `choice.member_lifecycle.${lifecycleAction}`)}
              </button>
            ))}
          </div>
          {governanceFeedback ? (
            <p className="form-success" role="status" data-testid="member-governance-feedback">
              <Check size={15} />
              <span>{governanceFeedback}</span>
            </p>
          ) : null}
          {!dialog && governanceFailure ? (
            <p className="form-error" role="alert">
              {governanceFailure}
            </p>
          ) : null}
        </section>
      ) : null}
      {dialog ? (
        <section
          className="member-governance-dialog"
          role="dialog"
          aria-modal="true"
          aria-label={label(
            locale,
            dialog.action === "remove"
              ? "member_directory.governance.remove_title"
              : "member_directory.governance.confirm_title",
          )}
          data-testid="member-governance-dialog"
        >
          <h4>
            {label(
              locale,
              dialog.action === "remove"
                ? "member_directory.governance.remove_title"
                : "member_directory.governance.confirm_title",
            )}
            {" · "}
            {label(locale, `choice.member_lifecycle.${dialog.action}`)}
          </h4>
          <p className="member-governance-target">
            <strong>{memberDisplayName(dialog.member)}</strong>{" "}
            <code>{dialog.member.node_id}</code>
          </p>
          {dialog.preview ? (
            <>
              <p className="member-governance-impact-title">
                <AlertTriangle size={15} />
                <span>{label(locale, "member_removal.impact.title")}</span>
              </p>
              <ul className="member-governance-impact" data-testid="member-removal-impact">
                {dialog.preview.impact_keys.map((key) => (
                  <li key={key}>{label(locale, key)}</li>
                ))}
              </ul>
              {!dialog.preview.can_remove ? (
                <p className="form-error" role="alert">
                  {label(locale, "member_directory.governance.unavailable")}
                </p>
              ) : null}
            </>
          ) : (
            <p className="member-governance-consequence">
              {label(
                locale,
                "confirm.change_member_lifecycle.consequence",
              )}
            </p>
          )}
          <label className="form-field" htmlFor={`${titleId}-reason`}>
            <span>{label(locale, "field.lifecycle_reason")}</span>
            <input
              id={`${titleId}-reason`}
              type="text"
              value={reason}
              maxLength={512}
              autoComplete="off"
              onChange={(event) => setReason(event.target.value)}
            />
          </label>
          {governanceErrors.length > 0 ? (
            <ul className="form-error" role="alert">
              {governanceErrors.map((error) => (
                <li key={error}>{error}</li>
              ))}
            </ul>
          ) : null}
          {governanceFailure ? (
            <p className="form-error" role="alert">
              {governanceFailure}
            </p>
          ) : null}
          <div className="member-governance-dialog-actions">
            <button
              type="button"
              className="secondary-button"
              data-testid="member-governance-cancel"
              onClick={() => {
                setDialog(null);
                setReason("");
                resetGovernanceMessages();
              }}
            >
              {label(locale, "common.cancel")}
            </button>
            <button
              type="button"
              className="primary-button"
              data-testid="member-governance-confirm"
              disabled={
                governing || (dialog.preview ? !dialog.preview.can_remove : false)
              }
              onClick={() => void submitLifecycle()}
            >
              {label(locale, `choice.member_lifecycle.${dialog.action}`)}
            </button>
          </div>
        </section>
      ) : null}
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
