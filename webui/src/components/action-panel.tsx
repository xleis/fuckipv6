import * as Dialog from "@radix-ui/react-dialog";
import {
  AlertTriangle,
  Check,
  ChevronRight,
  Copy,
  ShieldAlert,
  X,
} from "lucide-react";
import { useEffect, useId, useState, type FormEvent } from "react";
import type {
  NodeCommandOutcome,
  NodeOperationSnapshot,
  NodeSnapshot,
  NodeUiAction,
  NodeUiField,
} from "../../../contracts/typescript/node-contract-v1";
import {
  blockReasonText,
  executionBlockReason,
  initialActionValues,
  label,
  outcomeSummary,
  validateActionValues,
  type ActionValues,
} from "../contract/invariants";
import { tr } from "../i18n";
import { pageStateDefinition, type PageStateKind } from "../state-matrix";

type ActionPanelProps = {
  actions: NodeUiAction[];
  snapshot: NodeSnapshot;
  selectedNetwork: string;
  locale: Locale;
  pendingAction: string | null;
  interactionBlocked: PageStateKind | null;
  requestedActionId: string | null;
  operation: NodeOperationSnapshot | null;
  onRequestedActionOpened(): void;
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
const selectInputs = new Set([
  "network_mode",
  "network_selection",
  "role",
  "routing_mode",
]);

function selectOptions(
  field: NodeUiField,
  snapshot: NodeSnapshot,
): Array<{ value: string; text: string }> {
  if (field.input === "network_mode") {
    return [
      { value: "dual_stack", text: "dual_stack" },
      { value: "ipv4_only", text: "ipv4_only" },
      { value: "ipv6_only", text: "ipv6_only" },
    ];
  }
  if (field.input === "role") {
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
          {selectOptions(field, snapshot).map((option) => (
            <option key={option.value} value={option.value}>
              {option.text}
            </option>
          ))}
        </select>
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
            : field.input === "url_or_ticket"
              ? "url"
              : "text"
        }
        value={String(value)}
        required={field.required}
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

function OperationStatusCard({
  operation,
  locale,
}: {
  operation: NodeOperationSnapshot;
  locale: Locale;
}) {
  const [copied, setCopied] = useState(false);
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
    } catch {
      setCopied(false);
    }
  }

  return (
    <section className="operation-status" data-testid="operation-status">
      <div className="operation-status-heading">
        <div>
          <span>{label(locale, "operation.title")}</span>
          <strong>{label(locale, `operation.state.${operation.state}`)}</strong>
        </div>
        <button
          className="secondary-button operation-copy"
          type="button"
          onClick={() => void copyDiagnosticId()}
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

function ActionCard({
  action,
  snapshot,
  selectedNetwork,
  locale,
  pending,
  interactionBlocked,
  requested,
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
  onRequestedActionOpened(): void;
  onSubmit(
    action: NodeUiAction,
    values: ActionValues,
    onOperation: (operation: NodeOperationSnapshot) => void,
  ): Promise<ActionSubmission>;
}) {
  const [open, setOpen] = useState(requested);
  const [confirming, setConfirming] = useState(false);
  const [values, setValues] = useState<ActionValues>(() =>
    initialActionValues(action, snapshot, selectedNetwork),
  );
  const [errors, setErrors] = useState<string[]>([]);
  const [result, setResult] = useState("");
  const [failure, setFailure] = useState("");
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
  const [previousRequested, setPreviousRequested] = useState(requested);

  if (requested !== previousRequested) {
    setPreviousRequested(requested);
    if (requested) setOpen(true);
  }

  useEffect(() => {
    if (!requested) return;
    onRequestedActionOpened();
  }, [requested, onRequestedActionOpened]);

  async function submit(): Promise<void> {
    const validation = validateActionValues(action, values, locale);
    if (validation.length > 0) {
      setErrors(validation);
      return;
    }
    setErrors([]);
    setFailure("");
    setResult("");
    try {
      const submission = await onSubmit(action, values, () => {});
      if (submission.outcome?.status === "rejected") {
        setFailure(outcomeSummary(submission.outcome, locale));
      } else if (submission.outcome) {
        setResult(outcomeSummary(submission.outcome, locale));
        setOpen(false);
      } else if (submission.operation) {
        setFailure(operationFailureSummary(submission.operation, locale));
      } else {
        setFailure(label(locale, "common.operation_failed"));
      }
    } catch (error) {
      setFailure(error instanceof Error ? error.message : String(error));
    }
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
        onClick={() => setOpen((current) => !current)}
      >
        <span>{label(locale, action.label_key)}</span>
        <ChevronRight
          className={open ? "action-chevron open" : "action-chevron"}
          size={16}
        />
      </button>
      {open ? (
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
              onChange={(value) =>
                setValues((current) => ({ ...current, [field.id]: value }))
              }
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
          {result ? (
            <p className="form-success" role="status">
              {result}
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
  if (actions.length === 0) {
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
        <OperationStatusCard operation={props.operation} locale={locale} />
      ) : null}
      <div className="action-list">
        {actions.map((action) => (
          <ActionCard
            key={`${action.id}:${props.selectedNetwork}`}
            action={action}
            snapshot={props.snapshot}
            selectedNetwork={props.selectedNetwork}
            locale={locale}
            pending={pendingAction === action.id}
            interactionBlocked={props.interactionBlocked}
            requested={props.requestedActionId === action.id}
            onRequestedActionOpened={props.onRequestedActionOpened}
            onSubmit={onSubmit}
          />
        ))}
      </div>
    </aside>
  );
}
