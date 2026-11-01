import { AlertTriangle, Info, RefreshCw, ShieldAlert } from "lucide-react";
import type { JSX } from "react";
import type { PageState } from "../state-matrix";
import { label } from "../contract/invariants";
import { tr } from "../i18n";

export function StateNotice({
  state,
  locale,
  lastAuthoritativeAt,
}: {
  state: PageState;
  locale: Locale;
  lastAuthoritativeAt?: number | null;
}): JSX.Element | null {
  if (["ready", "empty", "initial_loading"].includes(state.kind)) return null;
  const time = lastAuthoritativeAt
    ? new Date(lastAuthoritativeAt).toLocaleString(locale)
    : label(locale, "common.unknown");
  const Icon =
    state.tone === "danger"
      ? AlertTriangle
      : state.kind === "refreshing"
        ? RefreshCw
        : state.tone === "warn"
          ? ShieldAlert
          : Info;
  return (
    <section
      className="state-notice-stack"
      aria-label={label(locale, "common.runtime_status")}
    >
      <div
        className={`state-notice ${state.tone}`}
        data-page-state={state.kind}
        role={state.tone === "danger" ? "alert" : "status"}
      >
        <Icon
          className={state.kind === "refreshing" ? "spin" : undefined}
          size={17}
          aria-hidden="true"
        />
        <div>
          <strong>{label(locale, state.titleKey)}</strong>
          <p>{label(locale, state.detailKey)}</p>
          {["offline", "stale", "refreshing"].includes(state.kind) ? (
            <small>
              {tr(locale, "common.last_authoritative", { time })}
            </small>
          ) : null}
        </div>
      </div>
    </section>
  );
}
