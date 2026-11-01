import * as Dialog from "@radix-ui/react-dialog";
import {
  AlertTriangle,
  Braces,
  CheckCircle2,
  Clock3,
  FileJson,
  GitCompare,
  History,
  LoaderCircle,
  RefreshCw,
  RotateCcw,
  ShieldAlert,
  Upload,
  X,
} from "lucide-react";
import { useEffect, useId, useRef, useState, type ChangeEvent, type JSX } from "react";
import type {
  ConfigurationApplyResult,
  ConfigurationDocument,
  ConfigurationHistoryEntry,
  ConfigurationPreview,
  NodeCommandOutcome,
  NodeSnapshot,
  NodeUiAction,
} from "../../../contracts/typescript/node-contract-v1";
import {
  blockReasonText,
  executionBlockReason,
  label,
  outcomeSummary,
  parseConfigurationNetworks,
  problemText,
  type ActionValues,
} from "../contract/invariants";
import { tr } from "../i18n";

export const CONFIGURATION_ACTION_IDS = new Set([
  "get_configuration",
  "preview_configuration",
  "apply_configuration",
  "rollback_configuration",
  "get_configuration_history",
]);

type ConfigurationWorkflowProps = {
  actions: NodeUiAction[];
  snapshot: NodeSnapshot;
  selectedNetwork: string;
  locale: Locale;
  pendingAction: string | null;
  onSubmit(
    action: NodeUiAction,
    values: ActionValues,
  ): Promise<NodeCommandOutcome>;
};

type BusyState = "load" | "history" | "preview" | "apply" | "rollback" | null;

function configurationJson(document: ConfigurationDocument | null): string {
  return document ? JSON.stringify(document.networks, null, 2) : "";
}

function rejectedMessage(
  outcome: NodeCommandOutcome,
  locale: Locale,
): string | null {
  return outcome.status === "rejected"
    ? problemText(outcome.data, locale)
    : null;
}

function errorText(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function jsonPointerSelection(
  source: string,
  pointer: string,
): { start: number; end: number } {
  if (!pointer.startsWith("/") || pointer === "/") {
    return { start: 0, end: 0 };
  }
  const segments = pointer
    .slice(1)
    .split("/")
    .map((segment) => segment.replaceAll("~1", "/").replaceAll("~0", "~"));
  let start = 0;
  let end = 0;
  for (const segment of segments) {
    const token = JSON.stringify(segment);
    const found = source.indexOf(token, start);
    if (found < 0) continue;
    start = found;
    end = found + token.length;
  }
  return { start, end };
}

function timeText(locale: Locale, value: string): string {
  const date = new Date(value);
  return Number.isNaN(date.getTime()) ? value : date.toLocaleString(locale);
}

function historyActionText(locale: Locale, action: string): string {
  return label(
    locale,
    action === "rollback"
      ? "configuration.history.action.rollback"
      : "configuration.history.action.apply",
  );
}

function AuditDetails({
  audit,
  locale,
}: {
  audit: ConfigurationHistoryEntry;
  locale: Locale;
}): JSX.Element {
  return (
    <dl className="configuration-audit-grid">
      <div>
        <dt>{label(locale, "configuration.history.revision")}</dt>
        <dd>{audit.revision}</dd>
      </div>
      <div>
        <dt>{label(locale, "configuration.history.previous")}</dt>
        <dd>{audit.previous_revision}</dd>
      </div>
      <div>
        <dt>{label(locale, "configuration.history.actor")}</dt>
        <dd title={audit.actor}>{audit.actor || "—"}</dd>
      </div>
      <div>
        <dt>{label(locale, "configuration.history.source")}</dt>
        <dd>{audit.source || "—"}</dd>
      </div>
      <div>
        <dt>{label(locale, "configuration.history.transaction")}</dt>
        <dd><code>{audit.transaction_id}</code></dd>
      </div>
      <div>
        <dt>{label(locale, "configuration.history.time")}</dt>
        <dd>{timeText(locale, audit.occurred_at)}</dd>
      </div>
      {audit.target_revision !== null && audit.target_revision !== undefined ? (
        <div>
          <dt>{label(locale, "configuration.history.target")}</dt>
          <dd>{audit.target_revision}</dd>
        </div>
      ) : null}
      <div className="configuration-audit-paths">
        <dt>{label(locale, "configuration.history.changed_paths")}</dt>
        <dd>
          {audit.changed_paths.length > 0
            ? audit.changed_paths.map((path) => <code key={path}>{path}</code>)
            : label(locale, "configuration.history.no_changes")}
        </dd>
      </div>
    </dl>
  );
}

export function ConfigurationWorkflow({
  actions,
  snapshot,
  selectedNetwork,
  locale,
  pendingAction,
  onSubmit,
}: ConfigurationWorkflowProps): JSX.Element {
  const getAction = actions.find((action) => action.id === "get_configuration");
  const previewAction = actions.find(
    (action) => action.id === "preview_configuration",
  );
  const applyAction = actions.find(
    (action) => action.id === "apply_configuration",
  );
  const rollbackAction = actions.find(
    (action) => action.id === "rollback_configuration",
  );
  const historyAction = actions.find(
    (action) => action.id === "get_configuration_history",
  );
  const titleId = useId();
  const editorId = useId();
  const importId = useId();
  const editorRef = useRef<HTMLTextAreaElement>(null);
  const fileInputRef = useRef<HTMLInputElement>(null);
  const initialLoadStarted = useRef(false);
  const [document, setDocument] = useState<ConfigurationDocument | null>(null);
  const [editor, setEditor] = useState("");
  const [history, setHistory] = useState<ConfigurationHistoryEntry[]>([]);
  const [preview, setPreview] = useState<ConfigurationPreview | null>(null);
  const [applied, setApplied] = useState<ConfigurationApplyResult | null>(null);
  const [busy, setBusy] = useState<BusyState>("load");
  const [error, setError] = useState("");
  const [historyError, setHistoryError] = useState("");
  const [notice, setNotice] = useState("");
  const [importName, setImportName] = useState("");
  const [activeIssuePath, setActiveIssuePath] = useState("");
  const [confirmingApply, setConfirmingApply] = useState(false);
  const [rollbackTarget, setRollbackTarget] =
    useState<ConfigurationHistoryEntry | null>(null);

  const viewBlocked = getAction
    ? executionBlockReason(getAction, snapshot, selectedNetwork)
    : "backend_unavailable";
  const applyBlocked = applyAction
    ? executionBlockReason(applyAction, snapshot, selectedNetwork)
    : "backend_unavailable";
  const rollbackBlocked = rollbackAction
    ? executionBlockReason(rollbackAction, snapshot, selectedNetwork)
    : "backend_unavailable";
  const dirty = Boolean(
    document && editor !== configurationJson(document),
  );
  const canApply = Boolean(
    preview?.can_apply &&
      preview.has_changes &&
      !preview.revision_conflict &&
      !applyBlocked,
  );

  async function request(
    action: NodeUiAction | undefined,
    values: ActionValues,
  ): Promise<NodeCommandOutcome> {
    if (!action) throw new Error(label(locale, "common.operation_failed"));
    const outcome = await onSubmit(action, values);
    const rejected = rejectedMessage(outcome, locale);
    if (rejected) throw new Error(rejected);
    return outcome;
  }

  async function refreshHistory(): Promise<void> {
    if (!historyAction) return;
    setHistoryError("");
    try {
      const outcome = await request(historyAction, {});
      if (outcome.status !== "configuration_history")
        throw new Error(label(locale, "common.operation_failed"));
      setHistory(outcome.data);
    } catch (historyFailure) {
      setHistoryError(errorText(historyFailure));
    }
  }

  async function loadAll(preserveEditor: boolean): Promise<void> {
    if (!getAction) return;
    setBusy("load");
    setError("");
    setHistoryError("");
    setNotice("");
    try {
      const outcome = await request(getAction, {});
      if (outcome.status !== "configuration") {
        throw new Error(label(locale, "common.operation_failed"));
      }
      setDocument(outcome.data);
      if (!preserveEditor || !editor) setEditor(configurationJson(outcome.data));
      setPreview(null);
      setApplied(null);
      await refreshHistory();
    } catch (loadFailure) {
      setError(errorText(loadFailure));
    } finally {
      setBusy(null);
    }
  }

  useEffect(() => {
    if (!getAction || initialLoadStarted.current) return;
    initialLoadStarted.current = true;
    void loadAll(false);
    // The initial load is deliberately once per mounted settings page. Command
    // callbacks are recreated by AppShell and must not restart the workflow.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [getAction]);

  function replaceEditor(value: string, importedName = ""): void {
    setEditor(value);
    setImportName(importedName);
    setPreview(null);
    setApplied(null);
    setNotice("");
    setError("");
    setActiveIssuePath("");
  }

  async function previewDraft(): Promise<void> {
    if (!previewAction || !document) return;
    setBusy("preview");
    setError("");
    setNotice("");
    setApplied(null);
    setActiveIssuePath("");
    try {
      parseConfigurationNetworks(editor, locale);
      const outcome = await request(previewAction, {
        expected_revision: String(document.revision),
        networks: editor,
      });
      if (outcome.status !== "configuration_preview") {
        throw new Error(label(locale, "common.operation_failed"));
      }
      setPreview(outcome.data);
      if (outcome.data.revision_conflict) {
        setNotice(label(locale, "configuration.revision_conflict"));
      }
    } catch (previewFailure) {
      setPreview(null);
      setError(errorText(previewFailure));
    } finally {
      setBusy(null);
    }
  }

  async function applyDraft(): Promise<void> {
    if (!applyAction || !document || !preview || !canApply) return;
    setConfirmingApply(false);
    setBusy("apply");
    setError("");
    setNotice("");
    try {
      const outcome = await request(applyAction, {
        expected_revision: String(document.revision),
        networks: editor,
      });
      if (outcome.status !== "configuration_applied") {
        throw new Error(label(locale, "common.operation_failed"));
      }
      setApplied(outcome.data);
      setDocument(outcome.data.document);
      setEditor(configurationJson(outcome.data.document));
      setPreview(null);
      setNotice(outcomeSummary(outcome, locale));
      await refreshHistory();
    } catch (applyFailure) {
      setError(errorText(applyFailure));
    } finally {
      setBusy(null);
    }
  }

  async function rollbackDraft(target: ConfigurationHistoryEntry): Promise<void> {
    if (!rollbackAction || !document) return;
    setRollbackTarget(null);
    setBusy("rollback");
    setError("");
    setNotice("");
    try {
      const outcome = await request(rollbackAction, {
        expected_revision: String(document.revision),
        target_revision: String(target.revision),
      });
      if (outcome.status !== "configuration_applied") {
        throw new Error(label(locale, "common.operation_failed"));
      }
      setApplied(outcome.data);
      setDocument(outcome.data.document);
      setEditor(configurationJson(outcome.data.document));
      setPreview(null);
      setNotice(outcomeSummary(outcome, locale));
      await refreshHistory();
    } catch (rollbackFailure) {
      setError(errorText(rollbackFailure));
    } finally {
      setBusy(null);
    }
  }

  async function importFile(event: ChangeEvent<HTMLInputElement>): Promise<void> {
    const file = event.target.files?.[0];
    event.target.value = "";
    if (!file) return;
    try {
      const raw = await file.text();
      const networks = parseConfigurationNetworks(raw, locale);
      replaceEditor(JSON.stringify(networks, null, 2), file.name);
    } catch (importFailure) {
      setError(errorText(importFailure));
    }
  }

  function locateIssue(path: string): void {
    const editorElement = editorRef.current;
    if (!editorElement) return;
    setActiveIssuePath(path);
    const selection = jsonPointerSelection(editor, path);
    editorElement.focus();
    editorElement.setSelectionRange(selection.start, selection.end);
  }

  const pending = Boolean(pendingAction) || busy !== null;

  return (
    <section
      className="configuration-workspace"
      aria-labelledby={titleId}
      data-testid="configuration-workspace"
    >
      <header className="configuration-heading">
        <div className="configuration-heading-mark">
          <FileJson size={22} />
        </div>
        <div className="configuration-heading-copy">
          <span>{label(locale, "configuration.eyebrow")}</span>
          <h2 id={titleId}>{label(locale, "configuration.title")}</h2>
          <p>{label(locale, "configuration.description")}</p>
        </div>
        <div className="configuration-heading-actions">
          {document ? (
            <span className="configuration-revision">
              {tr(locale, "configuration.revision", {
                revision: document.revision,
              })}
            </span>
          ) : null}
          {dirty ? (
            <span className="status-pill warn">
              {label(locale, "configuration.unsaved")}
            </span>
          ) : null}
          <button
            className="secondary-button"
            type="button"
            disabled={pending || Boolean(viewBlocked) || !getAction}
            onClick={() => void loadAll(Boolean(editor))}
          >
            {busy === "load" ? (
              <LoaderCircle className="spin" size={15} />
            ) : (
              <RefreshCw size={15} />
            )}
            {label(locale, "configuration.reload_latest")}
          </button>
        </div>
      </header>

      {viewBlocked ? (
        <div className="inline-notice warning">
          <ShieldAlert size={16} />
          <span>{blockReasonText(viewBlocked, locale)}</span>
        </div>
      ) : null}

      {error ? (
        <div className="inline-notice danger" role="alert">
          <AlertTriangle size={16} />
          <span>{error}</span>
        </div>
      ) : null}
      {notice ? (
        <div className="inline-notice success" role="status">
          <CheckCircle2 size={16} />
          <span>{notice}</span>
        </div>
      ) : null}

      {busy === "load" && !document ? (
        <div className="configuration-loading" role="status">
          <LoaderCircle className="spin" size={18} />
          <span>{label(locale, "configuration.loading")}</span>
        </div>
      ) : document ? (
        <>
          <div className="configuration-editor-grid">
            <section className="configuration-editor-card">
              <div className="configuration-section-heading">
                <div>
                  <span>{label(locale, "configuration.editor.eyebrow")}</span>
                  <h3>{label(locale, "configuration.editor.title")}</h3>
                </div>
                <div className="configuration-editor-actions">
                  <input
                    ref={fileInputRef}
                    id={importId}
                    className="visually-hidden"
                    type="file"
                    accept=".json,application/json"
                    onChange={(event) => void importFile(event)}
                  />
                  <button
                    className="secondary-button"
                    type="button"
                    disabled={pending}
                    onClick={() => fileInputRef.current?.click()}
                  >
                    <Upload size={15} />
                    {label(locale, "configuration.import")}
                  </button>
                </div>
              </div>
              {importName ? (
                <p className="configuration-import-name">
                  {tr(locale, "configuration.imported_from", {
                    file: importName,
                  })}
                </p>
              ) : null}
              <label className="visually-hidden" htmlFor={editorId}>
                {label(locale, "configuration.editor.label")}
              </label>
              <textarea
                ref={editorRef}
                id={editorId}
                className="configuration-editor"
                value={editor}
                spellCheck={false}
                aria-describedby={`${editorId}-help`}
                onChange={(event) => replaceEditor(event.target.value, importName)}
              />
              <p id={`${editorId}-help`} className="configuration-editor-help">
                {label(locale, "configuration.editor.help")}
              </p>
              <div className="configuration-editor-footer">
                <span>
                  <Braces size={14} />
                  {label(locale, "configuration.editor.networks_only")}
                </span>
                <button
                  className="primary-button"
                  type="button"
                  disabled={pending || Boolean(viewBlocked) || !previewAction}
                  onClick={() => void previewDraft()}
                >
                  {busy === "preview" ? (
                    <LoaderCircle className="spin" size={15} />
                  ) : (
                    <GitCompare size={15} />
                  )}
                  {label(locale, "configuration.preview")}
                </button>
              </div>
            </section>

            <section className="configuration-review-card" aria-live="polite">
              <div className="configuration-section-heading">
                <div>
                  <span>{label(locale, "configuration.review.eyebrow")}</span>
                  <h3>{label(locale, "configuration.review.title")}</h3>
                </div>
                {preview ? (
                  <span className="status-pill neutral">
                    {tr(locale, "configuration.diff_count", {
                      count: preview.diffs.length,
                    })}
                  </span>
                ) : null}
              </div>

              {!preview ? (
                <div className="configuration-empty">
                  <GitCompare size={22} />
                  <p>{label(locale, "configuration.review.empty")}</p>
                </div>
              ) : (
                <>
                  {preview.revision_conflict ? (
                    <div className="inline-notice warning">
                      <ShieldAlert size={16} />
                      <div>
                        <strong>
                          {label(locale, "configuration.revision_conflict")}
                        </strong>
                        <p>
                          {tr(locale, "configuration.revision_conflict_detail", {
                            expected: preview.expected_revision,
                            current: preview.current_revision,
                          })}
                        </p>
                      </div>
                    </div>
                  ) : null}

                  {preview.validation_errors.length > 0 ? (
                    <div className="configuration-validation">
                      <h4>
                        {tr(locale, "configuration.validation.title", {
                          count: preview.validation_errors.length,
                        })}
                      </h4>
                      <ul>
                        {preview.validation_errors.map((issue, index) => (
                          <li
                            key={`${issue.path}:${issue.code}:${index}`}
                            className={
                              activeIssuePath === issue.path
                                ? "configuration-validation-issue active"
                                : "configuration-validation-issue"
                            }
                          >
                            <button
                              type="button"
                              onClick={() => locateIssue(issue.path)}
                            >
                              <code>{issue.path || "/"}</code>
                              <span>{issue.message}</span>
                              <small>{issue.code}</small>
                            </button>
                          </li>
                        ))}
                      </ul>
                    </div>
                  ) : null}

                  <div className="configuration-diffs">
                    <h4>{label(locale, "configuration.diffs.title")}</h4>
                    {preview.diffs.length === 0 ? (
                      <p className="configuration-empty-copy">
                        {label(locale, "configuration.no_changes")}
                      </p>
                    ) : (
                      <ul>
                        {preview.diffs.map((diff) => (
                          <li key={diff.path}>
                            <div className="configuration-diff-head">
                              <code>{diff.path}</code>
                              <span className={`impact ${diff.impact}`}>
                                {label(
                                  locale,
                                  `configuration.impact.${diff.impact}`,
                                )}
                              </span>
                            </div>
                            <div className="configuration-value-diff">
                              <div>
                                <span>{label(locale, "configuration.before")}</span>
                                <pre>{JSON.stringify(diff.before, null, 2)}</pre>
                              </div>
                              <div>
                                <span>{label(locale, "configuration.after")}</span>
                                <pre>{JSON.stringify(diff.after, null, 2)}</pre>
                              </div>
                            </div>
                          </li>
                        ))}
                      </ul>
                    )}
                  </div>

                  <details className="configuration-candidate">
                    <summary>
                      {label(locale, "configuration.candidate.title")}
                    </summary>
                    <pre>{JSON.stringify(preview.candidate, null, 2)}</pre>
                  </details>

                  <div className="configuration-apply-bar">
                    <div>
                      <strong>
                        {preview.revision_conflict
                          ? label(locale, "configuration.apply.conflict")
                          : preview.validation_errors.length > 0
                            ? label(locale, "configuration.apply.invalid")
                            : !preview.has_changes
                              ? label(locale, "configuration.apply.no_changes")
                              : !preview.can_apply
                                ? label(locale, "configuration.apply.blocked")
                                : label(locale, "configuration.apply.ready")}
                      </strong>
                      {applyBlocked ? (
                        <span>{blockReasonText(applyBlocked, locale)}</span>
                      ) : null}
                    </div>
                    <button
                      className="primary-button"
                      type="button"
                      disabled={pending || !canApply}
                      onClick={() => setConfirmingApply(true)}
                    >
                      {busy === "apply" ? (
                        <LoaderCircle className="spin" size={15} />
                      ) : (
                        <CheckCircle2 size={15} />
                      )}
                      {label(locale, "configuration.apply")}
                    </button>
                  </div>
                </>
              )}
            </section>
          </div>

          {applied ? (
            <section className="configuration-applied" aria-live="polite">
              <div className="configuration-section-heading">
                <div>
                  <span>{label(locale, "configuration.audit.eyebrow")}</span>
                  <h3>{label(locale, "configuration.audit.title")}</h3>
                </div>
                <span className="status-pill good">
                  {label(
                    locale,
                    applied.no_op
                      ? "configuration.applied.unchanged"
                      : "configuration.applied.changed",
                  )}
                </span>
              </div>
              {applied.audit ? (
                <AuditDetails audit={applied.audit} locale={locale} />
              ) : (
                <p className="configuration-empty-copy">
                  {label(locale, "configuration.audit.unavailable")}
                </p>
              )}
            </section>
          ) : null}

          <section className="configuration-history">
            <div className="configuration-section-heading">
              <div>
                <span>{label(locale, "configuration.history.eyebrow")}</span>
                <h3>
                  <History size={17} />
                  {label(locale, "configuration.history.title")}
                </h3>
              </div>
              <button
                className="secondary-button"
                type="button"
                disabled={pending || !historyAction}
                onClick={() => void refreshHistory()}
              >
                {busy === "history" ? (
                  <LoaderCircle className="spin" size={15} />
                ) : (
                  <RefreshCw size={15} />
                )}
                {label(locale, "configuration.history.refresh")}
              </button>
            </div>
            {historyError ? (
              <div className="inline-notice danger" role="alert">
                <AlertTriangle size={16} />
                <span>{historyError}</span>
              </div>
            ) : history.length === 0 ? (
              <div className="configuration-empty">
                <History size={22} />
                <p>{label(locale, "configuration.history.empty")}</p>
              </div>
            ) : (
              <ol className="configuration-history-list">
                {history.map((entry) => (
                  <li key={entry.transaction_id}>
                    <div className="configuration-history-summary">
                      <div>
                        <strong>
                          {tr(locale, "configuration.history.revision", {
                            revision: entry.revision,
                          })}
                        </strong>
                        <span>
                          {historyActionText(locale, entry.action)}
                          {entry.target_revision !== null &&
                          entry.target_revision !== undefined
                            ? ` · ${tr(locale, "configuration.history.target_value", {
                                revision: entry.target_revision,
                              })}`
                            : ""}
                        </span>
                      </div>
                      <div className="configuration-history-meta">
                        <span>
                          <Clock3 size={13} />
                          {timeText(locale, entry.occurred_at)}
                        </span>
                        <span>{entry.source || entry.actor}</span>
                      </div>
                      <button
                        className="secondary-button"
                        type="button"
                        disabled={
                          pending ||
                          Boolean(rollbackBlocked) ||
                          !rollbackAction ||
                          entry.revision === document.revision
                        }
                        title={
                          rollbackBlocked
                            ? blockReasonText(rollbackBlocked, locale)
                            : entry.revision === document.revision
                              ? label(locale, "configuration.rollback.current")
                              : label(locale, "configuration.rollback.action")
                        }
                        onClick={() => setRollbackTarget(entry)}
                      >
                        <RotateCcw size={14} />
                        {label(locale, "configuration.rollback.action")}
                      </button>
                    </div>
                    <details>
                      <summary>
                        {label(locale, "configuration.history.details")}
                      </summary>
                      <AuditDetails audit={entry} locale={locale} />
                    </details>
                  </li>
                ))}
              </ol>
            )}
          </section>
        </>
      ) : null}

      <Dialog.Root open={confirmingApply} onOpenChange={setConfirmingApply}>
        <Dialog.Portal>
          <Dialog.Overlay className="dialog-overlay" />
          <Dialog.Content className="dialog-content confirm-dialog">
            <div className="dialog-heading">
              <div className="dialog-icon">
                <GitCompare size={20} />
              </div>
              <div>
                <Dialog.Title>
                  {label(locale, "configuration.apply.confirm_title")}
                </Dialog.Title>
                <Dialog.Description>
                  {tr(locale, "configuration.apply.confirm_detail", {
                    count: preview?.diffs.length ?? 0,
                    revision: document?.revision ?? 0,
                  })}
                </Dialog.Description>
              </div>
            </div>
            <div className="dialog-actions">
              <Dialog.Close asChild>
                <button className="secondary-button" type="button">
                  {label(locale, "common.cancel")}
                </button>
              </Dialog.Close>
              <button
                className="primary-button"
                type="button"
                disabled={pending}
                onClick={() => void applyDraft()}
              >
                {label(locale, "configuration.apply.confirm_action")}
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

      <Dialog.Root
        open={rollbackTarget !== null}
        onOpenChange={(open) => {
          if (!open) setRollbackTarget(null);
        }}
      >
        <Dialog.Portal>
          <Dialog.Overlay className="dialog-overlay" />
          <Dialog.Content className="dialog-content confirm-dialog">
            <div className="dialog-heading">
              <div className="dialog-icon danger">
                <RotateCcw size={20} />
              </div>
              <div>
                <Dialog.Title>
                  {label(locale, "configuration.rollback.confirm_title")}
                </Dialog.Title>
                <Dialog.Description>
                  {tr(locale, "configuration.rollback.confirm_detail", {
                    target: rollbackTarget?.revision ?? 0,
                    current: document?.revision ?? 0,
                  })}
                </Dialog.Description>
              </div>
            </div>
            <div className="dialog-actions">
              <Dialog.Close asChild>
                <button className="secondary-button" type="button">
                  {label(locale, "common.cancel")}
                </button>
              </Dialog.Close>
              <button
                className="danger-button"
                type="button"
                disabled={pending || !rollbackTarget}
                onClick={() =>
                  rollbackTarget && void rollbackDraft(rollbackTarget)
                }
              >
                {label(locale, "configuration.rollback.confirm_action")}
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
    </section>
  );
}
