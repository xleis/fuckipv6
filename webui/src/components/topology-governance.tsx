import { AlertTriangle, Check, RefreshCw, ShieldCheck } from "lucide-react";
import { useMemo, useState, type JSX } from "react";
import type {
  MemberLifecycleAction,
  MemberLifecycleChange,
  MemberRemovalPreview,
  NodeOperationSnapshot,
  NodeUiAction,
  TopologyBoundary,
  TopologySnapshot,
} from "../../../contracts/typescript/node-contract-v1";
import {
  label,
  outcomeSummary,
  validateActionValues,
  type ActionValues,
} from "../contract/invariants";
import { tr } from "../i18n";
import {
  buildBatchMemberPlan,
  planForBatchAction,
  type BatchCandidate,
  type BatchMemberAction,
  type BatchSkipReason,
  type DirectoryLoad,
  type TopologyActionSubmit,
} from "../topology-actions";

export type { DirectoryLoad, TopologyActionSubmit } from "../topology-actions";

type BatchResult = {
  nodeId: string;
  name: string;
  status: "success" | "skipped" | "failed";
  reason: string;
  auditId?: string;
};

type Props = {
  topology: TopologySnapshot;
  locale: Locale;
  networks: TopologyBoundary[];
  networkId: string | null;
  onNetworkIdChange(networkId: string): void;
  selected: ReadonlySet<string>;
  directoryLoad: DirectoryLoad;
  lifecycleAction: NodeUiAction | undefined;
  previewAction: NodeUiAction | undefined;
  requestedAction: BatchMemberAction | null;
  onActionChange(action: BatchMemberAction): void;
  onSubmit: TopologyActionSubmit | undefined;
  onRefreshDirectory(): void;
  onSelectedChange(next: Set<string>): void;
};

const DANGEROUS_ACTIONS = new Set<MemberLifecycleAction>([
  "reject",
  "suspend",
  "remove",
]);

function skipReasonLabel(locale: Locale, reason: BatchSkipReason | undefined): string {
  switch (reason) {
    case "member_missing":
      return label(locale, "topology.batch.skip.member_missing");
    case "not_manageable":
      return label(locale, "topology.batch.skip.not_manageable");
    case "action_unavailable":
    default:
      return label(locale, "topology.batch.skip.action_unavailable");
  }
}

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

function resultSummary(results: BatchResult[], locale: Locale): string {
  const success = results.filter((result) => result.status === "success").length;
  const skipped = results.filter((result) => result.status === "skipped").length;
  const failed = results.filter((result) => result.status === "failed").length;
  return tr(locale, "topology.batch.result.summary", {
    success: String(success),
    skipped: String(skipped),
    failed: String(failed),
  });
}

function skipCandidates(
  candidates: BatchCandidate[],
  locale: Locale,
): BatchResult[] {
  return candidates.map((candidate) => ({
    nodeId: candidate.nodeId,
    name: candidate.name,
    status: "skipped",
    reason: `${skipReasonLabel(locale, candidate.skipReason)} · ${label(
      locale,
      "topology.batch.result.not_submitted",
    )}`,
  }));
}

function removalBlockedReason(
  preview: MemberRemovalPreview,
  locale: Locale,
): string {
  const impacts = preview.impact_keys.map((key) => label(locale, key));
  return [
    label(locale, "topology.batch.remove_blocked"),
    ...impacts,
  ].join(" · ");
}

function errorText(error: unknown, locale: Locale): string {
  if (error instanceof Error && error.message) return error.message;
  return label(locale, "common.operation_failed");
}

function BatchResults({
  results,
  locale,
  onFinish,
}: {
  results: BatchResult[];
  locale: Locale;
  onFinish(): void;
}): JSX.Element {
  return (
    <section
      className="topology-governance topology-batch-results-panel"
      aria-labelledby="topology-batch-results-title"
      data-testid="topology-batch-results"
    >
      <div className="topology-governance-heading">
        <div>
          <strong id="topology-batch-results-title">
            {label(locale, "topology.batch.results_title")}
          </strong>
          <p>{resultSummary(results, locale)}</p>
        </div>
        <button type="button" className="primary-button" onClick={onFinish}>
          <Check size={15} />
          {label(locale, "topology.batch.finish")}
        </button>
      </div>
      <ul className="topology-batch-result-list" aria-live="polite">
        {results.map((result) => (
          <li
            key={result.nodeId}
            className={result.status}
            data-testid={`topology-batch-result-${result.nodeId}`}
          >
            <div>
              <strong>{result.name}</strong>
              <span className="topology-batch-result-status">
                {label(locale, `topology.batch.result.${result.status}`)}
              </span>
            </div>
            <p>{result.reason}</p>
            {result.auditId ? (
              <p className="topology-batch-audit">
                {label(locale, "topology.batch.audit_id")}: <code>{result.auditId}</code>
              </p>
            ) : null}
          </li>
        ))}
      </ul>
    </section>
  );
}

export function TopologyGovernancePanel({
  topology,
  locale,
  networks,
  networkId,
  onNetworkIdChange,
  selected,
  directoryLoad,
  lifecycleAction,
  previewAction,
  requestedAction,
  onActionChange,
  onSubmit,
  onRefreshDirectory,
  onSelectedChange,
}: Props): JSX.Element | null {
  const [previewOpen, setPreviewOpen] = useState(false);
  const [reason, setReason] = useState("");
  const [busy, setBusy] = useState(false);
  const [results, setResults] = useState<BatchResult[]>([]);
  const directory =
    directoryLoad.state === "ready" &&
    directoryLoad.networkId === networkId
      ? directoryLoad.directory
      : null;
  const basePlan = useMemo(
    () => buildBatchMemberPlan(topology, selected, networkId, directory),
    [topology, selected, networkId, directory],
  );
  const options = basePlan.actionOptions;
  const action =
    requestedAction && options.some((option) => option.action === requestedAction)
      ? requestedAction
      : (options[0]?.action ?? null);
  const selectedOption = options.find((option) => option.action === action);
  const plan = useMemo(
    () => planForBatchAction(basePlan, action),
    [basePlan, action],
  );
  const stale =
    directoryLoad.state === "ready" &&
    directoryLoad.networkId === networkId &&
    directoryLoad.topologyRevision !== topology.revision;

  function finishResults(): void {
    setResults([]);
    setPreviewOpen(false);
    setReason("");
    onSelectedChange(new Set());
    onRefreshDirectory();
  }

  function appendResult(result: BatchResult): void {
    setResults((current) => [...current, result]);
  }

  async function executeBatch(): Promise<void> {
    if (
      !lifecycleAction ||
      !onSubmit ||
      !networkId ||
      !directory ||
      !action ||
      busy ||
      stale
    ) {
      return;
    }
    setBusy(true);
    setResults(skipCandidates(plan.skipped, locale));
    const successes: string[] = [];
    try {
      for (const candidate of plan.eligible) {
        const member = candidate.member;
        if (!member) {
          appendResult({
            nodeId: candidate.nodeId,
            name: candidate.name,
            status: "skipped",
            reason: `${skipReasonLabel(locale, "member_missing")} · ${label(
              locale,
              "topology.batch.result.not_submitted",
            )}`,
          });
          continue;
        }

        let expectedRevision = member.revision;
        if (action === "remove") {
          if (!previewAction) {
            appendResult({
              nodeId: candidate.nodeId,
              name: candidate.name,
              status: "failed",
              reason: label(locale, "topology.batch.preview_unavailable"),
            });
            continue;
          }
          try {
            const previewSubmission = await onSubmit(
              previewAction,
              {
                network_id: networkId,
                target_node_id: candidate.nodeId,
              },
              () => {},
            );
            if (
              previewSubmission.outcome?.status === "member_removal_preview"
            ) {
              const preview = previewSubmission.outcome.data;
              if (!preview.can_remove) {
                appendResult({
                  nodeId: candidate.nodeId,
                  name: candidate.name,
                  status: "skipped",
                  reason: removalBlockedReason(preview, locale),
                });
                continue;
              }
              expectedRevision = preview.revision;
            } else if (previewSubmission.outcome?.status === "rejected") {
              appendResult({
                nodeId: candidate.nodeId,
                name: candidate.name,
                status: "failed",
                reason: outcomeSummary(previewSubmission.outcome, locale),
              });
              continue;
            } else if (previewSubmission.operation) {
              appendResult({
                nodeId: candidate.nodeId,
                name: candidate.name,
                status: "failed",
                reason: operationFailureText(previewSubmission.operation, locale),
              });
              continue;
            } else {
              appendResult({
                nodeId: candidate.nodeId,
                name: candidate.name,
                status: "failed",
                reason: label(locale, "topology.batch.preview_unavailable"),
              });
              continue;
            }
          } catch (error) {
            appendResult({
              nodeId: candidate.nodeId,
              name: candidate.name,
              status: "failed",
              reason: errorText(error, locale),
            });
            continue;
          }
        }

        const values: ActionValues = {
          network_id: networkId,
          target_node_id: candidate.nodeId,
          action,
          role: member.role,
          expected_revision: expectedRevision.toString(),
          ...(reason.trim() ? { reason: reason.trim() } : {}),
        };
        const validation = validateActionValues(lifecycleAction, values, locale);
        if (validation.length > 0) {
          appendResult({
            nodeId: candidate.nodeId,
            name: candidate.name,
            status: "failed",
            reason: validation.join(" · "),
          });
          continue;
        }

        try {
          const submission = await onSubmit(lifecycleAction, values, () => {});
          if (submission.outcome?.status === "member_lifecycle_changed") {
            const change: MemberLifecycleChange = submission.outcome.data;
            successes.push(candidate.nodeId);
            appendResult({
              nodeId: candidate.nodeId,
              name: candidate.name,
              status: "success",
              reason: tr(locale, "topology.batch.result.changed", {
                revision: String(change.revision),
              }),
              auditId: change.audit_id,
            });
          } else if (submission.outcome?.status === "rejected") {
            appendResult({
              nodeId: candidate.nodeId,
              name: candidate.name,
              status: "failed",
              reason: outcomeSummary(submission.outcome, locale),
            });
          } else if (submission.operation) {
            const succeeded = submission.operation.state === "succeeded";
            if (succeeded) successes.push(candidate.nodeId);
            appendResult({
              nodeId: candidate.nodeId,
              name: candidate.name,
              status: succeeded ? "success" : "failed",
              reason: succeeded
                ? label(locale, "topology.batch.result.command_succeeded")
                : operationFailureText(submission.operation, locale),
            });
          } else {
            appendResult({
              nodeId: candidate.nodeId,
              name: candidate.name,
              status: "failed",
              reason: label(locale, "common.operation_failed"),
            });
          }
        } catch (error) {
          appendResult({
            nodeId: candidate.nodeId,
            name: candidate.name,
            status: "failed",
            reason: errorText(error, locale),
          });
        }
      }
    } finally {
      if (successes.length > 0) {
        const successSet = new Set(successes);
        onSelectedChange(new Set([...selected].filter((id) => !successSet.has(id))));
        onRefreshDirectory();
      }
      setBusy(false);
    }
  }

  if (!busy && results.length > 0) {
    return (
      <BatchResults
        results={results}
        locale={locale}
        onFinish={finishResults}
      />
    );
  }

  if (selected.size === 0) {
    return (
      <section
        id="topology-governance"
        className="topology-governance"
        aria-live="polite"
      >
        <strong>{label(locale, "topology.batch.empty_selection")}</strong>
        <p>{label(locale, "topology.batch.empty_selection_hint")}</p>
      </section>
    );
  }
  if (
    !networkId ||
    !lifecycleAction ||
    !onSubmit ||
    directoryLoad.state !== "ready" ||
    directoryLoad.networkId !== networkId
  ) {
    if (directoryLoad.state === "loading") {
      return (
        <section id="topology-governance" className="topology-governance" role="status">
          <strong>{label(locale, "topology.batch.directory_loading")}</strong>
        </section>
      );
    }
    if (directoryLoad.state === "error") {
      return (
        <section
          id="topology-governance"
          className="topology-governance topology-governance-error"
          role="alert"
        >
          <strong>{label(locale, "topology.batch.directory_failed")}</strong>
          <p>{directoryLoad.message}</p>
          <button type="button" className="secondary-button" onClick={onRefreshDirectory}>
            <RefreshCw size={14} />
            {label(locale, "common.retry")}
          </button>
        </section>
      );
    }
    return (
      <section
        id="topology-governance"
        className="topology-governance topology-governance-error"
        role="alert"
      >
        <strong>{label(locale, "topology.batch.unavailable")}</strong>
        <p>{label(locale, "topology.batch.unavailable_hint")}</p>
      </section>
    );
  }

  const visibleSelectedCount = basePlan.candidates.length;
  const skippedByReason = new Map<BatchSkipReason | "none", number>();
  for (const candidate of plan.skipped) {
    const key = candidate.skipReason ?? "none";
    skippedByReason.set(key, (skippedByReason.get(key) ?? 0) + 1);
  }
  const canExecute = !stale && Boolean(selectedOption) && plan.eligible.length > 0;

  return (
    <section
      id="topology-governance"
      className="topology-governance"
      aria-labelledby="topology-governance-title"
      data-testid="topology-governance"
    >
      <div className="topology-governance-heading">
        <div>
          <strong id="topology-governance-title">
            {label(locale, "topology.batch.title")}
          </strong>
          <p>
            {tr(locale, "topology.batch.selection_count", {
              selected: String(selected.size),
              visible: String(visibleSelectedCount),
            })}
          </p>
        </div>
        <div className="topology-batch-heading-actions">
          <label>
            <span>{label(locale, "topology.batch.network")}</span>
            <select
              value={networkId}
              disabled={busy}
              onChange={(event) => onNetworkIdChange(event.currentTarget.value)}
            >
              {networks.map((network) => (
                <option key={network.network_id} value={network.network_id}>
                  {network.name}
                </option>
              ))}
            </select>
          </label>
          <button
            type="button"
            className="secondary-button"
            disabled={busy}
            onClick={onRefreshDirectory}
          >
            <RefreshCw size={14} />
            {label(locale, "topology.batch.refresh_directory")}
          </button>
        </div>
      </div>
      {stale ? (
        <p className="topology-governance-stale" role="status">
          {label(locale, "topology.batch.directory_stale")}
        </p>
      ) : null}
      <div className="topology-batch-controls">
        <label>
          <span>{label(locale, "topology.batch.action")}</span>
          <select
            value={selectedOption?.action ?? ""}
            disabled={busy || options.length === 0}
            onChange={(event) => {
              onActionChange(event.currentTarget.value as BatchMemberAction);
              setResults([]);
            }}
          >
            {options.length === 0 ? (
              <option value="">{label(locale, "topology.batch.no_actions")}</option>
            ) : null}
            {options.map((option) => (
              <option key={option.action} value={option.action}>
                {tr(locale, "topology.batch.action_option", {
                  action: label(locale, `choice.member_lifecycle.${option.action}`),
                  eligible: String(option.eligibleCount),
                  total: String(option.totalCount),
                })}
              </option>
            ))}
          </select>
        </label>
        <div className="topology-batch-counts">
          <span className="good">
            {tr(locale, "topology.batch.eligible", {
              count: String(plan.eligible.length),
            })}
          </span>
          <span className="warn">
            {tr(locale, "topology.batch.skipped", {
              count: String(plan.skipped.length),
            })}
          </span>
        </div>
        <button
          type="button"
          className="primary-button"
          disabled={busy || !canExecute}
          onClick={() => {
            setResults([]);
            setPreviewOpen(true);
          }}
        >
          {label(locale, "topology.batch.preview")}
        </button>
      </div>
      <p className="topology-batch-policy">
        {label(locale, "topology.batch.failure_policy")}
      </p>
      {plan.skipped.length > 0 ? (
        <details className="topology-batch-skip-details">
          <summary>
            {tr(locale, "topology.batch.permission_differences", {
              count: String(plan.skipped.length),
            })}
          </summary>
          <ul>
            {plan.skipped.map((candidate) => (
              <li key={candidate.nodeId}>
                <strong>{candidate.name}</strong>
                <span>{skipReasonLabel(locale, candidate.skipReason)}</span>
              </li>
            ))}
          </ul>
        </details>
      ) : null}
      {previewOpen ? (
        <section
          className="topology-batch-dialog"
          role="dialog"
          aria-modal="true"
          aria-label={label(locale, "topology.batch.confirm_title")}
          data-testid="topology-batch-dialog"
        >
          <h3>{label(locale, "topology.batch.confirm_title")}</h3>
          <p>
            {tr(locale, "topology.batch.confirm_counts", {
              selected: String(selected.size),
              eligible: String(plan.eligible.length),
              skipped: String(plan.skipped.length),
            })}
          </p>
          <p>{label(locale, "topology.batch.failure_policy")}</p>
          {action && DANGEROUS_ACTIONS.has(action) ? (
            <p className="topology-batch-danger">
              <AlertTriangle size={16} />
              {label(locale, `confirm.member_lifecycle.${action}`)}
            </p>
          ) : null}
          <p className="topology-batch-reauth">
            <ShieldCheck size={16} />
            {label(locale, "topology.batch.reauthentication")}
          </p>
          {skippedByReason.size > 0 ? (
            <ul className="topology-batch-reasons">
              {[...skippedByReason].map(([reasonKey, count]) => (
                <li key={reasonKey}>
                  {skipReasonLabel(
                    locale,
                    reasonKey === "none" ? undefined : reasonKey,
                  )}
                  : {count}
                </li>
              ))}
            </ul>
          ) : null}
          <label className="form-field">
            <span>{label(locale, "field.lifecycle_reason")}</span>
            <input
              type="text"
              maxLength={512}
              value={reason}
              autoComplete="off"
              disabled={busy || results.length > 0}
              onChange={(event) => setReason(event.currentTarget.value)}
            />
          </label>
          {results.length > 0 ? (
            <section className="topology-batch-results" aria-live="polite">
              <h4>{resultSummary(results, locale)}</h4>
              <ul>
                {results.map((result) => (
                  <li key={result.nodeId} className={result.status}>
                    <strong>{result.name}</strong>
                    <span>{result.reason}</span>
                    {result.auditId ? <code>{result.auditId}</code> : null}
                  </li>
                ))}
              </ul>
            </section>
          ) : null}
          <div className="topology-batch-dialog-actions">
            <button
              type="button"
              className="secondary-button"
              disabled={busy}
              onClick={() => {
                setPreviewOpen(false);
                setResults([]);
              }}
            >
              {label(locale, "common.cancel")}
            </button>
            <button
              type="button"
              className={
                action && DANGEROUS_ACTIONS.has(action)
                  ? "danger-button"
                  : "primary-button"
              }
              disabled={busy || !canExecute}
              onClick={() => void executeBatch()}
            >
              {busy
                ? label(locale, "common.loading")
                : tr(locale, "topology.batch.execute", {
                    count: String(plan.eligible.length),
                  })}
            </button>
          </div>
        </section>
      ) : null}
    </section>
  );
}
