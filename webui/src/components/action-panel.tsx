import * as Dialog from "@radix-ui/react-dialog";
import {
  AlertTriangle,
  Check,
  ChevronRight,
  Copy,
  ShieldAlert,
  X,
} from "lucide-react";
import { useEffect, useId, useRef, useState, type FormEvent } from "react";
import type {
  ExitContributionMode,
  ExitContributionObligation,
  InvitationPreview,
  InvitationState,
  InvitationStatusView,
  IssuedInvitation,
  JoinRequest,
  JoinRequestReviewAction,
  MemberLifecycleChange,
  NetworkMemberDirectory,
  NetworkProfileApplyResult,
  NetworkProfilePreview,
  NetworkProfileProjections,
  NetworkPublicProfileDocument,
  NodeCommandOutcome,
  NodeJoinRequestReviewAck,
  NodeNetworkJoinPreview,
  NodeOperationSnapshot,
  NodeSnapshot,
  NodeUiAction,
  NodeUiField,
  NodeJoinRequestListResult,
} from "../../../contracts/typescript/node-contract-v1";
import {
  blockReasonText,
  executionBlockReason,
  i18n,
  initialActionValues,
  label,
  operationStateIsTerminal,
  outcomeSummary,
  validateActionValues,
  type ActionValues,
} from "../contract/invariants";
import { MemberDirectoryCard } from "../member-directory";
import { tr } from "../i18n";
import { pageStateDefinition, type PageStateKind } from "../state-matrix";

type ActionPanelProps = {
  actions: NodeUiAction[];
  snapshot: NodeSnapshot;
  selectedNetwork: string;
  locale: Locale;
  pendingAction: string | null;
  interactionBlocked: PageStateKind | null;
  requestedAction: {
    actionId: string;
    values?: ActionValues;
  } | null;
  operation: NodeOperationSnapshot | null;
  onRetryOperation?: () => Promise<void>;
  onRequestedActionOpened(): void;
  onCancel(operationId: string): Promise<void>;
  onSubmit(
    action: NodeUiAction,
    values: ActionValues,
    onOperation: (operation: NodeOperationSnapshot) => void,
  ): Promise<ActionSubmission>;
};

export type ActionSubmission = {
  outcome?: NodeCommandOutcome;
  operation?: NodeOperationSnapshot;
};

const numberInputs = new Set(["port", "timeout_ms"]);
const joinRequestReasonMaxLength = 1000;
const selectInputs = new Set([
  "network_mode",
  "network_selection",
  "role",
  "routing_mode",
  "visibility",
  "join_mode",
  "exit_contribution_mode",
  "join_request_review_action",
  "member_lifecycle_action",
]);

function selectOptions(
  field: NodeUiField,
  snapshot: NodeSnapshot,
  locale: Locale,
): Array<{ value: string; text: string }> {
  if (field.input === "visibility") {
    return ["public", "unlisted", "private"].map((value) => ({
      value,
      text: label(locale, `choice.visibility.${value}`),
    }));
  }
  if (field.input === "join_mode") {
    return ["open", "approval_required", "invite_only"].map((value) => ({
      value,
      text: label(locale, `choice.join_mode.${value}`),
    }));
  }
  if (field.input === "exit_contribution_mode") {
    return [
      "disabled",
      "autonomous",
      "required_on_join",
      "approval_required",
    ].map((value) => ({
      value,
      text: label(locale, `choice.exit_contribution_mode.${value}`),
    }));
  }
  if (field.input === "join_request_review_action") {
    return ["approve", "reject", "request_information"].map((value) => ({
      value,
      text: label(locale, `choice.join_request_review.${value}`),
    }));
  }
  if (field.input === "network_mode") {
    return [
      { value: "dual_stack", text: "dual_stack" },
      { value: "ipv4_only", text: "ipv4_only" },
      { value: "ipv6_only", text: "ipv6_only" },
    ];
  }
  if (field.input === "member_lifecycle_action") {
    return [
      "invite",
      "approve",
      "reject",
      "suspend",
      "resume",
      "exit",
      "remove",
      "undo_remove",
    ].map((value) => ({
      value,
      text: label(locale, `choice.member_lifecycle.${value}`),
    }));
  }
  if (field.input === "role") {
    if (field.label_key === "field.membership_role") {
      return ["owner", "administrator", "member"].map((value) => ({
        value,
        text: label(locale, `join_request.role.${value}`),
      }));
    }
    if (field.label_key === "field.invitation_role") {
      return ["member", "administrator"].map((value) => ({
        value,
        text: label(locale, `join_request.role.${value}`),
      }));
    }
    return ["member", "discovery", "relay", "exit_node"].map((value) => ({
      value,
      text: value,
    }));
  }
  if (field.input === "routing_mode") {
    return ["mesh_only", "bypass_mainland", "global_proxy"].map((value) => ({
      value,
      text: value,
    }));
  }
  return [
    { value: "default", text: "default" },
    { value: "all", text: "all" },
    { value: "none", text: "none" },
    ...snapshot.networks.map((network) => ({
      value: network.name,
      text: network.name,
    })),
  ];
}

function FieldControl({
  field,
  locale,
  snapshot,
  value,
  onChange,
}: {
  field: NodeUiField;
  locale: Locale;
  snapshot: NodeSnapshot;
  value: string | boolean;
  onChange(value: string | boolean): void;
}) {
  const id = useId();
  const fieldLabel = label(locale, field.label_key);
  if (field.input === "boolean") {
    return (
      <label className="check-field" htmlFor={id}>
        <input
          id={id}
          type="checkbox"
          checked={Boolean(value)}
          onChange={(event) => onChange(event.target.checked)}
        />
        <span>{fieldLabel}</span>
      </label>
    );
  }
  if (selectInputs.has(field.input)) {
    return (
      <label className="form-field" htmlFor={id}>
        <span>{fieldLabel}</span>
        <select
          id={id}
          value={String(value)}
          onChange={(event) => onChange(event.target.value)}
        >
          {selectOptions(field, snapshot, locale).map((option) => (
            <option key={option.value} value={option.value}>
              {option.text}
            </option>
          ))}
        </select>
      </label>
    );
  }
  if (field.input === "purpose") {
    return (
      <label className="form-field" htmlFor={id}>
        <span>{fieldLabel}</span>
        <textarea
          id={id}
          value={String(value)}
          required={field.required}
          rows={3}
          maxLength={512}
          placeholder={label(locale, "placeholder.network_purpose")}
          onChange={(event) => onChange(event.target.value)}
        />
      </label>
    );
  }
  if (field.input === "join_request_reason") {
    const reason = String(value);
    const helpId = `${id}-help`;
    return (
      <label className="form-field" htmlFor={id}>
        <span>{fieldLabel}</span>
        <input
          id={id}
          type="text"
          value={reason}
          required={field.required}
          maxLength={joinRequestReasonMaxLength}
          aria-describedby={helpId}
          placeholder={label(locale, "placeholder.join_request_reason")}
          autoComplete="off"
          onChange={(event) => onChange(event.target.value)}
        />
        <small
          className="field-help"
          id={helpId}
          data-character-count={[...reason].length}
        >
          {label(locale, "join_requests.reason_input_help")} ·{" "}
          {tr(locale, "join_requests.reason_input_counter", {
            count: [...reason].length,
            max: joinRequestReasonMaxLength,
          })}
        </small>
      </label>
    );
  }
  return (
    <label className="form-field" htmlFor={id}>
      <span>{fieldLabel}</span>
      <input
        id={id}
        type={
          numberInputs.has(field.input)
            ? "number"
            : field.input === "url_or_ticket" ||
                field.input === "discovery_index_url"
              ? "url"
              : "text"
        }
        value={String(value)}
        required={field.required}
        placeholder={
          field.input === "fingerprint"
            ? label(locale, "placeholder.fingerprint")
            : undefined
        }
        min={
          field.input === "port"
            ? 1
            : field.input === "timeout_ms"
              ? 1
              : undefined
        }
        inputMode={numberInputs.has(field.input) ? "numeric" : undefined}
        autoComplete="off"
        onChange={(event) => onChange(event.target.value)}
      />
    </label>
  );
}

function operationFailureSummary(
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

type NetworkProfileOutcome = Extract<
  NodeCommandOutcome,
  { status: "network_profile_preview" | "network_profile_updated" }
>;

function profileList(
  values: string[],
  locale: Locale,
  className = "profile-chip-list",
) {
  if (values.length === 0)
    return <span className="profile-empty-value">{label(locale, "common.none")}</span>;
  return (
    <span className={className}>
      {values.map((value) => (
        <code key={value}>{value}</code>
      ))}
    </span>
  );
}

function ProfileDocumentCard({
  title,
  document,
  locale,
  signatureValid,
  memberCount,
  administratorCount,
  policyRevision,
}: {
  title: string;
  document: NetworkPublicProfileDocument;
  locale: Locale;
  signatureValid?: boolean;
  memberCount?: number;
  administratorCount?: number;
  policyRevision?: number;
}) {
  return (
    <section className="profile-projection-card">
      <div className="profile-projection-heading">
        <span>{title}</span>
        <strong>{document.name}</strong>
      </div>
      <dl className="profile-definition-list">
        <div>
          <dt>{label(locale, "field.purpose")}</dt>
          <dd>{document.purpose}</dd>
        </div>
        <div>
          <dt>{label(locale, "field.icon")}</dt>
          <dd>
            <code>{document.icon}</code>
          </dd>
        </div>
        <div>
          <dt>{label(locale, "field.language")}</dt>
          <dd>
            <code>{document.language}</code>
          </dd>
        </div>
        <div>
          <dt>{label(locale, "field.region")}</dt>
          <dd>
            <code>{document.region}</code>
          </dd>
        </div>
        <div>
          <dt>{label(locale, "field.tags")}</dt>
          <dd>{profileList(document.tags, locale)}</dd>
        </div>
        <div>
          <dt>{label(locale, "field.capability_declarations")}</dt>
          <dd>{profileList(document.capability_declarations, locale)}</dd>
        </div>
        <div>
          <dt>{label(locale, "field.visibility")}</dt>
          <dd>{label(locale, `choice.visibility.${document.visibility}`)}</dd>
        </div>
        <div>
          <dt>{label(locale, "field.join_mode")}</dt>
          <dd>{label(locale, `choice.join_mode.${document.join_mode}`)}</dd>
        </div>
      </dl>
      <div className="profile-signature-meta">
        <span>
          {tr(locale, "profile.revision", {
            revision: i18n().formatInteger(locale, document.revision),
          })}
        </span>
        <span>
          {tr(locale, "profile.schema_version", {
            version: i18n().formatInteger(locale, document.schema_version),
          })}
        </span>
        <span>
          {tr(locale, "profile.expires_at", {
            time: i18n().formatTimestamp(locale, document.expires_at_unix_ms),
          })}
        </span>
        {signatureValid ? <span>{label(locale, "profile.signature_valid")}</span> : null}
        {memberCount !== undefined ? (
          <span>
            {tr(locale, "profile.member_count", {
              count: i18n().formatInteger(locale, memberCount),
            })}
          </span>
        ) : null}
        {administratorCount !== undefined ? (
          <span>
            {tr(locale, "profile.admin_count", {
              count: i18n().formatInteger(locale, administratorCount),
            })}
          </span>
        ) : null}
        {policyRevision !== undefined ? (
          <span>
            {tr(locale, "profile.policy_revision", {
              revision: i18n().formatInteger(locale, policyRevision),
            })}
          </span>
        ) : null}
      </div>
    </section>
  );
}

function ProfileProjectionGrid({
  projections,
  locale,
  signedProfile,
}: {
  projections: NetworkProfileProjections;
  locale: Locale;
  signedProfile: boolean;
}) {
  return (
    <div className="profile-projection-grid">
      <ProfileDocumentCard
        title={label(locale, "profile.public_card")}
        document={projections.public_card.candidate}
        locale={locale}
        signatureValid={signedProfile}
      />
      <ProfileDocumentCard
        title={label(locale, "profile.member_view")}
        document={projections.member_view.profile}
        locale={locale}
        signatureValid={signedProfile}
        memberCount={projections.member_view.member_count}
        administratorCount={projections.member_view.administrator_count}
      />
      <ProfileDocumentCard
        title={label(locale, "profile.admin_view")}
        document={projections.administrator_view.profile}
        locale={locale}
        signatureValid={signedProfile}
        memberCount={projections.administrator_view.member_count}
        administratorCount={projections.administrator_view.administrator_count}
        policyRevision={projections.administrator_view.policy_revision}
      />
      <section className="profile-projection-card">
        <div className="profile-projection-heading">
          <span>{label(locale, "profile.local_secret")}</span>
          <strong>{projections.local_secret.storage}</strong>
        </div>
        <div className="profile-secret-flags">
          <span>
            {label(
              locale,
              projections.local_secret.identity_present
                ? "profile.identity_present"
                : "profile.identity_absent",
            )}
          </span>
          <span>
            {label(
              locale,
              projections.local_secret.secret_material_included
                ? "profile.secret_included"
                : "profile.secret_not_included",
            )}
          </span>
        </div>
        <code className="profile-fingerprint">
          {projections.local_secret.public_key_fingerprint}
        </code>
      </section>
    </div>
  );
}

function ProfileWarnings({
  outcome,
  locale,
}: {
  outcome: NetworkProfileOutcome;
  locale: Locale;
}) {
  const warnings = outcome.data.warnings;
  return (
    <section className="profile-result-section">
      <h4>{label(locale, "profile.warnings")}</h4>
      {warnings.length === 0 ? (
        <p className="profile-muted">{label(locale, "profile.no_warnings")}</p>
      ) : (
        <ul className="profile-warning-list">
          {warnings.map((warning, index) => (
            <li key={`${warning.code}:${warning.capability ?? ""}:${index}`}>
              <AlertTriangle size={14} />
              <div>
                <strong>
                  {label(locale, `warning.network_profile.${warning.code}`)}
                </strong>
                <span>{warning.message}</span>
                {warning.capability ? <code>{warning.capability}</code> : null}
              </div>
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}

function NetworkProfileResultCard({
  outcome,
  locale,
}: {
  outcome: NetworkProfileOutcome;
  locale: Locale;
}) {
  const previewData: NetworkProfilePreview | null =
    outcome.status === "network_profile_preview" ? outcome.data : null;
  const updateData: NetworkProfileApplyResult | null =
    outcome.status === "network_profile_updated" ? outcome.data : null;
  const data = previewData ?? updateData;
  if (!data) return null;
  return (
    <section
      className="network-profile-result"
      data-testid="network-profile-result"
      aria-live="polite"
    >
      <div className="network-profile-result-heading">
        <div>
          <span>
            {label(
              locale,
              previewData ? "profile.preview_title" : "profile.updated_title",
            )}
          </span>
          <strong>{data.network_name}</strong>
        </div>
        <Check size={16} />
      </div>

      {previewData ? (
        <div className="profile-result-status">
          <span>
            {tr(locale, "profile.revision", {
              revision: i18n().formatInteger(
                locale,
                previewData.current_revision,
              ),
            })}
          </span>
          <span>
            {tr(locale, "profile.revision", {
              revision: i18n().formatInteger(
                locale,
                previewData.candidate_revision,
              ),
            })}
          </span>
          <span>
            {label(
              locale,
              previewData.can_apply
                ? "profile.can_apply"
                : "profile.cannot_apply",
            )}
          </span>
        </div>
      ) : updateData ? (
        <div className="profile-result-status">
          <span>
            {tr(locale, "profile.revision", {
              revision: i18n().formatInteger(
                locale,
                updateData.previous_revision,
              ),
            })}
          </span>
          <span>
            {tr(locale, "profile.revision", {
              revision: i18n().formatInteger(locale, updateData.revision),
            })}
          </span>
          <span>
            {tr(locale, "profile.audit", {
              action: updateData.audit_action,
            })}
          </span>
        </div>
      ) : null}

      {previewData?.conflict ? (
        <div className="inline-notice warning">
          <ShieldAlert size={15} />
          <span>
            {tr(locale, "profile.conflict", {
              current: i18n().formatInteger(
                locale,
                previewData.current_revision,
              ),
              expected: i18n().formatInteger(
                locale,
                previewData.expected_revision,
              ),
            })}
          </span>
        </div>
      ) : null}

      <ProfileProjectionGrid
        projections={data.projections}
        locale={locale}
        signedProfile={
          updateData !== null ||
          Boolean(data.projections.public_card.currently_published)
        }
      />

      <section className="profile-withheld">
        <AlertTriangle size={14} />
        <div>
          <strong>{label(locale, "profile.withheld")}</strong>
          {profileList(data.projections.public_card.withheld_categories, locale)}
        </div>
      </section>

      {previewData ? (
        <section className="profile-result-section">
          <h4>{label(locale, "profile.diff")}</h4>
          {previewData.diff.length === 0 ? (
            <p className="profile-muted">{label(locale, "profile.no_changes")}</p>
          ) : (
            <ul className="profile-diff-list">
              {previewData.diff.map((entry) => (
                <li key={entry.path}>
                  <code>{entry.path}</code>
                  <div>
                    <span>{label(locale, "configuration.before")}</span>
                    <strong>{entry.before ?? label(locale, "common.none")}</strong>
                  </div>
                  <div>
                    <span>{label(locale, "configuration.after")}</span>
                    <strong>{entry.after ?? label(locale, "common.none")}</strong>
                  </div>
                </li>
              ))}
            </ul>
          )}
        </section>
      ) : updateData ? (
        <section className="profile-result-section">
          <h4>{label(locale, "configuration.history.changed_paths")}</h4>
          {updateData.changed_paths.length === 0 ? (
            <p className="profile-muted">{label(locale, "profile.no_changes")}</p>
          ) : (
            <ul className="profile-path-list">
              {updateData.changed_paths.map((path) => (
                <li key={path}>
                  <code>{path}</code>
                </li>
              ))}
            </ul>
          )}
        </section>
      ) : null}

      <ProfileWarnings outcome={outcome} locale={locale} />
    </section>
  );
}

function joinPreviewValues(
  preview: NodeNetworkJoinPreview,
  exitContributionAcknowledged: boolean,
): ActionValues {
  return {
    target: "",
    network_id: preview.network_id,
    network_name: preview.network_name,
    preview_digest: preview.preview_digest,
    network_policy_revision: String(preview.network_policy_revision),
    privacy_policy_revision: String(preview.privacy_policy_revision),
    terms_hash: preview.terms_hash,
    start: true,
    ...(exitContributionAcknowledged
      ? { exit_contribution_acknowledged: "true" }
      : {}),
  };
}

function invitationPreviewValues(
  preview: InvitationPreview,
  source: ActionValues,
  exitContributionAcknowledged: boolean,
): ActionValues {
  return {
    network_id: preview.network_id,
    coordinator_url: String(source.coordinator_url ?? ""),
    code: String(source.code ?? ""),
    invitation_id: preview.invitation_id,
    expected_invitation_revision: String(preview.invitation_revision),
    preview_digest: preview.preview_digest,
    terms_hash: preview.terms_hash,
    acknowledged_at_unix_ms: String(Date.now()),
    ...(exitContributionAcknowledged
      ? { exit_contribution_acknowledged: "true" }
      : {}),
  };
}

function invitationStateKey(state: InvitationState): string {
  return `invitations.state.${state}`;
}

function invitationRoleLabel(
  role: InvitationStatusView["invitation"]["role"],
  locale: Locale,
): string {
  return label(locale, `join_request.role.${role}`);
}

function capabilityList(
  capabilities: InvitationStatusView["invitation"]["capabilities"],
  locale: Locale,
): string {
  return joinList(
    (capabilities ?? []).map((capability) =>
      label(locale, `capability.${capability}`),
    ),
    locale,
  );
}

function invitationOutcomeSummary(
  outcome: NodeCommandOutcome,
  locale: Locale,
): string | null {
  switch (outcome.status) {
    case "invitation_created":
      return label(locale, "invitations.created_notice");
    case "invitations":
      return tr(locale, "invitations.loaded_notice", {
        count: i18n().formatInteger(locale, outcome.data.length),
      });
    case "invitation_status":
      return label(locale, "invitations.status_notice");
    case "invitation_revoked":
      return label(locale, "invitations.revoked_notice");
    case "invitation_preview":
      return label(locale, "invitations.preview_notice");
    case "invitation_redeemed":
      return tr(locale, "invitations.redeemed_notice", {
        role: invitationRoleLabel(outcome.data.role, locale),
      });
    default:
      return null;
  }
}

function InvitationCodeCard({
  issued,
  locale,
}: {
  issued: IssuedInvitation;
  locale: Locale;
}) {
  const [copyNotice, setCopyNotice] = useState("");
  const { invitation, remaining_uses } = issued.status;
  async function copyCode(): Promise<void> {
    try {
      await navigator.clipboard.writeText(issued.code);
      setCopyNotice(label(locale, "invitations.copied"));
    } catch {
      setCopyNotice(label(locale, "invitations.copy_failed"));
    }
  }
  return (
    <section
      className="network-profile-result invitation-code-result"
      data-testid="invitation-created"
      aria-live="polite"
    >
      <div className="network-profile-result-heading">
        <div>
          <span>{label(locale, "invitations.issued_title")}</span>
          <strong>{label(locale, invitationStateKey(issued.status.state))}</strong>
        </div>
        <Check size={16} />
      </div>
      <code className="profile-fingerprint invitation-code" data-testid="invitation-code">
        {issued.code}
      </code>
      <div className="inline-notice warning">
        <ShieldAlert size={15} />
        <span>{label(locale, "invitations.code_once")}</span>
      </div>
      <button className="secondary-button" type="button" onClick={() => void copyCode()}>
        <Copy size={14} />
        {label(locale, "invitations.copy")}
      </button>
      {copyNotice ? (
        <p className="profile-muted" role="status">
          {copyNotice}
        </p>
      ) : null}
      <InvitationMetadata
        invitation={invitation}
        remainingUses={remaining_uses}
        locale={locale}
      />
      <p className="profile-muted">
        {label(locale, "invitations.ordinary_membership")}
      </p>
      <p className="profile-muted">{label(locale, "invitations.not_permanent")}</p>
      <p className="profile-muted">{label(locale, "invitations.privacy_boundary")}</p>
    </section>
  );
}

function InvitationMetadata({
  invitation,
  remainingUses,
  locale,
}: {
  invitation: InvitationStatusView["invitation"];
  remainingUses: number;
  locale: Locale;
}) {
  return (
    <dl className="profile-definition-list invitation-metadata">
      <div>
        <dt>{label(locale, "invitations.role")}</dt>
        <dd>{invitationRoleLabel(invitation.role, locale)}</dd>
      </div>
      <div>
        <dt>{label(locale, "invitations.capabilities")}</dt>
        <dd>{capabilityList(invitation.capabilities, locale)}</dd>
      </div>
      <div>
        <dt>{label(locale, "invitations.used_count")}</dt>
        <dd>
          {tr(locale, "invitations.used_count", {
            used: i18n().formatInteger(locale, invitation.used_count),
            max: i18n().formatInteger(locale, invitation.max_uses),
          })}
        </dd>
      </div>
      <div>
        <dt>{label(locale, "invitations.remaining_uses")}</dt>
        <dd>{i18n().formatInteger(locale, remainingUses)}</dd>
      </div>
      <div>
        <dt>{label(locale, "invitations.expires_at")}</dt>
        <dd>{i18n().formatTimestamp(locale, invitation.expires_at_unix_ms)}</dd>
      </div>
      <div>
        <dt>{label(locale, "invitations.revision")}</dt>
        <dd>{i18n().formatInteger(locale, invitation.revision)}</dd>
      </div>
    </dl>
  );
}

function InvitationStatusCard({
  status,
  locale,
  refreshedAtUnixMs,
}: {
  status: InvitationStatusView;
  locale: Locale;
  refreshedAtUnixMs?: number;
}) {
  return (
    <section
      className="network-profile-result"
      data-testid="invitation-status"
      aria-live="polite"
    >
      <div className="network-profile-result-heading">
        <div>
          <span>{label(locale, "invitations.status_title")}</span>
          <strong>{label(locale, invitationStateKey(status.state))}</strong>
        </div>
        {status.state === "active" ? <Check size={16} /> : <ShieldAlert size={16} />}
      </div>
      <InvitationMetadata
        invitation={status.invitation}
        remainingUses={status.remaining_uses}
        locale={locale}
      />
      {refreshedAtUnixMs ? (
        <p className="profile-muted">
          {tr(locale, "invitations.refreshed_at", {
            time: i18n().formatTimestamp(locale, refreshedAtUnixMs),
          })}
        </p>
      ) : null}
      <p className="profile-muted">{label(locale, "invitations.privacy_boundary")}</p>
    </section>
  );
}

function joinList(
  values: string[],
  locale: Locale,
  emptyKey = "common.none",
): string {
  return values.length > 0
    ? values.join(label(locale, "common.list_separator"))
    : label(locale, emptyKey);
}

function normalizeExitContributionMode(
  mode: ExitContributionMode | null | undefined,
  obligation: ExitContributionObligation | null | undefined,
): ExitContributionMode {
  return mode ?? (obligation === "required" ? "required_on_join" : "disabled");
}

function exitContributionModeLabelKey(mode: ExitContributionMode): string {
  switch (mode) {
    case "autonomous":
      return "join.exit.autonomous";
    case "required_on_join":
      return "join.exit.required";
    case "approval_required":
      return "join.exit.approval_required";
    case "disabled":
      return "join.exit.not_required";
  }
}

function joinRequestExitLabelKey(
  mode: ExitContributionMode,
): string {
  switch (mode) {
    case "autonomous":
      return "join_requests.exit.autonomous";
    case "required_on_join":
      return "join_requests.exit.required";
    case "approval_required":
      return "join_requests.exit.approval_required";
    case "disabled":
      return "join_requests.exit.not_required";
  }
}

function invitationExitLabelKey(
  mode: ExitContributionMode,
): string {
  switch (mode) {
    case "autonomous":
      return "invitations.terms.exit_autonomous";
    case "required_on_join":
      return "invitations.terms.exit_required";
    case "approval_required":
      return "invitations.terms.exit_approval_required";
    case "disabled":
      return "invitations.terms.exit_not_required";
  }
}

function ExitContributionDisclosure({
  locale,
  mode,
  scope,
  purposeKey = "join.exit.disclosure.purpose",
  bandwidthKey = "join.exit.disclosure.bandwidth",
  trafficKey = "join.exit.disclosure.traffic",
  loggingKey = "join.exit.disclosure.logging",
  operationalImpactKey = "join.exit.disclosure.operational_impact",
  disableKey = "join.exit.disclosure.disable",
  localSupported,
  localEnabled,
  canJoinWithoutLocalExit,
}: {
  locale: Locale;
  mode: ExitContributionMode;
  scope: NodeNetworkJoinPreview["exit_contribution_scope"];
  purposeKey?: string;
  bandwidthKey?: string;
  trafficKey?: string;
  loggingKey?: string;
  operationalImpactKey?: string;
  disableKey?: string;
  localSupported: boolean;
  localEnabled: boolean;
  canJoinWithoutLocalExit: boolean;
}) {
  const required = mode === "required_on_join";
  const contributionAvailable = mode !== "disabled";
  return (
    <section
      className={`exit-contribution-disclosure ${required ? "required" : ""}`}
      data-testid="exit-contribution-disclosure"
    >
      <h4>{label(locale, "join.exit_contribution")}</h4>
      <p>
        <strong>
          {label(locale, exitContributionModeLabelKey(mode))}
        </strong>
      </p>
      {contributionAvailable ? (
        <>
          <p>{label(locale, purposeKey)}</p>
          <p>{label(locale, bandwidthKey)}</p>
          <p>{label(locale, trafficKey)}</p>
          <p>{label(locale, loggingKey)}</p>
          <p>{label(locale, operationalImpactKey)}</p>
          <p>{label(locale, disableKey)}</p>
          {required ? <p>{label(locale, `join.exit.scope.${scope}`)}</p> : null}
        </>
      ) : null}
      <p>
        {label(
          locale,
          localSupported
            ? "join.exit.local_supported"
            : "join.exit.local_unsupported",
        )}
      </p>
      <p>
        {label(
          locale,
          localEnabled
            ? "join.exit.local_enabled"
            : "join.exit.local_not_enabled",
        )}
      </p>
      <p>
        {label(
          locale,
          canJoinWithoutLocalExit
            ? "join.exit.can_join_without_local_exit"
            : "join.exit.cannot_join_without_local_exit",
        )}
      </p>
    </section>
  );
}

function JoinPreviewCard({
  preview,
  locale,
  checked,
  exitContributionChecked,
  pending,
  blockedReason,
  onCheckedChange,
  onExitContributionCheckedChange,
  onPreviewAgain,
  onConfirm,
}: {
  preview: NodeNetworkJoinPreview;
  locale: Locale;
  checked: boolean;
  exitContributionChecked: boolean;
  pending: boolean;
  blockedReason: string;
  onCheckedChange(checked: boolean): void;
  onExitContributionCheckedChange(checked: boolean): void;
  onPreviewAgain(): void;
  onConfirm(): void;
}) {
  const canConfirm =
    preview.can_join &&
    checked &&
    (!preview.join_requires_exit_consent || exitContributionChecked) &&
    !pending &&
    !blockedReason;
  return (
    <section
      className="network-profile-result join-preview-result"
      data-testid="network-join-preview"
      aria-live="polite"
    >
      <div className="network-profile-result-heading">
        <div>
          <span>{label(locale, "join.preview_title")}</span>
          <strong>{preview.network_name}</strong>
        </div>
        {preview.can_join ? <Check size={16} /> : <ShieldAlert size={16} />}
      </div>
      <div className="profile-result-status">
        <span>
          {label(
            locale,
            preview.can_join ? "join.can_join" : "join.cannot_join",
          )}
        </span>
        {preview.already_member ? (
          <span>{label(locale, "join.already_member")}</span>
        ) : null}
        <span>
          {label(locale, `choice.join_mode.${preview.join_mode}`)}
        </span>
        <span>
          {label(locale, `choice.visibility.${preview.visibility}`)}
        </span>
      </div>
      {!preview.can_join ? (
        <div className="inline-notice danger">
          <ShieldAlert size={15} />
          <span>
            {label(
              locale,
              preview.blocked_reason_key || "join.error.unavailable",
            )}
          </span>
        </div>
      ) : null}
      {blockedReason ? (
        <div className="inline-notice warning">
          <ShieldAlert size={15} />
          <span>{blockedReason}</span>
        </div>
      ) : null}
      <dl className="profile-definition-list join-preview-details">
        <div>
          <dt>{label(locale, "join.network_id")}</dt>
          <dd>
            <code>{preview.network_id}</code>
          </dd>
        </div>
        <div>
          <dt>{label(locale, "join.owner")}</dt>
          <dd>
            <code>{preview.owner}</code>
          </dd>
        </div>
        <div>
          <dt>{label(locale, "join.administrators")}</dt>
          <dd>{joinList(preview.administrators, locale)}</dd>
        </div>
        <div>
          <dt>{label(locale, "join.discovery_revision")}</dt>
          <dd>{i18n().formatInteger(locale, preview.discovery_revision)}</dd>
        </div>
        <div>
          <dt>{label(locale, "join.network_policy_revision")}</dt>
          <dd>
            {i18n().formatInteger(locale, preview.network_policy_revision)}
          </dd>
        </div>
        <div>
          <dt>{label(locale, "join.privacy_policy_revision")}</dt>
          <dd>
            {i18n().formatInteger(locale, preview.privacy_policy_revision)}
          </dd>
        </div>
        <div>
          <dt>{label(locale, "join.members")}</dt>
          <dd>
            {tr(locale, "join.member_count", {
              count: i18n().formatInteger(locale, preview.member_count),
              limit:
                preview.member_limit == null
                  ? label(locale, "common.unlimited")
                  : i18n().formatInteger(locale, preview.member_limit),
            })}
          </dd>
        </div>
        <div>
          <dt>{label(locale, "join.exit_contribution")}</dt>
          <dd>
            {label(
              locale,
              exitContributionModeLabelKey(preview.exit_contribution_mode),
            )}
          </dd>
        </div>
        <div>
          <dt>{label(locale, "join.privacy_changes")}</dt>
          <dd>{joinList(preview.privacy_changes, locale)}</dd>
        </div>
        <div>
          <dt>{label(locale, "join.capabilities")}</dt>
          <dd>
            {joinList(
              preview.capability_summary.map((capability) =>
                label(locale, `capability.${capability}`),
              ),
              locale,
            )}
          </dd>
        </div>
        <div>
          <dt>{label(locale, "join.local_capabilities")}</dt>
          <dd>
            {joinList(
              preview.locally_contributed_capabilities.map((capability) =>
                label(locale, `capability.${capability}`),
              ),
              locale,
              "join.local_capabilities_none",
            )}
          </dd>
        </div>
        <div>
          <dt>{label(locale, "join.contribution_policy")}</dt>
          <dd>{label(locale, preview.contribution_policy_key)}</dd>
        </div>
        <div>
          <dt>{label(locale, "join.endpoints")}</dt>
          <dd>
            {joinList(
              preview.endpoints.map(
                (endpoint) =>
                  `${endpoint.transport}://${endpoint.host}:${endpoint.port}`,
              ),
              locale,
            )}
          </dd>
        </div>
      </dl>
      <div className="profile-signature-meta">
        <span>
          {tr(locale, "join.issued_at", {
            time: i18n().formatTimestamp(locale, preview.issued_at_unix_ms),
          })}
        </span>
        <span>
          {tr(locale, "join.expires_at", {
            time: i18n().formatTimestamp(locale, preview.expires_at_unix_ms),
          })}
        </span>
        <span>
          {tr(locale, "join.preview_digest", {
            digest: preview.preview_digest,
          })}
        </span>
      </div>
      <div className="inline-notice">
        <ShieldAlert size={15} />
        <span>{label(locale, "join.local_revocable")}</span>
      </div>
      <ExitContributionDisclosure
        locale={locale}
        mode={preview.exit_contribution_mode}
        scope={preview.exit_contribution_scope}
        purposeKey={preview.exit_contribution_purpose_key}
        bandwidthKey={preview.exit_contribution_bandwidth_impact_key}
        trafficKey={preview.exit_contribution_traffic_impact_key}
        loggingKey={preview.exit_contribution_logging_boundary_key}
        operationalImpactKey={preview.exit_contribution_operational_impact_key}
        disableKey={preview.exit_contribution_disable_key}
        localSupported={preview.local_exit_available}
        localEnabled={preview.local_exit_enabled}
        canJoinWithoutLocalExit={preview.can_join_without_local_exit}
      />
      <label className="check-field join-terms-check">
        <input
          type="checkbox"
          checked={checked}
          disabled={pending || !preview.can_join}
          onChange={(event) => onCheckedChange(event.target.checked)}
        />
        <span>{label(locale, "join.terms_checkbox")}</span>
      </label>
      {preview.join_requires_exit_consent ? (
        <label className="check-field join-terms-check">
          <input
            type="checkbox"
            checked={exitContributionChecked}
            disabled={pending || !preview.can_join}
            onChange={(event) =>
              onExitContributionCheckedChange(event.target.checked)
            }
          />
          <span>{label(locale, "join.exit.consent")}</span>
        </label>
      ) : null}
      <div className="recovery-actions">
        <button
          className="secondary-button"
          type="button"
          disabled={pending}
          onClick={onPreviewAgain}
        >
          {label(locale, "join.preview_again")}
        </button>
        <button
          className="primary-button"
          type="button"
          disabled={!canConfirm}
          onClick={onConfirm}
        >
          {pending
            ? label(locale, "common.operation_in_progress")
            : label(locale, "join.confirm")}
        </button>
      </div>
    </section>
  );
}

function InvitationListCard({
  invitations,
  loadedAtUnixMs,
  locale,
  statusAction,
  revokeAction,
  statusBlockedReason,
  revokeBlockedReason,
  pendingAction,
  onSubmit,
  onOutcome,
}: {
  invitations: InvitationStatusView[];
  loadedAtUnixMs: number;
  locale: Locale;
  statusAction?: NodeUiAction;
  revokeAction?: NodeUiAction;
  statusBlockedReason: string;
  revokeBlockedReason: string;
  pendingAction: string | null;
  onSubmit(
    action: NodeUiAction,
    values: ActionValues,
    onOperation: (operation: NodeOperationSnapshot) => void,
  ): Promise<ActionSubmission>;
  onOutcome(outcome: NodeCommandOutcome): void;
}) {
  const [busy, setBusy] = useState(false);
  const [failure, setFailure] = useState("");
  const [revokeTarget, setRevokeTarget] =
    useState<InvitationStatusView | null>(null);

  async function refreshInvitation(status: InvitationStatusView): Promise<void> {
    if (!statusAction || busy || statusBlockedReason) return;
    setBusy(true);
    setFailure("");
    try {
      const submission = await onSubmit(
        statusAction,
        {
          network_id: status.invitation.network_id,
          invitation_id: status.invitation.invitation_id,
        },
        () => {},
      );
      if (submission.outcome?.status === "rejected") {
        setFailure(outcomeSummary(submission.outcome, locale));
      } else if (submission.outcome) {
        onOutcome(submission.outcome);
      } else if (submission.operation) {
        setFailure(operationFailureSummary(submission.operation, locale));
      } else {
        setFailure(label(locale, "common.operation_failed"));
      }
    } catch (error) {
      setFailure(error instanceof Error ? error.message : String(error));
    } finally {
      setBusy(false);
    }
  }

  async function revokeInvitation(): Promise<void> {
    if (!revokeAction || !revokeTarget || busy || revokeBlockedReason) return;
    setBusy(true);
    setFailure("");
    try {
      const submission = await onSubmit(
        revokeAction,
        {
          network_id: revokeTarget.invitation.network_id,
          invitation_id: revokeTarget.invitation.invitation_id,
          expected_revision: String(revokeTarget.invitation.revision),
        },
        () => {},
      );
      if (submission.outcome?.status === "rejected") {
        setFailure(outcomeSummary(submission.outcome, locale));
      } else if (submission.outcome) {
        onOutcome(submission.outcome);
        setRevokeTarget(null);
      } else if (submission.operation) {
        setFailure(operationFailureSummary(submission.operation, locale));
      } else {
        setFailure(label(locale, "common.operation_failed"));
      }
    } catch (error) {
      setFailure(error instanceof Error ? error.message : String(error));
    } finally {
      setBusy(false);
    }
  }

  return (
    <>
      <section
        className="network-profile-result"
        data-testid="invitation-list"
        aria-live="polite"
      >
        <div className="network-profile-result-heading">
          <div>
            <span>{label(locale, "invitations.list_title")}</span>
            <strong>
              {tr(locale, "invitations.count", {
                count: i18n().formatInteger(locale, invitations.length),
              })}
            </strong>
          </div>
          <ShieldAlert size={16} />
        </div>
        <p className="profile-muted">
          {tr(locale, "invitations.refreshed_at", {
            time: i18n().formatTimestamp(locale, loadedAtUnixMs),
          })}
        </p>
        <p className="profile-muted">{label(locale, "invitations.privacy_boundary")}</p>
        {failure ? (
          <p className="form-error" role="alert">
            {failure}
          </p>
        ) : null}
        {statusBlockedReason || revokeBlockedReason ? (
          <div className="inline-notice warning">
            <ShieldAlert size={15} />
            <span>{statusBlockedReason || revokeBlockedReason}</span>
          </div>
        ) : null}
        {invitations.length === 0 ? (
          <p className="profile-muted" data-testid="invitation-empty">
            {label(locale, "invitations.empty")}
          </p>
        ) : (
          <ul className="profile-warning-list invitation-list">
            {invitations.map((status) => (
              <li key={status.invitation.invitation_id}>
                <ShieldAlert size={14} />
                <div>
                  <strong>{label(locale, invitationStateKey(status.state))}</strong>
                  <InvitationMetadata
                    invitation={status.invitation}
                    remainingUses={status.remaining_uses}
                    locale={locale}
                  />
                  <div className="invitation-row-actions">
                    <button
                      className="secondary-button"
                      type="button"
                      disabled={busy || Boolean(statusBlockedReason) || !statusAction}
                      onClick={() => void refreshInvitation(status)}
                    >
                      {busy && pendingAction === statusAction?.id
                        ? label(locale, "common.operation_in_progress")
                        : label(locale, "invitations.status_refresh")}
                    </button>
                    <button
                      className="danger-button"
                      type="button"
                      disabled={
                        busy ||
                        Boolean(revokeBlockedReason) ||
                        !revokeAction ||
                        status.state !== "active"
                      }
                      onClick={() => setRevokeTarget(status)}
                    >
                      {label(locale, "invitations.revoke")}
                    </button>
                  </div>
                </div>
              </li>
            ))}
          </ul>
        )}
      </section>
      <Dialog.Root
        open={revokeTarget !== null}
        onOpenChange={(open) => {
          if (!open) setRevokeTarget(null);
        }}
      >
        <Dialog.Portal>
          <Dialog.Overlay className="dialog-overlay" />
          <Dialog.Content className="dialog-content confirm-dialog">
            <div className="dialog-heading">
              <div className="dialog-icon danger">
                <AlertTriangle size={20} />
              </div>
              <div>
                <Dialog.Title>{label(locale, "confirm.revoke_invitation.title")}</Dialog.Title>
                <Dialog.Description>
                  {label(locale, "confirm.revoke_invitation.consequence")}
                </Dialog.Description>
                <p className="dialog-secondary-copy">
                  {label(locale, "confirm.reauthentication_required")}
                </p>
              </div>
            </div>
            {failure ? (
              <p className="form-error" role="alert">
                {failure}
              </p>
            ) : null}
            <div className="dialog-actions">
              <Dialog.Close asChild>
                <button className="secondary-button" type="button">
                  {label(locale, "common.cancel")}
                </button>
              </Dialog.Close>
              <button
                className="danger-button"
                type="button"
                disabled={busy || Boolean(revokeBlockedReason)}
                onClick={() => void revokeInvitation()}
              >
                {busy
                  ? label(locale, "common.operation_in_progress")
                  : label(locale, "common.execute")}
              </button>
            </div>
          </Dialog.Content>
        </Dialog.Portal>
      </Dialog.Root>
    </>
  );
}

function InvitationPreviewCard({
  preview,
  locale,
  checked,
  exitContributionChecked,
  localExitAvailable,
  localExitEnabled,
  pending,
  blockedReason,
  sourceValues,
  previewLoadedAtUnixMs,
  onCheckedChange,
  onExitContributionCheckedChange,
  onRetryPreview,
  onBackToEdit,
  onConfirm,
}: {
  preview: InvitationPreview;
  locale: Locale;
  checked: boolean;
  exitContributionChecked: boolean;
  localExitAvailable: boolean;
  localExitEnabled: boolean;
  pending: boolean;
  blockedReason: string;
  sourceValues: ActionValues;
  previewLoadedAtUnixMs: number;
  onCheckedChange(checked: boolean): void;
  onExitContributionCheckedChange(checked: boolean): void;
  onRetryPreview(): void;
  onBackToEdit(): void;
  onConfirm(values: ActionValues): void;
}) {
  const exitMode = normalizeExitContributionMode(
    preview.terms.exit_contribution_mode,
    preview.terms.exit_contribution,
  );
  const exitRequired = exitMode === "required_on_join";
  const previewAcceptable =
    preview.can_accept &&
    preview.state === "active" &&
    preview.remaining_uses > 0 &&
    preview.expires_at_unix_ms > previewLoadedAtUnixMs &&
    !preview.creates_permanent_membership &&
    (!exitRequired || localExitAvailable);
  const canConfirm =
    previewAcceptable &&
    checked &&
    (!exitRequired || exitContributionChecked) &&
    !pending &&
    !blockedReason;
  const terms = [
    tr(locale, "invitations.terms.network_policy", {
      revision: i18n().formatInteger(locale, preview.terms.network_policy_revision),
    }),
    tr(locale, "invitations.terms.privacy_policy", {
      revision: i18n().formatInteger(locale, preview.terms.privacy_policy_revision),
    }),
    label(locale, invitationExitLabelKey(exitMode)),
    preview.terms.member_limit == null
      ? label(locale, "common.unlimited")
      : tr(locale, "invitations.terms.member_limit", {
          limit: i18n().formatInteger(locale, preview.terms.member_limit),
        }),
  ];
  return (
    <section
      className="network-profile-result join-preview-result"
      data-testid="invitation-preview"
      aria-live="polite"
    >
      <div className="network-profile-result-heading">
        <div>
          <span>{label(locale, "invitations.preview_title")}</span>
          <strong>{preview.network_name}</strong>
        </div>
        {previewAcceptable ? <Check size={16} /> : <ShieldAlert size={16} />}
      </div>
      <div className="profile-result-status">
        <span>
          {label(
            locale,
            previewAcceptable
              ? "invitations.can_accept"
              : "invitations.cannot_accept",
          )}
        </span>
        <span>{label(locale, invitationStateKey(preview.state))}</span>
      </div>
      {previewAcceptable ? null : (
        <div className="inline-notice danger">
          <ShieldAlert size={15} />
          <span>
            {preview.denial_key
              ? label(locale, preview.denial_key)
              : label(locale, "invitations.denied")}
          </span>
        </div>
      )}
      {blockedReason ? (
        <div className="inline-notice warning">
          <ShieldAlert size={15} />
          <span>{blockedReason}</span>
        </div>
      ) : null}
      <dl className="profile-definition-list invitation-preview-details">
        <div>
          <dt>{label(locale, "invitations.role")}</dt>
          <dd>{invitationRoleLabel(preview.role, locale)}</dd>
        </div>
        <div>
          <dt>{label(locale, "invitations.capabilities")}</dt>
          <dd>{capabilityList(preview.capabilities, locale)}</dd>
        </div>
        <div>
          <dt>{label(locale, "invitations.permission_changes")}</dt>
          <dd>{joinList(preview.permission_changes, locale)}</dd>
        </div>
        <div>
          <dt>{label(locale, "invitations.terms")}</dt>
          <dd>{joinList(terms, locale)}</dd>
        </div>
        <div>
          <dt>{label(locale, "invitations.expires_at")}</dt>
          <dd>{i18n().formatTimestamp(locale, preview.expires_at_unix_ms)}</dd>
        </div>
        <div>
          <dt>{label(locale, "invitations.remaining_uses")}</dt>
          <dd>{i18n().formatInteger(locale, preview.remaining_uses)}</dd>
        </div>
      </dl>
      {preview.requires_recent_reauthentication ? (
        <div className="inline-notice warning">
          <ShieldAlert size={15} />
          <span>{label(locale, "invitations.reauthentication_required")}</span>
        </div>
      ) : null}
      <ExitContributionDisclosure
        locale={locale}
        mode={exitMode}
        scope={preview.terms.exit_contribution_scope ?? "not_applicable"}
        localSupported={localExitAvailable}
        localEnabled={localExitEnabled}
        canJoinWithoutLocalExit={!exitRequired}
      />
      <p className="profile-muted">{label(locale, "invitations.ordinary_membership")}</p>
      <p className="profile-muted">{label(locale, "invitations.not_permanent")}</p>
      <label className="check-field join-terms-check">
        <input
          type="checkbox"
          checked={checked}
          disabled={pending || !previewAcceptable}
          onChange={(event) => onCheckedChange(event.target.checked)}
        />
        <span>{label(locale, "invitations.terms_checkbox")}</span>
      </label>
      {exitRequired ? (
        <label className="check-field join-terms-check">
          <input
            type="checkbox"
            checked={exitContributionChecked}
            disabled={pending || !previewAcceptable}
            onChange={(event) =>
              onExitContributionCheckedChange(event.target.checked)
            }
          />
          <span>{label(locale, "join.exit.consent")}</span>
        </label>
      ) : null}
      <div className="recovery-actions">
        {!previewAcceptable ? (
          <button
            className="secondary-button"
            type="button"
            disabled={pending}
            onClick={onBackToEdit}
          >
            {label(locale, "invitations.back_to_edit")}
          </button>
        ) : null}
        <button
          className="secondary-button"
          type="button"
          disabled={pending}
          onClick={onRetryPreview}
        >
          {label(locale, "invitations.preview_again")}
        </button>
        <button
          className="primary-button"
          type="button"
          disabled={!canConfirm}
          onClick={() =>
            onConfirm(
              invitationPreviewValues(preview, sourceValues, exitRequired),
            )
          }
        >
          {pending
            ? label(locale, "common.operation_in_progress")
            : label(locale, "invitations.accept")}
        </button>
      </div>
    </section>
  );
}

function OperationStatusCard({
  operation,
  locale,
  onCancel,
  onRetry,
}: {
  operation: NodeOperationSnapshot;
  locale: Locale;
  onCancel(operationId: string): Promise<void>;
  onRetry?: () => Promise<void>;
}) {
  const [copied, setCopied] = useState(false);
  const [copyFailure, setCopyFailure] = useState(false);
  const [cancelPending, setCancelPending] = useState(false);
  const [cancelFailure, setCancelFailure] = useState("");
  const [retryPending, setRetryPending] = useState(false);
  const [retryFailure, setRetryFailure] = useState("");
  const cancellation = operation.cancellable
    ? label(locale, "operation.cancellable_yes")
    : tr(locale, "operation.cancellable_no", {
        reason: operation.cancel_blocked_reason
          ? label(locale, operation.cancel_blocked_reason)
          : label(locale, "operation.cancellable_unknown"),
      });

  async function copyDiagnosticId(): Promise<void> {
    try {
      await navigator.clipboard.writeText(operation.operation_id);
      setCopied(true);
      setCopyFailure(false);
    } catch {
      setCopied(false);
      setCopyFailure(true);
    }
  }

  async function cancel(): Promise<void> {
    if (cancelPending) return;
    setCancelPending(true);
    setCancelFailure("");
    try {
      await onCancel(operation.operation_id);
    } catch (error) {
      setCancelFailure(
        tr(locale, "operation.cancel_failed", {
          error: error instanceof Error ? error.message : String(error),
        }),
      );
    } finally {
      setCancelPending(false);
    }
  }

  async function retry(): Promise<void> {
    if (!onRetry || retryPending) return;
    setRetryPending(true);
    setRetryFailure("");
    try {
      await onRetry();
    } catch (error) {
      setRetryFailure(
        tr(locale, "operation.retry_failed", {
          error: error instanceof Error ? error.message : String(error),
        }),
      );
    } finally {
      setRetryPending(false);
    }
  }

  const showCancel =
    operation.cancellable && !operationStateIsTerminal(operation.state);
  const showRetry =
    operationStateIsTerminal(operation.state) &&
    operation.error?.retryable === true &&
    Boolean(onRetry);

  return (
    <section className="operation-status" data-testid="operation-status">
      <div className="operation-status-heading">
        <div>
          <span>{label(locale, "operation.title")}</span>
          <strong>{label(locale, `operation.state.${operation.state}`)}</strong>
        </div>
        <div className="operation-status-actions">
          {showRetry ? (
            <button
              className="secondary-button"
              type="button"
              disabled={retryPending}
              onClick={() => void retry()}
            >
              {retryPending
                ? label(locale, "operation.retrying")
                : label(locale, "operation.retry")}
            </button>
          ) : null}
          {showCancel ? (
            <button
              className="secondary-button"
              type="button"
              disabled={cancelPending}
              onClick={() => void cancel()}
            >
              {cancelPending
                ? label(locale, "operation.cancelling")
                : label(locale, "operation.cancel")}
            </button>
          ) : null}
          <button
            className="secondary-button operation-copy"
            type="button"
            onClick={() => {
              setCopyFailure(false);
              void copyDiagnosticId();
            }}
          >
            {copied ? <Check size={14} /> : <Copy size={14} />}
            {label(
              locale,
              copied
                ? "operation.diagnostic_copied"
                : "operation.copy_diagnostic_id",
            )}
          </button>
        </div>
      </div>
      <div className="operation-metadata">
        <span>
          {label(locale, "operation.stage")}: <code>{operation.stage}</code>
        </span>
        <span>
          {tr(locale, "operation.attempt", {
            attempt: String(operation.attempt),
            max: String(operation.max_attempts),
          })}
        </span>
        <span>{cancellation}</span>
      </div>
      {operation.error ? (
        <div className="operation-error" role="alert">
          <strong>
            {label(locale, "operation.error_code")}: {operation.error.code}
          </strong>
          <p>{operationFailureSummary(operation, locale)}</p>
          <span>
            {label(
              locale,
              operation.error.retryable
                ? "operation.retryable"
                : "operation.not_retryable",
            )}
          </span>
        </div>
      ) : null}
      {cancelFailure ? (
        <div className="inline-notice warning" role="alert">
          <AlertTriangle size={15} />
          <span>{cancelFailure}</span>
        </div>
      ) : null}
      {retryFailure ? (
        <div className="inline-notice warning" role="alert">
          <AlertTriangle size={15} />
          <span>{retryFailure}</span>
        </div>
      ) : null}
      {copyFailure ? (
        <p className="form-error" role="alert">
          {label(locale, "diagnostic.copy_failed")}
        </p>
      ) : null}
      <div className="operation-diagnostic-id">
        <span>{label(locale, "operation.diagnostic_id")}</span>
        <code>{operation.operation_id}</code>
      </div>
      {copied ? (
        <span className="sr-only" role="status">
          {label(locale, "operation.diagnostic_copied")}
        </span>
      ) : null}
    </section>
  );
}

const joinRequestSoonToExpireMs = 24 * 60 * 60 * 1000;

function joinRequestIsReviewable(request: JoinRequest): boolean {
  return request.state === "pending" || request.state === "needs_information";
}

type JoinRequestReviewDialogState = {
  action: JoinRequestReviewAction;
  batch: boolean;
  requests: JoinRequest[];
};

function JoinRequestRisk({
  request,
  locale,
  loadedAtUnixMs,
}: {
  request: JoinRequest;
  locale: Locale;
  loadedAtUnixMs: number;
}) {
  const grant = request.grant_preview;
  const exitMode = normalizeExitContributionMode(
    grant.exit_contribution_mode,
    grant.exit_contribution,
  );
  const capabilities =
    grant.capabilities && grant.capabilities.length > 0
      ? grant.capabilities
          .map((capability) => label(locale, `capability.${capability}`))
          .join(" · ")
      : label(locale, "common.none");
  return (
    <section className="join-request-risk" data-testid="join-request-risk">
      <div className="join-request-risk-heading">
        <ShieldAlert size={15} />
        <strong>{label(locale, "join_requests.risk")}</strong>
      </div>
      <dl className="join-request-risk-list">
        <div>
          <dt>{label(locale, "join_requests.role")}</dt>
          <dd>{label(locale, `join_request.role.${grant.role}`)}</dd>
        </div>
        <div>
          <dt>{label(locale, "join_requests.capabilities")}</dt>
          <dd>{capabilities}</dd>
        </div>
        <div>
          <dt>{label(locale, "join_requests.exit")}</dt>
          <dd>
            {label(locale, joinRequestExitLabelKey(exitMode))}
          </dd>
        </div>
        <div>
          <dt>{label(locale, "join_requests.file_access")}</dt>
          <dd>
            {label(
              locale,
              grant.file_access
                ? "join_requests.file_access.granted"
                : "join_requests.file_access.not_granted",
            )}
          </dd>
        </div>
        <div>
          <dt>{label(locale, "join_requests.data_plane")}</dt>
          <dd>
            {label(
              locale,
              grant.grants_data_plane_access
                ? "join_requests.data_plane.granted"
                : "join_requests.data_plane.not_granted",
            )}
          </dd>
        </div>
        <div>
          <dt>{label(locale, "join_requests.expires_at")}</dt>
          <dd>
            {i18n().formatTimestamp(locale, request.expires_at_unix_ms)}
          </dd>
        </div>
      </dl>
      {request.expires_at_unix_ms <=
      loadedAtUnixMs + joinRequestSoonToExpireMs ? (
        <p className="join-request-expiry-warning">
          <AlertTriangle size={14} />
          <span>{label(locale, "join_requests.expiring_soon")}</span>
        </p>
      ) : null}
    </section>
  );
}

function JoinRequestListCard({
  result,
  locale,
  loadedAtUnixMs,
  reviewAction,
  batchReviewAction,
  reviewBlockedReason,
  batchReviewBlockedReason,
  activePendingAction,
  onSubmit,
  onReviewed,
}: {
  result: NodeJoinRequestListResult;
  locale: Locale;
  loadedAtUnixMs: number;
  reviewAction?: NodeUiAction;
  batchReviewAction?: NodeUiAction;
  reviewBlockedReason: string;
  batchReviewBlockedReason: string;
  activePendingAction: string | null;
  onSubmit: ActionPanelProps["onSubmit"];
  onReviewed(ack: NodeJoinRequestReviewAck): void;
}) {
  const [selectedIds, setSelectedIds] = useState<string[]>([]);
  const [dialog, setDialog] = useState<JoinRequestReviewDialogState | null>(
    null,
  );
  const [note, setNote] = useState("");
  const [reviewErrors, setReviewErrors] = useState<string[]>([]);
  const [reviewFailure, setReviewFailure] = useState("");
  const [reviewFeedback, setReviewFeedback] = useState("");
  const [submitting, setSubmitting] = useState(false);
  const reviewableRequests = result.requests.filter(joinRequestIsReviewable);
  const selectedRequests = reviewableRequests.filter((request) =>
    selectedIds.includes(request.request_id),
  );
  const reviewPending = activePendingAction === "review_join_request";
  const batchReviewPending =
    activePendingAction === "batch_review_join_requests";
  const pending = reviewPending || batchReviewPending || submitting;

  function toggleRequest(requestId: string): void {
    setSelectedIds((current) =>
      current.includes(requestId)
        ? current.filter((id) => id !== requestId)
        : [...current, requestId],
    );
  }

  function openReview(
    requests: JoinRequest[],
    action: JoinRequestReviewAction,
    batch: boolean,
  ): void {
    const blockedReason = batch
      ? batchReviewBlockedReason
      : reviewBlockedReason;
    if (blockedReason || pending || requests.length === 0) return;
    setDialog({ requests, action, batch });
    setNote("");
    setReviewErrors([]);
    setReviewFailure("");
    setReviewFeedback("");
  }

  async function submitReview(): Promise<void> {
    if (!dialog || pending) return;
    const blockedReason = dialog.batch
      ? batchReviewBlockedReason
      : reviewBlockedReason;
    if (blockedReason) return;
    const action = dialog.batch ? batchReviewAction : reviewAction;
    if (!action) {
      setReviewFailure(label(locale, "join_requests.actions_unavailable"));
      return;
    }
    const values: ActionValues = dialog.batch
      ? {
          network_id: result.network_id,
          reviews: JSON.stringify(
            dialog.requests.map((request) => ({
              request_id: request.request_id,
              expected_revision: request.revision,
              action: dialog.action,
              ...(dialog.action === "approve" || !note
                ? {}
                : { note }),
            })),
          ),
        }
      : {
          network_id: result.network_id,
          request_id: dialog.requests[0].request_id,
          expected_revision: String(dialog.requests[0].revision),
          action: dialog.action,
          ...(dialog.action === "approve" ? {} : { note }),
        };
    const validation = validateActionValues(action, values, locale);
    if (validation.length > 0) {
      setReviewErrors(validation);
      return;
    }

    setSubmitting(true);
    setReviewErrors([]);
    setReviewFailure("");
    try {
      const submission = await onSubmit(action, values, () => {});
      if (submission.outcome?.status === "rejected") {
        setReviewFailure(outcomeSummary(submission.outcome, locale));
      } else if (submission.outcome?.status === "join_requests_reviewed") {
        onReviewed(submission.outcome.data);
        setReviewFeedback(
          tr(locale, "join_requests.review_succeeded", {
            count: String(submission.outcome.data.reviews.length),
          }),
        );
        setSelectedIds((current) =>
          current.filter(
            (id) =>
              !submission.outcome?.status ||
              submission.outcome.status !== "join_requests_reviewed" ||
              !submission.outcome.data.reviews.some(
                (review) => review.request_id === id,
              ),
          ),
        );
        setDialog(null);
      } else if (submission.operation) {
        setReviewFailure(operationFailureSummary(submission.operation, locale));
      } else {
        setReviewFailure(label(locale, "common.operation_failed"));
      }
    } catch (error) {
      setReviewFailure(error instanceof Error ? error.message : String(error));
    } finally {
      setSubmitting(false);
    }
  }

  return (
    <>
      <section
        className="join-request-list"
        data-testid="join-request-list"
        aria-label={label(locale, "join_requests.title")}
      >
        <div className="join-request-list-heading">
          <div>
            <span>{label(locale, "join_requests.title")}</span>
            <strong>
              {tr(locale, "join_requests.count", {
                count: String(result.requests.length),
              })}
            </strong>
          </div>
          <code>{result.network_id}</code>
        </div>
        <p className="join-request-privacy">
          {label(locale, "join_requests.privacy_boundary")}
        </p>
        {reviewFeedback ? (
          <p className="join-request-review-feedback" role="status">
            <Check size={15} />
            <span>{reviewFeedback}</span>
          </p>
        ) : null}
        {reviewBlockedReason || batchReviewBlockedReason ? (
          <p className="join-request-review-blocked">
            {reviewBlockedReason || batchReviewBlockedReason}
          </p>
        ) : null}
        {reviewableRequests.length > 1 &&
        reviewAction &&
        batchReviewAction ? (
          <div
            className="join-request-selection-bar"
            data-testid="join-request-selection-bar"
          >
            <label className="join-request-select-all">
              <input
                type="checkbox"
                checked={selectedRequests.length === reviewableRequests.length}
                onChange={(event) =>
                  setSelectedIds(
                    event.target.checked
                      ? reviewableRequests.map((request) => request.request_id)
                      : [],
                  )
                }
              />
              <span>{label(locale, "join_requests.select_all")}</span>
            </label>
            <span className="join-request-selected-count">
              {tr(locale, "join_requests.selected_count", {
                count: String(selectedRequests.length),
              })}
            </span>
            <div className="join-request-batch-actions">
              {(["approve", "reject", "request_information"] as const).map(
                (reviewActionValue) => (
                  <button
                    key={reviewActionValue}
                    type="button"
                    className="secondary-button join-request-action-button"
                    disabled={
                      pending ||
                      Boolean(batchReviewBlockedReason) ||
                      selectedRequests.length === 0
                    }
                    onClick={() =>
                      openReview(selectedRequests, reviewActionValue, true)
                    }
                  >
                    {label(
                      locale,
                      `join_requests.batch.${reviewActionValue}`,
                    )}
                  </button>
                ),
              )}
            </div>
          </div>
        ) : null}
        {result.requests.length === 0 ? (
          <p className="join-request-empty" data-testid="join-request-empty">
            {label(locale, "join_requests.empty")}
          </p>
        ) : (
          <div className="join-request-items">
            {result.requests.map((request) => {
              const visibility =
                request.reason_visibility ?? "owners_and_administrators";
              const reviewable = joinRequestIsReviewable(request);
              return (
                <article
                  className="join-request-item"
                  data-testid="join-request-item"
                  key={request.request_id}
                >
                  <header className="join-request-item-heading">
                    <div>
                      <span>{label(locale, "join_requests.applicant")}</span>
                      <strong>
                        {request.applicant.display_name ??
                          request.applicant.node_id}
                      </strong>
                    </div>
                    <div className="join-request-status-chips">
                      <span>
                        {label(locale, `join_request.state.${request.state}`)}
                      </span>
                      <span>
                        {label(locale, "join_requests.revision")}:{" "}
                        {request.revision}
                      </span>
                      {reviewable && reviewAction ? (
                        <label className="join-request-row-select">
                          <input
                            type="checkbox"
                            aria-label={tr(locale, "join_requests.select_request", {
                              applicant:
                                request.applicant.display_name ??
                                request.applicant.node_id,
                            })}
                            checked={selectedIds.includes(request.request_id)}
                            onChange={() => toggleRequest(request.request_id)}
                          />
                          <span>{label(locale, "join_requests.select")}</span>
                        </label>
                      ) : null}
                    </div>
                  </header>
                  <dl className="join-request-applicant">
                    <div>
                      <dt>{label(locale, "join_requests.node_id")}</dt>
                      <dd>
                        <code>{request.applicant.node_id}</code>
                      </dd>
                    </div>
                    <div>
                      <dt>{label(locale, "join_requests.fingerprint")}</dt>
                      <dd>
                        <code>{request.applicant.public_key_fingerprint}</code>
                      </dd>
                    </div>
                    <div>
                      <dt>{label(locale, "join_requests.request_id")}</dt>
                      <dd>
                        <code>{request.request_id}</code>
                      </dd>
                    </div>
                  </dl>
                  <section className="join-request-reason">
                    <h4>{label(locale, "join_requests.reason")}</h4>
                    <blockquote>
                      {request.reason ?? label(locale, "join_requests.reason_redacted")}
                    </blockquote>
                    <p>
                      {label(locale, `join_requests.reason_visibility.${visibility}`)}
                    </p>
                  </section>
                  {request.information_request ? (
                    <p className="join-request-information">
                      <strong>
                        {label(locale, "join_requests.information_request")}
                      </strong>{" "}
                      {request.information_request}
                    </p>
                  ) : null}
                  <JoinRequestRisk
                    request={request}
                    locale={locale}
                    loadedAtUnixMs={loadedAtUnixMs}
                  />
                  {reviewable && reviewAction ? (
                    <div
                      className="join-request-row-actions"
                      aria-label={label(locale, "join_requests.review_actions")}
                    >
                      {(["approve", "reject", "request_information"] as const).map(
                        (reviewActionValue) => (
                          <button
                            key={reviewActionValue}
                            type="button"
                            className="secondary-button join-request-action-button"
                            disabled={pending || Boolean(reviewBlockedReason)}
                            onClick={() =>
                              openReview([request], reviewActionValue, false)
                            }
                          >
                            {label(
                              locale,
                              `choice.join_request_review.${reviewActionValue}`,
                            )}
                          </button>
                        ),
                      )}
                    </div>
                  ) : null}
                </article>
              );
            })}
          </div>
        )}
      </section>
      <Dialog.Root
        open={Boolean(dialog)}
        onOpenChange={(open) => {
          if (!open && !submitting) {
            setDialog(null);
            setReviewErrors([]);
            setReviewFailure("");
          }
        }}
      >
        <Dialog.Portal>
          <Dialog.Overlay className="dialog-overlay" />
          <Dialog.Content className="dialog-content join-request-review-dialog">
            <Dialog.Title>
              {dialog?.batch
                ? tr(locale, "join_requests.batch_dialog_title", {
                    count: String(dialog.requests.length),
                  })
                : label(locale, "join_requests.review_dialog_title")}
            </Dialog.Title>
            <Dialog.Description>
              {label(
                locale,
                dialog?.batch
                  ? "confirm.batch_review_join_requests.consequence"
                  : "confirm.review_join_request.consequence",
              )}
            </Dialog.Description>
            <div className="join-request-review-targets">
              <span>{label(locale, "join_requests.review_targets")}</span>
              <ul>
                {dialog?.requests.map((request) => (
                  <li key={request.request_id}>
                    <strong>
                      {request.applicant.display_name ??
                        request.applicant.node_id}
                    </strong>
                    <span>
                      {label(locale, `choice.join_request_review.${dialog.action}`)}
                      {" · "}
                      {label(locale, "join_requests.revision")} {request.revision}
                    </span>
                  </li>
                ))}
              </ul>
            </div>
            <p className="join-request-revision-notice">
              {label(locale, "join_requests.revision_pinned")}
            </p>
            {dialog ? (
              <section
                className="join-request-review-grants"
                aria-label={label(locale, "join_requests.grant_impact")}
              >
                <h3>{label(locale, "join_requests.grant_impact")}</h3>
                {dialog.requests.map((request) => (
                  <div
                    className="join-request-review-grant"
                    data-testid="join-request-review-grant"
                    key={request.request_id}
                  >
                    <strong>
                      {request.applicant.display_name ??
                        request.applicant.node_id}
                    </strong>
                    <JoinRequestRisk
                      request={request}
                      locale={locale}
                      loadedAtUnixMs={loadedAtUnixMs}
                    />
                  </div>
                ))}
              </section>
            ) : null}
            {dialog?.action !== "approve" ? (
              <label className="form-field" htmlFor="join-request-review-note">
                <span id="join-request-review-note-label">
                  {label(locale, "field.join_request_note")}
                </span>
                <textarea
                  id="join-request-review-note"
                  aria-labelledby="join-request-review-note-label"
                  aria-describedby="join-request-review-note-hint"
                  value={note}
                  rows={4}
                  maxLength={500}
                  required={dialog?.action === "request_information"}
                  placeholder={label(locale, "placeholder.join_request_note")}
                  onChange={(event) => setNote(event.target.value)}
                />
                <small id="join-request-review-note-hint">
                  {label(
                    locale,
                    dialog?.action === "request_information"
                      ? "join_requests.note_required"
                      : "join_requests.note_optional",
                  )}
                </small>
              </label>
            ) : null}
            {reviewErrors.length > 0 ? (
              <ul className="form-errors" role="alert">
                {reviewErrors.map((error) => (
                  <li key={error}>{error}</li>
                ))}
              </ul>
            ) : null}
            {reviewFailure ? (
              <p className="form-error" role="alert">
                {reviewFailure}
              </p>
            ) : null}
            <div className="dialog-actions">
              <Dialog.Close asChild>
                <button
                  className="secondary-button"
                  type="button"
                  disabled={submitting}
                >
                  {label(locale, "common.cancel")}
                </button>
              </Dialog.Close>
              <button
                className={
                  dialog?.action === "reject"
                    ? "danger-button"
                    : "primary-button"
                }
                type="button"
                disabled={pending || Boolean(reviewBlockedReason || batchReviewBlockedReason)}
                onClick={() => void submitReview()}
              >
                {submitting
                  ? label(locale, "common.operation_in_progress")
                  : label(locale, "join_requests.confirm_review")}
              </button>
            </div>
            <Dialog.Close asChild>
              <button
                className="icon-button dialog-close"
                type="button"
                aria-label={label(locale, "common.close")}
              >
                <X size={17} />
              </button>
            </Dialog.Close>
          </Dialog.Content>
        </Dialog.Portal>
      </Dialog.Root>
    </>
  );
}

function localExitState(
  snapshot: NodeSnapshot,
  networkId: string,
  networkName: string,
): { supported: boolean; enabled: boolean } {
  const membershipName = snapshot.memberships?.find(
    (membership) => membership.network_id === networkId,
  )?.network_name;
  const resolvedName = membershipName ?? networkName;
  return {
    supported: snapshot.exit_node_capabilities?.supported === true,
    enabled: snapshot.networks.some(
      (network) =>
        network.name === resolvedName && network.roles.includes("exit_node"),
    ),
  };
}

function ActionCard({
  action,
  snapshot,
  selectedNetwork,
  locale,
  pending,
  interactionBlocked,
  requested,
  requestedValues,
  joinAction,
  joinPending,
  previewJoinAction,
  statusInvitationAction,
  revokeInvitationAction,
  redeemInvitationAction,
  redeemInvitationPending,
  previewInvitationAction,
  activePendingAction,
  reviewJoinRequestAction,
  batchReviewJoinRequestsAction,
  previewMemberRemovalAction,
  changeMemberLifecycleAction,
  networkMembersAction,
  onRequestedActionOpened,
  onSubmit,
}: {
  action: NodeUiAction;
  snapshot: NodeSnapshot;
  selectedNetwork: string;
  locale: Locale;
  pending: boolean;
  interactionBlocked: PageStateKind | null;
  requested: boolean;
  requestedValues?: ActionValues;
  joinAction?: NodeUiAction;
  joinPending: boolean;
  previewJoinAction?: NodeUiAction;
  statusInvitationAction?: NodeUiAction;
  revokeInvitationAction?: NodeUiAction;
  redeemInvitationAction?: NodeUiAction;
  redeemInvitationPending: boolean;
  previewInvitationAction?: NodeUiAction;
  activePendingAction: string | null;
  reviewJoinRequestAction?: NodeUiAction;
  batchReviewJoinRequestsAction?: NodeUiAction;
  previewMemberRemovalAction?: NodeUiAction;
  changeMemberLifecycleAction?: NodeUiAction;
  networkMembersAction?: NodeUiAction;
  onRequestedActionOpened(): void;
  onSubmit(
    action: NodeUiAction,
    values: ActionValues,
    onOperation: (operation: NodeOperationSnapshot) => void,
  ): Promise<ActionSubmission>;
}) {
  const [open, setOpen] = useState(requested);
  const [confirming, setConfirming] = useState(false);
  const [values, setValues] = useState<ActionValues>(() => ({
    ...initialActionValues(action, snapshot, selectedNetwork),
    ...requestedValues,
  }));
  const [errors, setErrors] = useState<string[]>([]);
  const [result, setResult] = useState("");
  const [profileOutcome, setProfileOutcome] =
    useState<NetworkProfileOutcome | null>(null);
  const [joinPreview, setJoinPreview] =
    useState<NodeNetworkJoinPreview | null>(null);
  const [joinRequests, setJoinRequests] = useState<{
    result: NodeJoinRequestListResult;
    loadedAtUnixMs: number;
  } | null>(null);
  const [networkMembers, setNetworkMembers] =
    useState<NetworkMemberDirectory | null>(null);
  const [joinConfirmed, setJoinConfirmed] = useState(false);
  const [joinExitConfirmed, setJoinExitConfirmed] = useState(false);
  const [issuedInvitation, setIssuedInvitation] =
    useState<IssuedInvitation | null>(null);
  const [invitations, setInvitations] = useState<{
    items: InvitationStatusView[];
    loadedAtUnixMs: number;
  } | null>(null);
  const [invitationStatus, setInvitationStatus] = useState<{
    status: InvitationStatusView;
    refreshedAtUnixMs: number;
  } | null>(null);
  const [invitationPreview, setInvitationPreview] =
    useState<InvitationPreview | null>(null);
  const [invitationPreviewLoadedAtUnixMs, setInvitationPreviewLoadedAtUnixMs] =
    useState(0);
  const [invitationConfirmed, setInvitationConfirmed] = useState(false);
  const [invitationExitConfirmed, setInvitationExitConfirmed] = useState(false);
  const [failure, setFailure] = useState("");
  const editedFields = useRef(new Set<string>());
  const fingerprint = snapshot.identity?.public_key_fingerprint ?? "";
  const previousFingerprint = useRef(fingerprint);
  const pageBlocked = interactionBlocked
    ? pageStateDefinition(interactionBlocked)
    : null;
  const executionBlocked = executionBlockReason(
    action,
    snapshot,
    selectedNetwork,
  );
  const blocked = Boolean(pageBlocked || executionBlocked);
  const confirmBlockedReason = pending
    ? label(locale, "common.control_pending")
    : pageBlocked
      ? label(locale, pageBlocked.titleKey)
      : executionBlocked
        ? blockReasonText(executionBlocked, locale)
        : "";
  const joinBlockedReason = !joinAction
    ? label(locale, "join.action_unavailable")
    : executionBlockReason(joinAction, snapshot, selectedNetwork)
      ? blockReasonText(
          executionBlockReason(joinAction, snapshot, selectedNetwork),
          locale,
        )
      : "";
  function joinRequestReviewBlockedReason(
    reviewAction?: NodeUiAction,
  ): string {
    if (pageBlocked) return label(locale, pageBlocked.titleKey);
    if (!reviewAction)
      return label(locale, "join_requests.actions_unavailable");
    const reason = executionBlockReason(reviewAction, snapshot, selectedNetwork);
    return reason ? blockReasonText(reason, locale) : "";
  }
  const reviewBlockedReason =
    joinRequestReviewBlockedReason(reviewJoinRequestAction);
  const batchReviewBlockedReason = joinRequestReviewBlockedReason(
    batchReviewJoinRequestsAction,
  );
  function invitationActionBlockedReason(target?: NodeUiAction): string {
    if (pageBlocked) return label(locale, pageBlocked.titleKey);
    if (!target) return label(locale, "invitations.actions_unavailable");
    const reason = executionBlockReason(target, snapshot, selectedNetwork);
    return reason ? blockReasonText(reason, locale) : "";
  }
  const invitationStatusBlockedReason = invitationActionBlockedReason(
    statusInvitationAction,
  );
  const invitationRevokeBlockedReason = invitationActionBlockedReason(
    revokeInvitationAction,
  );
  const invitationRedeemBlockedReason = invitationActionBlockedReason(
    redeemInvitationAction,
  );
  const [previousRequested, setPreviousRequested] = useState(requested);
  const previousSelectedNetwork = useRef(selectedNetwork);

  useEffect(() => {
    if (previousSelectedNetwork.current === selectedNetwork) return;
    previousSelectedNetwork.current = selectedNetwork;
    setConfirming(false);
    setValues(initialActionValues(action, snapshot, selectedNetwork));
    setErrors([]);
    setFailure("");
    setProfileOutcome(null);
    setJoinPreview(null);
    setJoinRequests(null);
    setNetworkMembers(null);
    setJoinConfirmed(false);
    setJoinExitConfirmed(false);
    setIssuedInvitation(null);
    setInvitations(null);
    setInvitationStatus(null);
    setInvitationPreview(null);
    setInvitationPreviewLoadedAtUnixMs(0);
    setInvitationConfirmed(false);
    setInvitationExitConfirmed(false);
  }, [action, selectedNetwork, snapshot]);

  if (requested !== previousRequested) {
    setPreviousRequested(requested);
    if (requested) {
      setOpen(true);
      setConfirming(false);
      setValues({
        ...initialActionValues(action, snapshot, selectedNetwork),
        ...requestedValues,
      });
      setErrors([]);
      setFailure("");
      setResult("");
      setProfileOutcome(null);
      setJoinPreview(null);
      setJoinRequests(null);
      setNetworkMembers(null);
      setJoinConfirmed(false);
      setJoinExitConfirmed(false);
      setIssuedInvitation(null);
      setInvitations(null);
      setInvitationStatus(null);
      setInvitationPreview(null);
      setInvitationPreviewLoadedAtUnixMs(0);
      setInvitationConfirmed(false);
      setInvitationExitConfirmed(false);
    }
  }

  useEffect(() => {
    if (requested) onRequestedActionOpened();
  }, [onRequestedActionOpened, requested]);

  useEffect(() => {
    if (fingerprint === previousFingerprint.current) return;
    previousFingerprint.current = fingerprint;
    if (
      action.fields.some((field) => field.id === "expected_fingerprint") &&
      !editedFields.current.has("expected_fingerprint")
    ) {
      setValues((current) => ({
        ...current,
        expected_fingerprint: fingerprint,
      }));
    }
  }, [action.fields, fingerprint]);

  function applyInvitationOutcome(outcome: NodeCommandOutcome): void {
    if (outcome.status === "invitation_created") {
      setIssuedInvitation(outcome.data);
      setInvitationStatus({
        status: outcome.data.status,
        refreshedAtUnixMs: Date.now(),
      });
      return;
    }
    if (outcome.status === "invitations") {
      setInvitations({
        items: outcome.data,
        loadedAtUnixMs: Date.now(),
      });
      return;
    }
    if (
      outcome.status === "invitation_status" ||
      outcome.status === "invitation_revoked"
    ) {
      setInvitationStatus({
        status: outcome.data,
        refreshedAtUnixMs: Date.now(),
      });
      setInvitations((current) => {
        if (!current) return current;
        const updated = current.items.map((status) =>
          status.invitation.invitation_id === outcome.data.invitation.invitation_id
            ? outcome.data
            : status,
        );
        return { ...current, items: updated };
      });
      return;
    }
    if (outcome.status === "invitation_preview") {
      setInvitationPreview(outcome.data);
      setInvitationPreviewLoadedAtUnixMs(Date.now());
      setInvitationConfirmed(false);
      setInvitationExitConfirmed(false);
      return;
    }
    if (outcome.status === "invitation_redeemed") {
      setInvitationPreview(null);
      setInvitationPreviewLoadedAtUnixMs(0);
      setInvitationConfirmed(false);
      setInvitationExitConfirmed(false);
    }
  }

  async function submit(): Promise<void> {
    const validation = validateActionValues(action, values, locale);
    if (validation.length > 0) {
      setErrors(validation);
      return;
    }
    setErrors([]);
    setFailure("");
    setResult("");
    setProfileOutcome(null);
    try {
      const submission = await onSubmit(action, values, () => {});
      if (submission.outcome?.status === "rejected") {
        setFailure(
          action.id === "list_network_members" &&
            submission.outcome.data.code === "permission_denied"
            ? label(locale, "member_directory.error.permission_denied")
            : outcomeSummary(submission.outcome, locale),
        );
      } else if (submission.outcome) {
        setResult(
          invitationOutcomeSummary(submission.outcome, locale) ??
            outcomeSummary(submission.outcome, locale),
        );
        applyInvitationOutcome(submission.outcome);
        if (
          submission.outcome.status === "network_profile_preview" ||
          submission.outcome.status === "network_profile_updated"
        ) {
          setProfileOutcome(submission.outcome);
        }
        if (submission.outcome.status === "network_join_preview") {
          setJoinPreview(submission.outcome.data);
          setJoinConfirmed(false);
          setJoinExitConfirmed(false);
          setOpen(true);
        } else if (submission.outcome.status === "join_requests") {
          setJoinRequests({
            result: submission.outcome.data,
            loadedAtUnixMs: Date.now(),
          });
          setNetworkMembers(null);
          setOpen(true);
        } else if (submission.outcome.status === "network_members") {
          setNetworkMembers(submission.outcome.data);
          setJoinRequests(null);
          setOpen(true);
        } else if (
          !submission.outcome.status.startsWith("invitation_") &&
          submission.outcome.status !== "invitations" &&
          submission.outcome.status !== "member_removal_preview" &&
          submission.outcome.status !== "member_lifecycle_changed"
        ) {
          if (
            submission.outcome.status === "network_joined" ||
            submission.outcome.status === "network_left"
          ) {
            setJoinPreview(null);
            setJoinConfirmed(false);
            setJoinExitConfirmed(false);
          }
          setOpen(false);
        }
      } else if (submission.operation) {
        setFailure(operationFailureSummary(submission.operation, locale));
      } else {
        setFailure(label(locale, "common.operation_failed"));
      }
    } catch (error) {
      setFailure(error instanceof Error ? error.message : String(error));
    }
  }

  async function confirmJoin(): Promise<void> {
    if (
      !joinPreview ||
      !joinAction ||
      !joinConfirmed ||
      (joinPreview.join_requires_exit_consent && !joinExitConfirmed)
    ) {
      return;
    }
    setErrors([]);
    setFailure("");
    setResult("");
    try {
      const submission = await onSubmit(
        joinAction,
        {
          ...joinPreviewValues(
            joinPreview,
            joinPreview.join_requires_exit_consent && joinExitConfirmed,
          ),
          acknowledged_at_unix_ms: String(Date.now()),
        },
        () => {},
      );
      if (submission.outcome?.status === "rejected") {
        setFailure(outcomeSummary(submission.outcome, locale));
      } else if (submission.outcome) {
        setResult(outcomeSummary(submission.outcome, locale));
        if (submission.outcome.status === "network_joined") {
          setJoinPreview(null);
          setJoinConfirmed(false);
          setJoinExitConfirmed(false);
          setOpen(false);
        }
      } else if (submission.operation) {
        setFailure(operationFailureSummary(submission.operation, locale));
      } else {
        setFailure(label(locale, "common.operation_failed"));
      }
    } catch (error) {
      setFailure(error instanceof Error ? error.message : String(error));
    }
  }

  async function previewJoinAgain(): Promise<void> {
    if (!joinPreview || !previewJoinAction) {
      setFailure(label(locale, "join.action_unavailable"));
      return;
    }
    setErrors([]);
    setFailure("");
    setResult("");
    try {
      const submission = await onSubmit(
        previewJoinAction,
        { network_id: joinPreview.network_id },
        () => {},
      );
      if (submission.outcome?.status === "rejected") {
        setFailure(outcomeSummary(submission.outcome, locale));
      } else if (submission.outcome?.status === "network_join_preview") {
        setJoinPreview(submission.outcome.data);
        setJoinConfirmed(false);
        setJoinExitConfirmed(false);
        setOpen(true);
      } else if (submission.operation) {
        setFailure(operationFailureSummary(submission.operation, locale));
      } else {
        setFailure(label(locale, "common.operation_failed"));
      }
    } catch (error) {
      setFailure(error instanceof Error ? error.message : String(error));
    }
  }

  async function confirmInvitation(confirmValues: ActionValues): Promise<void> {
    const invitationExitMode = invitationPreview
      ? normalizeExitContributionMode(
          invitationPreview.terms.exit_contribution_mode,
          invitationPreview.terms.exit_contribution,
        )
      : "disabled";
    if (
      !invitationPreview ||
      !redeemInvitationAction ||
      !invitationConfirmed ||
      (invitationExitMode === "required_on_join" && !invitationExitConfirmed) ||
      invitationRedeemBlockedReason ||
      redeemInvitationPending
    ) {
      return;
    }
    setErrors([]);
    setFailure("");
    setResult("");
    try {
      const submission = await onSubmit(
        redeemInvitationAction,
        confirmValues,
        () => {},
      );
      if (submission.outcome?.status === "rejected") {
        setFailure(outcomeSummary(submission.outcome, locale));
      } else if (submission.outcome) {
        setResult(
          invitationOutcomeSummary(submission.outcome, locale) ??
            outcomeSummary(submission.outcome, locale),
        );
        applyInvitationOutcome(submission.outcome);
        if (submission.outcome.status === "invitation_redeemed") {
          setOpen(false);
        }
      } else if (submission.operation) {
        setFailure(operationFailureSummary(submission.operation, locale));
      } else {
        setFailure(label(locale, "common.operation_failed"));
      }
    } catch (error) {
      setFailure(error instanceof Error ? error.message : String(error));
    }
  }

  async function retryInvitationPreview(): Promise<void> {
    if (!previewInvitationAction) {
      setFailure(label(locale, "invitations.actions_unavailable"));
      return;
    }
    setErrors([]);
    setFailure("");
    setResult("");
    try {
      const submission = await onSubmit(previewInvitationAction, values, () => {});
      if (submission.outcome?.status === "rejected") {
        setFailure(outcomeSummary(submission.outcome, locale));
      } else if (submission.outcome?.status === "invitation_preview") {
        applyInvitationOutcome(submission.outcome);
      } else if (submission.operation) {
        setFailure(operationFailureSummary(submission.operation, locale));
      } else {
        setFailure(label(locale, "common.operation_failed"));
      }
    } catch (error) {
      setFailure(error instanceof Error ? error.message : String(error));
    }
  }

  function returnToInvitationEdit(): void {
    setInvitationPreview(null);
    setInvitationPreviewLoadedAtUnixMs(0);
    setInvitationConfirmed(false);
    setInvitationExitConfirmed(false);
    setFailure("");
    setResult("");
    setOpen(true);
  }

  function handleJoinRequestsReviewed(ack: NodeJoinRequestReviewAck): void {
    setJoinRequests((current) => {
      if (!current) return current;
      const reviewed = new Map(
        ack.reviews.map((review) => [review.request_id, review]),
      );
      return {
        ...current,
        result: {
          ...current.result,
          requests: current.result.requests.map((request) => {
            const result = reviewed.get(request.request_id);
            return result
              ? {
                  ...request,
                  state: result.state,
                  revision: result.revision,
                }
              : request;
          }),
        },
      };
    });
  }

  function handleLifecycleChanged(change: MemberLifecycleChange): void {
    if (!networkMembersAction) return;
    void onSubmit(
      networkMembersAction,
      { network_id: change.network_id },
      () => {},
    )
      .then((submission) => {
        if (submission.outcome?.status === "network_members") {
          setNetworkMembers(submission.outcome.data);
          setResult(outcomeSummary(submission.outcome, locale));
        }
      })
      .catch(() => {
        // The next directory refresh keeps server truth; a failed refresh must
        // not close or corrupt the governance result already shown.
      });
  }

  function handleSubmit(event: FormEvent): void {
    event.preventDefault();
    if (blocked || pending) return;
    if (action.confirmation) {
      setConfirming(true);
      return;
    }
    void submit();
  }

  return (
    <article className="action-card">
      <button
        type="button"
        className="action-card-head"
        aria-expanded={open}
        onClick={() => {
          if (!open) {
            setJoinRequests(null);
            setNetworkMembers(null);
          }
          setOpen((current) => !current);
        }}
      >
        <span>{label(locale, action.label_key)}</span>
        <ChevronRight
          className={open ? "action-chevron open" : "action-chevron"}
          size={16}
        />
      </button>
      {result ? (
        <p className="form-success" role="status">
          {result}
        </p>
      ) : null}
      {profileOutcome ? (
        <NetworkProfileResultCard outcome={profileOutcome} locale={locale} />
      ) : null}
      {joinPreview ? (
        <JoinPreviewCard
          preview={joinPreview}
          locale={locale}
          checked={joinConfirmed}
          exitContributionChecked={joinExitConfirmed}
          pending={joinPending}
          blockedReason={joinBlockedReason}
          onCheckedChange={setJoinConfirmed}
          onExitContributionCheckedChange={setJoinExitConfirmed}
          onPreviewAgain={() => void previewJoinAgain()}
          onConfirm={() => void confirmJoin()}
        />
      ) : null}
      {networkMembers && action.id === "list_network_members" ? (
        <MemberDirectoryCard
          directory={networkMembers}
          locale={locale}
          previewRemovalAction={previewMemberRemovalAction}
          changeLifecycleAction={changeMemberLifecycleAction}
          activePendingAction={activePendingAction}
          onSubmit={onSubmit}
          onLifecycleChanged={handleLifecycleChanged}
        />
      ) : null}
      {joinRequests ? (
        <JoinRequestListCard
          key={`${joinRequests.result.network_id}:${joinRequests.loadedAtUnixMs}`}
          result={joinRequests.result}
          locale={locale}
          loadedAtUnixMs={joinRequests.loadedAtUnixMs}
          reviewAction={reviewJoinRequestAction}
          batchReviewAction={batchReviewJoinRequestsAction}
          reviewBlockedReason={reviewBlockedReason}
          batchReviewBlockedReason={batchReviewBlockedReason}
          activePendingAction={activePendingAction}
          onSubmit={onSubmit}
          onReviewed={handleJoinRequestsReviewed}
        />
      ) : null}
      {issuedInvitation && action.id === "create_invitation" ? (
        <InvitationCodeCard issued={issuedInvitation} locale={locale} />
      ) : null}
      {invitations && action.id === "list_invitations" ? (
        <InvitationListCard
          invitations={invitations.items}
          loadedAtUnixMs={invitations.loadedAtUnixMs}
          locale={locale}
          statusAction={statusInvitationAction}
          revokeAction={revokeInvitationAction}
          statusBlockedReason={invitationStatusBlockedReason}
          revokeBlockedReason={invitationRevokeBlockedReason}
          pendingAction={activePendingAction}
          onSubmit={onSubmit}
          onOutcome={applyInvitationOutcome}
        />
      ) : null}
      {invitationStatus &&
      (action.id === "get_invitation_status" ||
        action.id === "revoke_invitation") ? (
        <InvitationStatusCard
          status={invitationStatus.status}
          refreshedAtUnixMs={invitationStatus.refreshedAtUnixMs}
          locale={locale}
        />
      ) : null}
      {invitationPreview && action.id === "preview_invitation" ? (
        <InvitationPreviewCard
          preview={invitationPreview}
          locale={locale}
          checked={invitationConfirmed}
          exitContributionChecked={invitationExitConfirmed}
          localExitAvailable={
            localExitState(
              snapshot,
              invitationPreview.network_id,
              invitationPreview.network_name,
            ).supported
          }
          localExitEnabled={
            localExitState(
              snapshot,
              invitationPreview.network_id,
              invitationPreview.network_name,
            ).enabled
          }
          pending={redeemInvitationPending}
          blockedReason={invitationRedeemBlockedReason}
          sourceValues={values}
          previewLoadedAtUnixMs={invitationPreviewLoadedAtUnixMs}
          onCheckedChange={setInvitationConfirmed}
          onExitContributionCheckedChange={setInvitationExitConfirmed}
          onRetryPreview={() => void retryInvitationPreview()}
          onBackToEdit={returnToInvitationEdit}
          onConfirm={(confirmValues) => void confirmInvitation(confirmValues)}
        />
      ) : null}
      {open && !invitationPreview ? (
        <form className="action-form" onSubmit={handleSubmit}>
          {pageBlocked || executionBlocked ? (
            <div className="inline-notice warning">
              <ShieldAlert size={15} />
              <span>
                {pageBlocked
                  ? label(locale, pageBlocked.titleKey)
                  : blockReasonText(executionBlocked, locale)}
              </span>
            </div>
          ) : null}
          {action.fields.map((field) => (
            <FieldControl
              key={field.id}
              field={field}
              locale={locale}
              snapshot={snapshot}
              value={values[field.id] ?? ""}
              onChange={(value) => {
                editedFields.current.add(field.id);
                setValues((current) => ({ ...current, [field.id]: value }));
              }}
            />
          ))}
          {errors.length > 0 ? (
            <ul className="form-errors" role="alert">
              {errors.map((error) => (
                <li key={error}>{error}</li>
              ))}
            </ul>
          ) : null}
          {failure ? (
            <p className="form-error" role="alert">
              {failure}
            </p>
          ) : null}
          {pending ? (
            <small className="control-reason" role="status">
              {label(locale, "common.control_pending")}
            </small>
          ) : null}
          <button
            className="primary-button"
            type="submit"
            disabled={blocked || pending}
          >
            {pending
              ? label(locale, "common.operation_in_progress")
              : label(locale, "common.execute")}
          </button>
        </form>
      ) : null}

      <Dialog.Root open={confirming} onOpenChange={setConfirming}>
        <Dialog.Portal>
          <Dialog.Overlay className="dialog-overlay" />
          <Dialog.Content className="dialog-content confirm-dialog">
            <div className="dialog-heading">
              <div
                className={
                  action.confirmation?.destructive
                    ? "dialog-icon danger"
                    : "dialog-icon"
                }
              >
                <AlertTriangle size={20} />
              </div>
              <div>
                <Dialog.Title>
                  {label(
                    locale,
                    action.confirmation?.title_key ?? "common.confirm_again",
                  )}
                </Dialog.Title>
                <Dialog.Description>
                  {label(
                    locale,
                    action.confirmation?.consequence_key ??
                      "common.ready_to_submit",
                  )}
                </Dialog.Description>
                {action.confirmation?.recent_authentication_required ? (
                  <p className="dialog-secondary-copy">
                    {label(locale, "confirm.reauthentication_required")}
                  </p>
                ) : null}
              </div>
            </div>
            {confirmBlockedReason ? (
              <small className="control-reason" role="status">
                {confirmBlockedReason}
              </small>
            ) : null}
            <div className="dialog-actions">
              <Dialog.Close asChild>
                <button className="secondary-button" type="button">
                  {label(locale, "common.cancel")}
                </button>
              </Dialog.Close>
              <button
                className={
                  action.confirmation?.destructive
                    ? "danger-button"
                    : "primary-button"
                }
                type="button"
                disabled={Boolean(confirmBlockedReason)}
                onClick={() => {
                  setConfirming(false);
                  void submit();
                }}
              >
                {label(locale, "common.execute")}
              </button>
            </div>
            <Dialog.Close asChild>
              <button
                className="icon-button dialog-close"
                type="button"
                aria-label={label(locale, "common.close")}
              >
                <X size={17} />
              </button>
            </Dialog.Close>
          </Dialog.Content>
        </Dialog.Portal>
      </Dialog.Root>
    </article>
  );
}

export function ActionPanel(props: ActionPanelProps) {
  const { actions, locale, pendingAction, onSubmit } = props;
  const hasIntegratedJoinReview = actions.some(
    (action) => action.id === "list_join_requests",
  );
  const renderedActions = hasIntegratedJoinReview
    ? actions.filter(
        (action) =>
          action.id !== "review_join_request" &&
          action.id !== "batch_review_join_requests",
      )
    : actions;
  if (renderedActions.length === 0) {
    return (
      <aside
        className="actions-panel"
        data-ui-region="action-panel"
        aria-label={label(locale, "common.shared_actions")}
      >
        <div className="panel-title">
          {label(locale, "common.shared_actions")}
        </div>
        <p className="empty-copy">
          {label(locale, "common.page_actions_empty")}
        </p>
        {props.operation ? (
          <OperationStatusCard
            operation={props.operation}
            locale={locale}
            onCancel={props.onCancel}
            onRetry={props.onRetryOperation}
          />
        ) : null}
      </aside>
    );
  }
  return (
    <aside
      className="actions-panel"
      data-ui-region="action-panel"
      aria-label={label(locale, "common.shared_actions")}
    >
      <div className="panel-title">
        {label(locale, "common.shared_actions")}
      </div>
      {props.operation ? (
        <OperationStatusCard
          operation={props.operation}
          locale={locale}
          onCancel={props.onCancel}
          onRetry={props.onRetryOperation}
        />
      ) : null}
      <div className="action-list">
        {renderedActions.map((action) => (
          <ActionCard
            key={action.id}
            action={action}
            snapshot={props.snapshot}
            selectedNetwork={props.selectedNetwork}
            locale={locale}
            pending={pendingAction === action.id}
            interactionBlocked={props.interactionBlocked}
            requested={props.requestedAction?.actionId === action.id}
            requestedValues={
              props.requestedAction?.actionId === action.id
                ? props.requestedAction.values
                : undefined
            }
            joinAction={actions.find(
              (candidate) => candidate.id === "join_network",
            )}
            joinPending={pendingAction === "join_network"}
            previewJoinAction={actions.find(
              (candidate) => candidate.id === "preview_join_network",
            )}
            statusInvitationAction={actions.find(
              (candidate) => candidate.id === "get_invitation_status",
            )}
            revokeInvitationAction={actions.find(
              (candidate) => candidate.id === "revoke_invitation",
            )}
            redeemInvitationAction={actions.find(
              (candidate) => candidate.id === "redeem_invitation",
            )}
            redeemInvitationPending={pendingAction === "redeem_invitation"}
            previewInvitationAction={actions.find(
              (candidate) => candidate.id === "preview_invitation",
            )}
            activePendingAction={pendingAction}
            reviewJoinRequestAction={actions.find(
              (candidate) => candidate.id === "review_join_request",
            )}
            batchReviewJoinRequestsAction={actions.find(
              (candidate) => candidate.id === "batch_review_join_requests",
            )}
            previewMemberRemovalAction={actions.find(
              (candidate) => candidate.id === "preview_member_removal",
            )}
            changeMemberLifecycleAction={actions.find(
              (candidate) => candidate.id === "change_member_lifecycle",
            )}
            networkMembersAction={actions.find(
              (candidate) => candidate.id === "list_network_members",
            )}
            onRequestedActionOpened={props.onRequestedActionOpened}
            onSubmit={onSubmit}
          />
        ))}
      </div>
    </aside>
  );
}
