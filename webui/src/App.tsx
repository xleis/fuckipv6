import * as DropdownMenu from "@radix-ui/react-dropdown-menu";
import {
  useMutation,
  useQuery,
  useQueryClient,
  type QueryClient,
} from "@tanstack/react-query";
import {
  Activity as ActivityIcon,
  AlertCircle,
  CheckCircle2,
  ChevronDown,
  Globe2,
  KeyRound,
  LoaderCircle,
  LogOut,
  Network as NetworkIcon,
  RefreshCw,
  RotateCcw,
  Search as SearchIcon,
  ServerCog,
  Settings as SettingsIcon,
  ShieldAlert,
  Wifi,
  X,
} from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState, type JSX } from "react";
import type {
  NodeCommandOutcome,
  NodeOperationSnapshot,
  NodeSnapshot,
  NodeUiAction,
  NodeUiManifest,
  NodeUiPage,
} from "../../contracts/typescript/node-contract-v1";
import {
  AuthenticationError,
  HttpError,
  loadManifest,
  loadSnapshot,
  newIdempotencyKey,
  nextRequestId,
  sendCommand,
  waitForOperation,
} from "./api/client";
import {
  loadLanguagePreference,
  loadOwnerSession,
  loginWithPasskey,
  logoutOwnerSession,
  saveLanguagePreference,
  type LanguagePreference,
} from "./api/session";
import { webAuthnIssue } from "./auth/webauthn";
import { tr } from "./i18n";
import {
  NAVIGATION_MODULES,
  navigationModuleForPage,
  type NavigationModuleContract,
} from "./navigation";
import { ActionPanel } from "./components/action-panel";
import {
  CONFIGURATION_ACTION_IDS,
  ConfigurationWorkflow,
  type ConfigurationDraftMemo,
} from "./components/configuration-workflow";
import { PageContent } from "./components/page-content";
import { StateNotice } from "./components/state-notice";
import {
  commandEnvelope,
  defaultNetworkName,
  label,
  operationStateIsTerminal,
  primaryStatus,
  validateSnapshot,
  type ActionValues,
} from "./contract/invariants";
import {
  interactionBlockReason,
  pageStateSignals,
  pageUnsupported,
  primaryPageState,
} from "./state-matrix";
import {
  searchElementId,
  searchKindKey,
  searchNodeSnapshot,
  type SearchResult,
} from "./search";

type ModuleDefinition = {
  id: string;
  labelKey: string;
  defaultPageId: string;
  pages: string[];
  icon: typeof Globe2;
};

const MODULE_ICONS: Record<string, typeof Globe2> = {
  overview: Globe2,
  nodes: ServerCog,
  networks: NetworkIcon,
  activity: ActivityIcon,
  settings: SettingsIcon,
};

const MODULES: ModuleDefinition[] = NAVIGATION_MODULES.map(
  (module: NavigationModuleContract) => ({
    id: module.id,
    labelKey: module.labelKey,
    defaultPageId: module.defaultPageId,
    pages: module.pageIds,
    icon: MODULE_ICONS[module.id] ?? Globe2,
  }),
);

const SESSION_QUERY_KEY = ["owner-session"] as const;
const LANGUAGE_QUERY_KEY = ["language-preference"] as const;
const MANIFEST_QUERY_KEY = ["node-manifest", 1] as const;
const SNAPSHOT_QUERY_KEY = ["node-snapshot", 1] as const;

type UiSessionMemo = {
  pageId: string;
  selectedNetwork: string;
  searchQuery: string;
  requestedActionId: string | null;
  configurationDraft: ConfigurationDraftMemo | null;
};

const EMPTY_UI_SESSION: UiSessionMemo = {
  pageId: "",
  selectedNetwork: "",
  searchQuery: "",
  requestedActionId: null,
  configurationDraft: null,
};

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function isControlMaintenance(error: unknown): boolean {
  return error instanceof HttpError && error.status === 503;
}

function moduleForPage(pageId: string): ModuleDefinition {
  const contract = navigationModuleForPage(pageId);
  return MODULES.find((module) => module.id === contract.id) ?? MODULES[0];
}

function resolveLocale(preference: LanguagePreference | undefined): Locale {
  const runtime = window.FuckIPV6I18n;
  if (!runtime) return "zh-CN";
  return runtime.resolveLocale(preference ?? "system", navigator.language);
}

type NodeEventState = {
  status: "connecting" | "live" | "resyncing" | "reconnecting";
  stale: boolean;
  lastAuthoritativeAt: number | null;
};

function useNodeEvents(
  authenticated: boolean,
  queryClient: QueryClient,
): NodeEventState {
  const [state, setState] = useState<NodeEventState>({
    status: "connecting",
    stale: false,
    lastAuthoritativeAt: null,
  });

  useEffect(() => {
    if (!authenticated) return;
    let socket: WebSocket | null = null;
    let reconnectTimer: number | null = null;
    let stopped = false;

    const queueResync = () => {
      setState((current) => ({
        ...current,
        status: "resyncing",
        stale: true,
      }));
      void queryClient.invalidateQueries({ queryKey: SNAPSHOT_QUERY_KEY });
    };

    const connect = () => {
      if (stopped) return;
      setState((current) => ({ ...current, status: "connecting" }));
      const protocol = window.location.protocol === "https:" ? "wss:" : "ws:";
      socket = new WebSocket(
        `${protocol}//${window.location.host}/api/node/events?schema_version=1`,
      );
      socket.addEventListener("open", () =>
        setState((current) => ({ ...current, status: "live" })),
      );
      socket.addEventListener("message", (event) => {
        try {
          const message = JSON.parse(String(event.data)) as Record<
            string,
            unknown
          >;
          if (
            message.schema_version !== 1 ||
            typeof message.type !== "string"
          ) {
            queueResync();
            return;
          }
          if (message.type === "snapshot" || message.type === "resync") {
            const snapshot = validateSnapshot(message.snapshot as NodeSnapshot);
            queryClient.setQueryData(SNAPSHOT_QUERY_KEY, snapshot);
            setState({
              status: "live",
              stale: false,
              lastAuthoritativeAt: Date.now(),
            });
            return;
          }
          if (message.type === "event") {
            void queryClient.invalidateQueries({
              queryKey: SNAPSHOT_QUERY_KEY,
            });
            return;
          }
          if (message.type === "error") queueResync();
        } catch {
          queueResync();
        }
      });
      socket.addEventListener("close", () => {
        setState((current) => ({
          ...current,
          status: stopped ? current.status : "reconnecting",
          stale: !stopped,
        }));
        if (!stopped) reconnectTimer = window.setTimeout(connect, 2_000);
      });
      socket.addEventListener("error", () => socket?.close());
    };

    connect();
    return () => {
      stopped = true;
      if (reconnectTimer !== null) window.clearTimeout(reconnectTimer);
      socket?.close();
    };
  }, [authenticated, queryClient]);

  return state;
}

function useBrowserOnline(): boolean {
  const [online, setOnline] = useState(() => navigator.onLine);
  useEffect(() => {
    const update = () => setOnline(navigator.onLine);
    window.addEventListener("online", update);
    window.addEventListener("offline", update);
    return () => {
      window.removeEventListener("online", update);
      window.removeEventListener("offline", update);
    };
  }, []);
  return online;
}

function LoadingScreen({ locale }: { locale: Locale }): JSX.Element {
  return (
    <main className="centered-shell" aria-busy="true">
      <div className="loading-mark">
        <LoaderCircle className="spin" size={30} />
      </div>
      <h1>{label(locale, "owner_auth.session.checking")}</h1>
    </main>
  );
}

function AuthenticatedLoadingShell({ locale }: { locale: Locale }): JSX.Element {
  return (
    <div className="app-shell" aria-busy="true">
      <a className="skip-link" href="#page-content">
        {label(locale, "common.skip_content")}
      </a>
      <aside
        className="primary-nav"
        data-ui-region="sidebar"
        aria-label={label(locale, "common.main_navigation")}
      >
        <div className="brand-lockup">
          <div className="brand-mark">
            <Wifi size={22} />
          </div>
          <div>
            <span>fuckipv6</span>
            <strong>{label(locale, "common.node_contract")}</strong>
          </div>
        </div>
        <nav className="module-list" aria-label={label(locale, "common.main_navigation")}>
          {MODULES.map((module) => {
            const Icon = module.icon;
            return (
              <button
                key={module.id}
                type="button"
                className="module-button"
                disabled
              >
                <Icon size={19} />
                <span>{label(locale, module.labelKey)}</span>
              </button>
            );
          })}
        </nav>
        <div className="nav-footer">
          <span>{label(locale, "state.initial_loading.title")}</span>
          <span className="stale-indicator active">
            <span aria-hidden="true" /> {label(locale, "state.refreshing.title")}
          </span>
        </div>
      </aside>

      <div className="app-main">
        <header className="global-bar" data-ui-region="topbar">
          <div className="global-context">
            <div className="top-status warn" role="status">
              <LoaderCircle className="spin" size={17} />
              <div>
                <strong>{label(locale, "state.initial_loading.title")}</strong>
                <span>{label(locale, "state.initial_loading.detail")}</span>
              </div>
            </div>
          </div>
          <label className="network-picker">
            <span>{label(locale, "common.current_network")}</span>
            <select disabled value="">
              <option value="">{label(locale, "state.initial_loading.title")}</option>
            </select>
          </label>
          <div className="global-actions">
            <button
              className="toolbar-button"
              type="button"
              disabled
              aria-label={label(locale, "common.language")}
            >
              <Globe2 size={16} />
            </button>
            <button
              className="icon-button"
              type="button"
              disabled
              aria-label={label(locale, "common.refresh")}
            >
              <RefreshCw className="spin" size={17} />
            </button>
            <button
              className="icon-button"
              type="button"
              disabled
              aria-label={label(locale, "owner_auth.session.logout")}
            >
              <LogOut size={17} />
            </button>
          </div>
        </header>

        <div className="secondary-shell">
          <div className="local-toolbar">
            <div className="page-tabs" role="tablist" aria-label={label(locale, "common.page_sections")}>
              {MODULES.map((module) => (
                <button
                  key={module.id}
                  className="page-tab"
                  type="button"
                  role="tab"
                  aria-selected={false}
                  disabled
                >
                  {label(locale, module.labelKey)}
                </button>
              ))}
            </div>
            <div className="local-toolbar-actions">
              <span className="sync-badge" role="status">
                {label(locale, "state.initial_loading.title")}
              </span>
            </div>
          </div>
        </div>

        <main className="workspace" id="page-content" tabIndex={-1}>
          <section
            className="page-content authenticated-loading-content"
            data-ui-region="content"
            aria-label={label(locale, "state.initial_loading.title")}
          >
            <header className="page-heading">
              <div className="page-heading-icon">
                <LoaderCircle className="spin" size={20} />
              </div>
              <div>
                <h1>{label(locale, "state.initial_loading.title")}</h1>
                <p>{label(locale, "state.initial_loading.detail")}</p>
              </div>
            </header>
            <div className="page-content-stack">
              <div className="loading-panel" role="status">
                <LoaderCircle className="spin" size={19} />
                <div>
                  <strong>{label(locale, "state.initial_loading.title")}</strong>
                  <p>{label(locale, "state.initial_loading.detail")}</p>
                </div>
              </div>
              <div className="loading-skeleton" aria-hidden="true">
                <div className="loading-skeleton-row">
                  <span />
                  <span />
                  <span />
                </div>
                <div className="loading-skeleton-block tall" />
                <div className="loading-skeleton-block" />
              </div>
            </div>
          </section>
          <aside
            className="actions-panel"
            data-ui-region="action-panel"
            aria-label={label(locale, "common.shared_actions")}
          >
            <div className="panel-title">{label(locale, "common.shared_actions")}</div>
            <div className="loading-skeleton" aria-hidden="true">
              <div className="loading-skeleton-block compact" />
              <div className="loading-skeleton-block compact" />
            </div>
          </aside>
        </main>
      </div>
    </div>
  );
}

function FatalScreen({
  locale,
  title,
  detail,
  onRetry,
}: {
  locale: Locale;
  title: string;
  detail: string;
  onRetry?: () => void;
}): JSX.Element {
  return (
    <main className="centered-shell">
      <div className="state-icon danger">
        <AlertCircle size={28} />
      </div>
      <h1>{title}</h1>
      <p>{detail}</p>
      {onRetry ? (
        <button className="primary-button" type="button" onClick={onRetry}>
          <RotateCcw size={16} /> {label(locale, "common.retry")}
        </button>
      ) : null}
    </main>
  );
}

function LoginScreen({ locale }: { locale: Locale }): JSX.Element {
  const queryClient = useQueryClient();
  const issue = webAuthnIssue();
  const login = useMutation({
    mutationFn: loginWithPasskey,
    onSuccess: (session) =>
      queryClient.setQueryData(SESSION_QUERY_KEY, session),
  });
  const [failure, setFailure] = useState("");

  return (
    <main className="auth-shell">
      <section className="auth-card" aria-labelledby="auth-title">
        <div className="auth-emblem">
          <ShieldAlert size={32} />
        </div>
        <span className="eyebrow">fuckipv6</span>
        <h1 id="auth-title">{label(locale, "owner_auth.login.title")}</h1>
        <p>{label(locale, "owner_auth.login.description")}</p>
        {issue ? (
          <div className="inline-notice warning" role="alert">
            <AlertCircle size={16} />
            <span>{issue}</span>
          </div>
        ) : null}
        {failure ? (
          <div className="inline-notice danger" role="alert">
            {failure}
          </div>
        ) : null}
        <button
          className="primary-button auth-submit"
          type="button"
          disabled={Boolean(issue) || login.isPending}
          onClick={() => {
            setFailure("");
            login.mutate(undefined, {
              onError: (error) => setFailure(errorMessage(error)),
            });
          }}
        >
          {login.isPending ? (
            <LoaderCircle className="spin" size={18} />
          ) : (
            <KeyRound size={18} />
          )}
          {login.isPending
            ? label(locale, "owner_auth.passkey.waiting")
            : label(locale, "owner_auth.passkey.login_button")}
        </button>
        <p className="auth-help">{label(locale, "owner_auth.login.help")}</p>
      </section>
    </main>
  );
}

function TopStatus({
  snapshot,
  locale,
}: {
  snapshot: NodeSnapshot;
  locale: Locale;
}): JSX.Element {
  const status = primaryStatus(snapshot, locale);
  const healthy = status.id === "running";
  return (
    <div
      className={`top-status ${healthy ? "good" : status.id === "degraded" || status.id === "control_unavailable" ? "warn" : "bad"}`}
    >
      <span className="top-status-dot" aria-hidden="true" />
      <div>
        <strong>{status.label}</strong>
        <span>{status.detail}</span>
      </div>
    </div>
  );
}

function LanguageMenu({
  locale,
  preference,
  disabled,
  onChange,
}: {
  locale: Locale;
  preference: LanguagePreference;
  disabled: boolean;
  onChange(preference: LanguagePreference): void;
}): JSX.Element {
  const choices: Array<{ value: LanguagePreference; title: string }> = [
    { value: "system", title: tr(locale, "lang.system", {}) },
    { value: "zh-CN", title: "简体中文" },
    { value: "en", title: "English" },
  ];
  return (
    <div className="language-control">
      <DropdownMenu.Root>
        <DropdownMenu.Trigger asChild>
          <button
            className="toolbar-button"
            type="button"
            disabled={disabled}
            aria-label={label(locale, "common.language")}
          >
            <Globe2 size={16} />
            <span>
              {choices.find((choice) => choice.value === preference)?.title ??
                preference}
            </span>
            <ChevronDown size={14} />
          </button>
        </DropdownMenu.Trigger>
        <DropdownMenu.Portal>
          <DropdownMenu.Content
            className="dropdown-content"
            align="end"
            sideOffset={8}
          >
            <DropdownMenu.Label className="dropdown-label">
              {label(locale, "common.language")}
            </DropdownMenu.Label>
            {choices.map((choice) => (
              <DropdownMenu.Item
                className="dropdown-item"
                key={choice.value}
                onSelect={() => onChange(choice.value)}
              >
                {choice.value === preference ? (
                  <CheckCircle2 size={15} />
                ) : (
                  <span className="menu-spacer" />
                )}
                {choice.title}
              </DropdownMenu.Item>
            ))}
          </DropdownMenu.Content>
        </DropdownMenu.Portal>
      </DropdownMenu.Root>
      {disabled ? (
        <small className="control-reason" role="status">
          {label(locale, "common.control_pending")}
        </small>
      ) : null}
    </div>
  );
}

function AppShell({
  manifest,
  snapshot,
  locale,
  languagePreference,
  eventState,
  browserOnline,
  refreshing,
  lastAuthoritativeAt,
  snapshotFailure,
  uiSession,
  onUiSessionChange,
  queryClient,
}: {
  manifest: NodeUiManifest;
  snapshot: NodeSnapshot;
  locale: Locale;
  languagePreference: LanguagePreference;
  eventState: NodeEventState;
  browserOnline: boolean;
  refreshing: boolean;
  lastAuthoritativeAt: number | null;
  snapshotFailure: unknown;
  uiSession: UiSessionMemo;
  onUiSessionChange(patch: Partial<UiSessionMemo>): void;
  queryClient: QueryClient;
}): JSX.Element {
  const initialPage =
    manifest.pages.find((page) => page.id === "overview") ?? manifest.pages[0];
  const pageId = uiSession.pageId || (initialPage?.id ?? "overview");
  const selectedNetworkState = uiSession.selectedNetwork;
  const searchQuery = uiSession.searchQuery;
  const requestedActionId = uiSession.requestedActionId;
  const [pendingAction, setPendingAction] = useState<string | null>(null);
  const [refreshFailure, setRefreshFailure] = useState<unknown>(null);
  const [manualRefreshPending, setManualRefreshPending] = useState(false);
  const setPageId = (value: string) => onUiSessionChange({ pageId: value });
  const setSelectedNetworkState = (value: string) =>
    onUiSessionChange({ selectedNetwork: value });
  const setSearchQuery = (value: string) =>
    onUiSessionChange({ searchQuery: value });
  const setRequestedActionId = (value: string | null) =>
    onUiSessionChange({ requestedActionId: value });
  const handleConfigurationDraftChange = useCallback(
    (configurationDraft: ConfigurationDraftMemo | null) => {
      onUiSessionChange({ configurationDraft });
    },
    [onUiSessionChange],
  );
  const searchInputRef = useRef<HTMLInputElement>(null);
  const activePage =
    manifest.pages.find((page) => page.id === pageId) ?? initialPage;
  const activeModule = moduleForPage(activePage?.id ?? "overview");
  const selectedNetwork = defaultNetworkName(snapshot, selectedNetworkState);
  const searchResults = useMemo(
    () => searchNodeSnapshot(manifest, snapshot, searchQuery, locale),
    [locale, manifest, searchQuery, snapshot],
  );
  const searchOpen = searchQuery.trim().length > 0;
  const controlRefreshing = refreshing || manualRefreshPending;
  const maintenance =
    isControlMaintenance(snapshotFailure) ||
    isControlMaintenance(refreshFailure);
  const controlFailure = snapshotFailure ?? refreshFailure;
  const pageErrorMessage = controlFailure ? errorMessage(controlFailure) : null;
  const pageSignals = pageStateSignals(snapshot, activePage?.id ?? "overview", {
    browserOnline,
    refreshing: controlRefreshing,
    stale: eventState.stale,
    lastAuthoritativeAt: lastAuthoritativeAt ?? eventState.lastAuthoritativeAt,
    error: maintenance ? null : pageErrorMessage,
    maintenance,
    permissionDenied:
      !snapshot.permissions?.permissions.includes("view_state"),
    unsupported: pageUnsupported(snapshot, activePage?.id ?? ""),
  });
  const pageState = primaryPageState(pageSignals);
  const interactionBlocked = interactionBlockReason(pageSignals);
  const syncMode = eventState.stale
    ? "stale"
    : controlRefreshing
      ? "refreshing"
      : "live";

  useEffect(() => {
    const focusSearch = (event: KeyboardEvent): void => {
      if (!(event.ctrlKey || event.metaKey) || event.key.toLowerCase() !== "k") {
        return;
      }
      event.preventDefault();
      searchInputRef.current?.focus();
    };
    window.addEventListener("keydown", focusSearch);
    return () => window.removeEventListener("keydown", focusSearch);
  }, []);

  const languageMutation = useMutation({
    mutationFn: saveLanguagePreference,
    onSuccess: (preference) => {
      queryClient.setQueryData(LANGUAGE_QUERY_KEY, preference);
    },
  });
  const logoutMutation = useMutation({
    mutationFn: logoutOwnerSession,
    onSuccess: () =>
      queryClient.setQueryData(SESSION_QUERY_KEY, { authenticated: false }),
  });

  async function refresh(): Promise<void> {
    if (manualRefreshPending || refreshing) return;
    setManualRefreshPending(true);
    setRefreshFailure(null);
    try {
      const latest = await loadSnapshot();
      queryClient.setQueryData(SNAPSHOT_QUERY_KEY, latest);
    } catch (error) {
      if (error instanceof AuthenticationError) {
        queryClient.setQueryData(SESSION_QUERY_KEY, { authenticated: false });
        return;
      }
      setRefreshFailure(error);
    } finally {
      setManualRefreshPending(false);
    }
  }

  async function resolveOperationOutcome(
    submitted: NodeOperationSnapshot,
  ): Promise<NodeCommandOutcome> {
    const operation = operationStateIsTerminal(submitted.state)
      ? submitted
      : await waitForOperation(submitted.operation_id);
    if (operation.result) return operation.result.outcome;
    if (operation.state !== "succeeded") {
      const error = operation.error;
      throw new Error(
        error
          ? tr(locale, error.message_key, error.params ?? {})
          : label(locale, "common.operation_failed"),
      );
    }
    throw new Error(label(locale, "common.operation_failed"));
  }

  async function submitAction(
    action: NodeUiAction,
    values: ActionValues,
  ): Promise<NodeCommandOutcome> {
    setPendingAction(action.id);
    try {
      const response = await sendCommand(
        commandEnvelope(
          nextRequestId(),
          action,
          values,
          locale,
          newIdempotencyKey(),
        ),
      );
      const outcome = response.result
        ? response.result.outcome
        : response.operation
          ? await resolveOperationOutcome(response.operation)
          : (() => {
              throw new Error(label(locale, "common.operation_failed"));
            })();
      if (outcome.status === "snapshot") {
        queryClient.setQueryData(SNAPSHOT_QUERY_KEY, outcome.data);
      } else {
        await Promise.all([
          queryClient.invalidateQueries({ queryKey: SNAPSHOT_QUERY_KEY }),
          queryClient.invalidateQueries({ queryKey: MANIFEST_QUERY_KEY }),
        ]);
      }
      return outcome;
    } finally {
      setPendingAction(null);
    }
  }

  function navigate(nextPage: string, networkName?: string): void {
    if (!manifest.pages.some((page) => page.id === nextPage)) return;
    if (
      networkName &&
      snapshot.networks.some((network) => network.name === networkName)
    ) {
      setSelectedNetworkState(networkName);
    }
    setRequestedActionId(null);
    setPageId(nextPage);
    setSearchQuery("");
    document.getElementById("page-content")?.focus();
  }

  function navigateToSearchResult(result: SearchResult): void {
    navigate(result.pageId, result.networkName);
  }

  function clearSearch(focusInput = false): void {
    setSearchQuery("");
    if (focusInput) searchInputRef.current?.focus();
  }

  function requestAction(actionId: string): void {
    setRequestedActionId(actionId);
  }

  function selectModule(module: ModuleDefinition): void {
    const next =
      module.pages.find(
        (id) =>
          id === module.defaultPageId &&
          manifest.pages.some((page) => page.id === id),
      ) ??
      module.pages.find((id) => manifest.pages.some((page) => page.id === id));
    if (next) {
      setPageId(next);
      setSearchQuery("");
    }
  }

  const subpages = activeModule.pages
    .map((id) => manifest.pages.find((page) => page.id === id))
    .filter((page): page is NodeUiPage => Boolean(page));
  const controlPending =
    manualRefreshPending ||
    refreshing ||
    languageMutation.isPending ||
    logoutMutation.isPending;

  return (
    <div className="app-shell">
      <a className="skip-link" href="#page-content">
        {label(locale, "common.skip_content")}
      </a>
      <aside
        className="primary-nav"
        data-ui-region="sidebar"
        aria-label={label(locale, "common.main_navigation")}
      >
        <div className="brand-lockup">
          <div className="brand-mark">
            <Wifi size={22} />
          </div>
          <div>
            <span>fuckipv6</span>
            <strong>{label(locale, "common.node_contract")}</strong>
          </div>
        </div>
        <nav className="module-list">
          {MODULES.map((module) => {
            const Icon = module.icon;
            const selected = module.id === activeModule.id;
            return (
              <button
                key={module.id}
                type="button"
                className={
                  selected ? "module-button selected" : "module-button"
                }
                aria-current={selected ? "page" : undefined}
                onClick={() => selectModule(module)}
              >
                <Icon size={19} />
                <span>{label(locale, module.labelKey)}</span>
              </button>
            );
          })}
        </nav>
        <div className="nav-footer">
          <span>
            {label(locale, "common.snapshot")} #{snapshot.revision}
          </span>
          <span
            className={
              syncMode === "live"
                ? "stale-indicator"
                : "stale-indicator active"
            }
          >
            <span aria-hidden="true" />{" "}
            {syncMode === "stale"
              ? label(locale, "state.stale.title")
              : syncMode === "refreshing"
                ? label(locale, "state.refreshing.title")
                : label(locale, "common.live")}
          </span>
        </div>
      </aside>

      <div className="app-main">
        <header className="global-bar" data-ui-region="topbar">
          <div className="global-context">
            <TopStatus snapshot={snapshot} locale={locale} />
          </div>
          <label className="network-picker">
            <span>{label(locale, "common.current_network")}</span>
            <select
              value={selectedNetwork}
              onChange={(event) => setSelectedNetworkState(event.target.value)}
            >
              {snapshot.networks.length === 0 ? (
                <option value="">
                  {label(locale, "common.no_running_networks")}
                </option>
              ) : null}
              {snapshot.networks.map((network) => (
                <option value={network.name} key={network.name}>
                  {network.name}
                </option>
              ))}
            </select>
          </label>
          <div className="global-actions">
            <LanguageMenu
              locale={locale}
              preference={languagePreference}
              disabled={languageMutation.isPending}
              onChange={(preference) => languageMutation.mutate(preference)}
            />
            <button
              className="icon-button"
              type="button"
              disabled={manualRefreshPending || refreshing}
              onClick={() => void refresh()}
              aria-label={label(locale, "common.refresh")}
            >
              <RefreshCw
                className={
                  manualRefreshPending || refreshing ? "spin" : undefined
                }
                size={17}
              />
            </button>
            <button
              className="icon-button"
              type="button"
              disabled={logoutMutation.isPending}
              onClick={() => logoutMutation.mutate()}
              aria-label={label(locale, "owner_auth.session.logout")}
            >
              {logoutMutation.isPending ? (
                <LoaderCircle className="spin" size={17} />
              ) : (
                <LogOut size={17} />
              )}
            </button>
            {controlPending ? (
              <small className="global-action-reason" role="status">
                {label(locale, "common.control_pending")}
              </small>
            ) : null}
          </div>
        </header>

        <div className="secondary-shell">
          <div className="local-toolbar">
            <div
              className="page-tabs"
              role="tablist"
              aria-label={label(locale, "common.page_sections")}
            >
              {subpages.map((page) => (
                <button
                  key={page.id}
                  type="button"
                  role="tab"
                  aria-selected={page.id === activePage?.id}
                  className={
                    page.id === activePage?.id ? "page-tab selected" : "page-tab"
                  }
                  onClick={() => navigate(page.id)}
                >
                  {label(locale, page.nav_label_key)}
                </button>
              ))}
            </div>
            <div className="local-toolbar-actions">
              <div className="global-search">
                <SearchIcon
                  className="global-search-icon"
                  size={15}
                  aria-hidden="true"
                />
                <input
                  ref={searchInputRef}
                  id="global-search-input"
                  type="text"
                  value={searchQuery}
                  aria-label={label(locale, "common.search")}
                  aria-controls={searchOpen ? "global-search-results" : undefined}
                  aria-expanded={searchOpen}
                  autoComplete="off"
                  spellCheck={false}
                  placeholder={label(locale, "common.search_placeholder")}
                  onChange={(event) => setSearchQuery(event.target.value)}
                  onKeyDown={(event) => {
                    if (event.key === "Escape") clearSearch(true);
                  }}
                />
                {searchOpen ? (
                  <button
                    className="global-search-clear"
                    type="button"
                    aria-label={label(locale, "common.search_clear")}
                    onClick={() => clearSearch(true)}
                  >
                    <X size={14} />
                  </button>
                ) : null}
              </div>
              {syncMode !== "live" ? (
                <span
                  className="sync-badge"
                  data-sync-mode={syncMode}
                  role="status"
                >
                  {syncMode === "stale"
                    ? label(locale, "state.stale.title")
                    : label(locale, "state.refreshing.title")}
                </span>
              ) : null}
            </div>
          </div>

          {searchOpen ? (
            <div className="global-search-panel" id="global-search-results">
              {searchResults.length === 0 ? (
                <div className="global-search-empty" role="status">
                  {label(locale, "common.search_no_results")}
                </div>
              ) : (
                <ul
                  className="global-search-results"
                  role="list"
                  aria-label={label(locale, "common.search_results")}
                >
                  {searchResults.map((result) => (
                    <li id={searchElementId(result)} key={result.id}>
                      <button
                        className="global-search-result"
                        type="button"
                        aria-label={`${label(locale, searchKindKey(result.kind))} · ${result.title} · ${result.detail}`}
                        onClick={() => navigateToSearchResult(result)}
                      >
                        <span className="global-search-result-copy">
                          <strong>{result.title}</strong>
                          <span>{result.detail}</span>
                        </span>
                        <span className="global-search-result-kind">
                          {label(locale, searchKindKey(result.kind))}
                        </span>
                      </button>
                    </li>
                  ))}
                </ul>
              )}
            </div>
          ) : null}

          {!maintenance && pageErrorMessage ? (
            <div className="app-banner danger" role="alert">
              {pageErrorMessage}
            </div>
          ) : null}
        </div>
        <main className="workspace" id="page-content" tabIndex={-1}>
          {activePage ? (
            <>
              <div className="page-content-stack" data-ui-region="content">
                <StateNotice
                  state={pageState}
                  locale={locale}
                  lastAuthoritativeAt={
                    lastAuthoritativeAt ?? eventState.lastAuthoritativeAt
                  }
                />
                <PageContent
                  page={activePage}
                  snapshot={snapshot}
                  locale={locale}
                  selectedNetwork={selectedNetwork}
                  footer={
                    activePage.actions.some((action) =>
                      CONFIGURATION_ACTION_IDS.has(action.id),
                    ) ? (
                      <ConfigurationWorkflow
                        actions={activePage.actions.filter((action) =>
                          CONFIGURATION_ACTION_IDS.has(action.id),
                        )}
                        snapshot={snapshot}
                        selectedNetwork={selectedNetwork}
                        locale={locale}
                        pendingAction={pendingAction}
                        interactionBlocked={interactionBlocked}
                        draft={uiSession.configurationDraft}
                        onDraftChange={handleConfigurationDraftChange}
                        onSubmit={submitAction}
                      />
                    ) : null
                  }
                  onSelectNetwork={setSelectedNetworkState}
                  onNavigate={navigate}
                  onRequestAction={requestAction}
                />
              </div>
              <ActionPanel
                actions={activePage.actions.filter(
                  (action) => !CONFIGURATION_ACTION_IDS.has(action.id),
                )}
                snapshot={snapshot}
                selectedNetwork={selectedNetwork}
                locale={locale}
                pendingAction={pendingAction}
                interactionBlocked={interactionBlocked}
                requestedActionId={requestedActionId}
                onRequestedActionOpened={() => setRequestedActionId(null)}
                onSubmit={submitAction}
              />
            </>
          ) : (
            <div className="empty-state">
              {label(locale, "common.manifest_not_loaded")}
            </div>
          )}
        </main>
      </div>
    </div>
  );
}

export function App(): JSX.Element {
  const queryClient = useQueryClient();
  const sessionQuery = useQuery({
    queryKey: SESSION_QUERY_KEY,
    queryFn: loadOwnerSession,
    retry: false,
    staleTime: 30_000,
  });
  const authenticated = sessionQuery.data?.authenticated === true;
  const languageQuery = useQuery({
    queryKey: LANGUAGE_QUERY_KEY,
    queryFn: loadLanguagePreference,
    enabled: authenticated,
    retry: false,
  });
  const manifestQuery = useQuery({
    queryKey: MANIFEST_QUERY_KEY,
    queryFn: loadManifest,
    enabled: authenticated,
    retry: false,
  });
  const snapshotQuery = useQuery({
    queryKey: SNAPSHOT_QUERY_KEY,
    queryFn: loadSnapshot,
    enabled: authenticated,
    retry: false,
    placeholderData: (previous) => previous,
  });
  const eventState = useNodeEvents(authenticated, queryClient);
  const browserOnline = useBrowserOnline();
  const [uiSession, setUiSession] = useState<UiSessionMemo>(EMPTY_UI_SESSION);
  const updateUiSession = useCallback((patch: Partial<UiSessionMemo>) => {
    setUiSession((current) => ({ ...current, ...patch }));
  }, []);
  const locale = useMemo(
    () => resolveLocale(languageQuery.data),
    [languageQuery.data],
  );
  if (sessionQuery.isPending) return <LoadingScreen locale={locale} />;
  if (
    sessionQuery.error instanceof AuthenticationError ||
    sessionQuery.data?.authenticated === false
  ) {
    return <LoginScreen locale={locale} />;
  }
  if (sessionQuery.error) {
    return (
      <FatalScreen
        locale={locale}
        title={label(locale, "owner_auth.session.auth_failed")}
        detail={errorMessage(sessionQuery.error)}
        onRetry={() => void sessionQuery.refetch()}
      />
    );
  }
  if (
    manifestQuery.error instanceof AuthenticationError ||
    snapshotQuery.error instanceof AuthenticationError
  ) {
    return <LoginScreen locale={locale} />;
  }
  if (
    (manifestQuery.error || snapshotQuery.error) &&
    (!manifestQuery.data || !snapshotQuery.data)
  ) {
    return (
      <FatalScreen
        locale={locale}
        title={label(locale, "common.operation_failed")}
        detail={errorMessage(manifestQuery.error ?? snapshotQuery.error)}
        onRetry={() => {
          void manifestQuery.refetch();
          void snapshotQuery.refetch();
        }}
      />
    );
  }
  if (manifestQuery.isPending || snapshotQuery.isPending)
    return <AuthenticatedLoadingShell locale={locale} />;
  if (!manifestQuery.data || !snapshotQuery.data) {
    return <AuthenticatedLoadingShell locale={locale} />;
  }
  return (
    <AppShell
      manifest={manifestQuery.data}
      snapshot={snapshotQuery.data}
      locale={locale}
      languagePreference={languageQuery.data ?? "system"}
      eventState={eventState}
      browserOnline={browserOnline}
      refreshing={snapshotQuery.isFetching && Boolean(snapshotQuery.data)}
      lastAuthoritativeAt={
        snapshotQuery.dataUpdatedAt > 0 ? snapshotQuery.dataUpdatedAt : null
      }
      snapshotFailure={snapshotQuery.error}
      uiSession={uiSession}
      onUiSessionChange={updateUiSession}
      queryClient={queryClient}
    />
  );
}
