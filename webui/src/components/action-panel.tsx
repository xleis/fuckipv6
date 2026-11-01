import * as Dialog from "@radix-ui/react-dialog";
import { AlertTriangle, ChevronRight, ShieldAlert, X } from "lucide-react";
import { useId, useState, type FormEvent } from "react";
import type {
  NodeCommandOutcome,
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

type ActionPanelProps = {
  actions: NodeUiAction[];
  snapshot: NodeSnapshot;
  selectedNetwork: string;
  locale: Locale;
  pendingAction: string | null;
  onSubmit(action: NodeUiAction, values: ActionValues): Promise<NodeCommandOutcome>;
};

const numberInputs = new Set(["port", "timeout_ms"]);
const selectInputs = new Set(["network_mode", "network_selection", "role", "routing_mode"]);

function selectOptions(field: NodeUiField, snapshot: NodeSnapshot): Array<{ value: string; text: string }> {
  if (field.input === "network_mode") {
    return [
      { value: "dual_stack", text: "dual_stack" },
      { value: "ipv4_only", text: "ipv4_only" },
      { value: "ipv6_only", text: "ipv6_only" },
    ];
  }
  if (field.input === "role") {
    return ["member", "discovery", "relay", "exit_node"].map((value) => ({ value, text: value }));
  }
  if (field.input === "routing_mode") {
    return ["mesh_only", "bypass_mainland", "global_proxy"].map((value) => ({ value, text: value }));
  }
  return [
    { value: "default", text: "default" },
    { value: "all", text: "all" },
    { value: "none", text: "none" },
    ...snapshot.networks.map((network) => ({ value: network.name, text: network.name })),
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
        <select id={id} value={String(value)} onChange={(event) => onChange(event.target.value)}>
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
        type={numberInputs.has(field.input) ? "number" : field.input === "url_or_ticket" ? "url" : "text"}
        value={String(value)}
        required={field.required}
        min={field.input === "port" ? 1 : field.input === "timeout_ms" ? 1 : undefined}
        inputMode={numberInputs.has(field.input) ? "numeric" : undefined}
        autoComplete="off"
        onChange={(event) => onChange(event.target.value)}
      />
    </label>
  );
}

function ActionCard({
  action,
  snapshot,
  selectedNetwork,
  locale,
  pending,
  onSubmit,
}: {
  action: NodeUiAction;
  snapshot: NodeSnapshot;
  selectedNetwork: string;
  locale: Locale;
  pending: boolean;
  onSubmit(action: NodeUiAction, values: ActionValues): Promise<NodeCommandOutcome>;
}) {
  const [open, setOpen] = useState(false);
  const [confirming, setConfirming] = useState(false);
  const [values, setValues] = useState<ActionValues>(() =>
    initialActionValues(action, snapshot, selectedNetwork),
  );
  const [errors, setErrors] = useState<string[]>([]);
  const [result, setResult] = useState("");
  const [failure, setFailure] = useState("");
  const blocked = executionBlockReason(action, snapshot, selectedNetwork);

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
      const outcome = await onSubmit(action, values);
      if (outcome.status === "rejected") {
        setFailure(outcomeSummary(outcome, locale));
      } else {
        setResult(outcomeSummary(outcome, locale));
        setOpen(false);
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
        <ChevronRight className={open ? "action-chevron open" : "action-chevron"} size={16} />
      </button>
      {open ? (
        <form className="action-form" onSubmit={handleSubmit}>
          {blocked ? (
            <div className="inline-notice warning">
              <ShieldAlert size={15} />
              <span>{blockReasonText(blocked, locale)}</span>
            </div>
          ) : null}
          {action.fields.map((field) => (
            <FieldControl
              key={field.id}
              field={field}
              locale={locale}
              snapshot={snapshot}
              value={values[field.id] ?? ""}
              onChange={(value) => setValues((current) => ({ ...current, [field.id]: value }))}
            />
          ))}
          {errors.length > 0 ? (
            <ul className="form-errors" role="alert">
              {errors.map((error) => (
                <li key={error}>{error}</li>
              ))}
            </ul>
          ) : null}
          {failure ? <p className="form-error" role="alert">{failure}</p> : null}
          {result ? <p className="form-success" role="status">{result}</p> : null}
          <button className="primary-button" type="submit" disabled={Boolean(blocked) || pending}>
            {pending ? label(locale, "common.operation_in_progress") : label(locale, "common.execute")}
          </button>
        </form>
      ) : null}

      <Dialog.Root open={confirming} onOpenChange={setConfirming}>
        <Dialog.Portal>
          <Dialog.Overlay className="dialog-overlay" />
          <Dialog.Content className="dialog-content confirm-dialog">
            <div className="dialog-heading">
              <div className={action.confirmation?.destructive ? "dialog-icon danger" : "dialog-icon"}>
                <AlertTriangle size={20} />
              </div>
              <div>
                <Dialog.Title>{label(locale, action.confirmation?.title_key ?? "common.confirm_again")}</Dialog.Title>
                <Dialog.Description>
                  {label(locale, action.confirmation?.consequence_key ?? "common.ready_to_submit")}
                </Dialog.Description>
              </div>
            </div>
            <div className="dialog-actions">
              <Dialog.Close asChild>
                <button className="secondary-button" type="button">{label(locale, "common.cancel")}</button>
              </Dialog.Close>
              <button
                className={action.confirmation?.destructive ? "danger-button" : "primary-button"}
                type="button"
                disabled={pending}
                onClick={() => {
                  setConfirming(false);
                  void submit();
                }}
              >
                {label(locale, "common.execute")}
              </button>
            </div>
            <Dialog.Close asChild>
              <button className="icon-button dialog-close" type="button" aria-label={label(locale, "common.close")}>
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
        <div className="panel-title">{label(locale, "common.shared_actions")}</div>
        <p className="empty-copy">{label(locale, "common.page_actions_empty")}</p>
      </aside>
    );
  }
  return (
    <aside
      className="actions-panel"
      data-ui-region="action-panel"
      aria-label={label(locale, "common.shared_actions")}
    >
      <div className="panel-title">{label(locale, "common.shared_actions")}</div>
      <div className="action-list">
        {actions.map((action) => (
          <ActionCard
            key={`${action.id}:${props.selectedNetwork}`}
            action={action}
            snapshot={props.snapshot}
            selectedNetwork={props.selectedNetwork}
            locale={locale}
            pending={pendingAction === action.id}
            onSubmit={onSubmit}
          />
        ))}
      </div>
    </aside>
  );
}
