import * as DropdownMenu from "@radix-ui/react-dropdown-menu";
import { useMutation, useQuery, useQueryClient, type QueryClient } from "@tanstack/react-query";
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
  ServerCog,
  Settings as SettingsIcon,
  ShieldAlert,
  Wifi,
} from "lucide-react";
import { useEffect, useMemo, useState, type JSX } from "react";
import type {
  NodeCommandOutcome,
  NodeSnapshot,
  NodeUiAction,
  NodeUiManifest,
  NodeUiPage,
} from "../../contracts/typescript/node-contract-v1";
import { AuthenticationError, loadManifest, loadSnapshot, newIdempotencyKey, nextRequestId, sendCommand } from "./api/client";
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
import { NAVIGATION_MODULES, navigationModuleForPage, type NavigationModuleContract } from "./navigation";
import { ActionPanel } from "./components/action-panel";
import { PageContent } from "./components/page-content";
import {
  commandEnvelope,
  defaultNetworkName,
  label,
  primaryStatus,
  validateSnapshot,
  type ActionValues,
} from "./contract/invariants";

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

const MODULES: ModuleDefinition[] = NAVIGATION_MODULES.map((module: NavigationModuleContract) => ({
  id: module.id,
  labelKey: module.labelKey,
  defaultPageId: module.defaultPageId,
  pages: module.pageIds,
  icon: MODULE_ICONS[module.id] ?? Globe2,
}));

const SESSION_QUERY_KEY = ["owner-session"] as const;
const LANGUAGE_QUERY_KEY = ["language-preference"] as const;
const MANIFEST_QUERY_KEY = ["node-manifest", 1] as const;
const SNAPSHOT_QUERY_KEY = ["node-snapshot", 1] as const;

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
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

function useNodeEvents(authenticated: boolean, queryClient: QueryClient): boolean {
  const [stale, setStale] = useState(false);

  useEffect(() => {
    if (!authenticated) return;
    let socket: WebSocket | null = null;
    let reconnectTimer: number | null = null;
    let stopped = false;

    const queueResync = () => {
      setStale(true);
      void queryClient.invalidateQueries({ queryKey: SNAPSHOT_QUERY_KEY });
    };

    const connect = () => {
      if (stopped) return;
      const protocol = window.location.protocol === "https:" ? "wss:" : "ws:";
      socket = new WebSocket(`${protocol}//${window.location.host}/api/node/events?schema_version=1`);
      socket.addEventListener("open", () => setStale(false));
      socket.addEventListener("message", (event) => {
        try {
          const message = JSON.parse(String(event.data)) as Record<string, unknown>;
          if (message.schema_version !== 1 || typeof message.type !== "string") {
            queueResync();
            return;
          }
          if (message.type === "snapshot" || message.type === "resync") {
            const snapshot = validateSnapshot(message.snapshot as NodeSnapshot);
            queryClient.setQueryData(SNAPSHOT_QUERY_KEY, snapshot);
            setStale(false);
            return;
          }
          if (message.type === "event") {
            void queryClient.invalidateQueries({ queryKey: SNAPSHOT_QUERY_KEY });
            return;
          }
          if (message.type === "error") queueResync();
        } catch {
          queueResync();
        }
      });
      socket.addEventListener("close", () => {
        setStale(true);
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

  return stale;
}

function LoadingScreen({ locale }: { locale: Locale }): JSX.Element {
  return (
    <main className="centered-shell" aria-busy="true">
      <div className="loading-mark"><LoaderCircle className="spin" size={30} /></div>
      <h1>{label(locale, "owner_auth.session.checking")}</h1>
    </main>
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
      <div className="state-icon danger"><AlertCircle size={28} /></div>
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
    onSuccess: (session) => queryClient.setQueryData(SESSION_QUERY_KEY, session),
  });
  const [failure, setFailure] = useState("");

  return (
    <main className="auth-shell">
      <section className="auth-card" aria-labelledby="auth-title">
        <div className="auth-emblem"><ShieldAlert size={32} /></div>
        <span className="eyebrow">fuckipv6</span>
        <h1 id="auth-title">{label(locale, "owner_auth.login.title")}</h1>
        <p>{label(locale, "owner_auth.login.description")}</p>
        {issue ? (
          <div className="inline-notice warning" role="alert">
            <AlertCircle size={16} />
            <span>{issue}</span>
          </div>
        ) : null}
        {failure ? <div className="inline-notice danger" role="alert">{failure}</div> : null}
        <button
          className="primary-button auth-submit"
          type="button"
          disabled={Boolean(issue) || login.isPending}
          onClick={() => {
            setFailure("");
            login.mutate(undefined, { onError: (error) => setFailure(errorMessage(error)) });
          }}
        >
          {login.isPending ? <LoaderCircle className="spin" size={18} /> : <KeyRound size={18} />}
          {login.isPending
            ? label(locale, "owner_auth.passkey.waiting")
            : label(locale, "owner_auth.passkey.login_button")}
        </button>
        <p className="auth-help">{label(locale, "owner_auth.login.help")}</p>
      </section>
    </main>
  );
}

function TopStatus({ snapshot, locale }: { snapshot: NodeSnapshot; locale: Locale }): JSX.Element {
  const status = primaryStatus(snapshot, locale);
  const healthy = status.id === "running";
  return (
    <div className={`top-status ${healthy ? "good" : status.id === "degraded" || status.id === "control_unavailable" ? "warn" : "bad"}`}>
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
    <DropdownMenu.Root>
      <DropdownMenu.Trigger asChild>
        <button className="toolbar-button" type="button" disabled={disabled} aria-label={label(locale, "common.language")}>
          <Globe2 size={16} />
          <span>{choices.find((choice) => choice.value === preference)?.title ?? preference}</span>
          <ChevronDown size={14} />
        </button>
      </DropdownMenu.Trigger>
      <DropdownMenu.Portal>
        <DropdownMenu.Content className="dropdown-content" align="end" sideOffset={8}>
          <DropdownMenu.Label className="dropdown-label">{label(locale, "common.language")}</DropdownMenu.Label>
          {choices.map((choice) => (
            <DropdownMenu.Item
              className="dropdown-item"
              key={choice.value}
              onSelect={() => onChange(choice.value)}
            >
              {choice.value === preference ? <CheckCircle2 size={15} /> : <span className="menu-spacer" />}
              {choice.title}
            </DropdownMenu.Item>
          ))}
        </DropdownMenu.Content>
      </DropdownMenu.Portal>
    </DropdownMenu.Root>
  );
}

function AppShell({
  manifest,
  snapshot,
  locale,
  languagePreference,
  stale,
  queryClient,
}: {
  manifest: NodeUiManifest;
  snapshot: NodeSnapshot;
  locale: Locale;
  languagePreference: LanguagePreference;
  stale: boolean;
  queryClient: QueryClient;
}): JSX.Element {
  const initialPage = manifest.pages.find((page) => page.id === "overview") ?? manifest.pages[0];
  const [pageId, setPageId] = useState(initialPage?.id ?? "overview");
  const [selectedNetworkState, setSelectedNetworkState] = useState("");
  const [pendingAction, setPendingAction] = useState<string | null>(null);
  const [refreshError, setRefreshError] = useState("");
  const activePage = manifest.pages.find((page) => page.id === pageId) ?? initialPage;
  const activeModule = moduleForPage(activePage?.id ?? "overview");
  const selectedNetwork = defaultNetworkName(snapshot, selectedNetworkState);

  const languageMutation = useMutation({
    mutationFn: saveLanguagePreference,
    onSuccess: (preference) => {
      queryClient.setQueryData(LANGUAGE_QUERY_KEY, preference);
    },
  });
  const logoutMutation = useMutation({
    mutationFn: logoutOwnerSession,
    onSuccess: () => queryClient.setQueryData(SESSION_QUERY_KEY, { authenticated: false }),
  });

  async function refresh(): Promise<void> {
    setRefreshError("");
    try {
      const latest = await loadSnapshot();
      queryClient.setQueryData(SNAPSHOT_QUERY_KEY, latest);
    } catch (error) {
      if (error instanceof AuthenticationError) {
        queryClient.setQueryData(SESSION_QUERY_KEY, { authenticated: false });
        return;
      }
      setRefreshError(errorMessage(error));
    }
  }

  async function submitAction(action: NodeUiAction, values: ActionValues): Promise<NodeCommandOutcome> {
    setPendingAction(action.id);
    try {
      const response = await sendCommand(commandEnvelope(nextRequestId(), action, values, locale, newIdempotencyKey()));
      if (!response.result) throw new Error(label(locale, "common.operation_failed"));
      const outcome = response.result.outcome;
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

  function navigate(nextPage: string): void {
    if (!manifest.pages.some((page) => page.id === nextPage)) return;
    setPageId(nextPage);
    document.getElementById("page-content")?.focus();
  }

  function selectModule(module: ModuleDefinition): void {
    const next =
      module.pages.find(
        (id) => id === module.defaultPageId && manifest.pages.some((page) => page.id === id),
      ) ?? module.pages.find((id) => manifest.pages.some((page) => page.id === id));
    if (next) setPageId(next);
  }

  const subpages = activeModule.pages
    .map((id) => manifest.pages.find((page) => page.id === id))
    .filter((page): page is NodeUiPage => Boolean(page));

  return (
    <div className="app-shell">
      <a className="skip-link" href="#page-content">{label(locale, "common.skip_content")}</a>
      <aside
        className="primary-nav"
        data-ui-region="sidebar"
        aria-label={label(locale, "common.main_navigation")}
      >
        <div className="brand-lockup">
          <div className="brand-mark"><Wifi size={22} /></div>
          <div><span>fuckipv6</span><strong>{label(locale, "common.node_contract")}</strong></div>
        </div>
        <nav className="module-list">
          {MODULES.map((module) => {
            const Icon = module.icon;
            const selected = module.id === activeModule.id;
            return (
              <button
                key={module.id}
                type="button"
                className={selected ? "module-button selected" : "module-button"}
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
          <span>{label(locale, "common.snapshot")} #{snapshot.revision}</span>
          <span className={stale ? "stale-indicator active" : "stale-indicator"}>
            <span aria-hidden="true" /> {stale ? label(locale, "common.syncing") : "Live"}
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
            <select value={selectedNetwork} onChange={(event) => setSelectedNetworkState(event.target.value)}>
              {snapshot.networks.length === 0 ? <option value="">{label(locale, "common.no_running_networks")}</option> : null}
              {snapshot.networks.map((network) => <option value={network.name} key={network.name}>{network.name}</option>)}
            </select>
          </label>
          <div className="global-actions">
            <LanguageMenu
              locale={locale}
              preference={languagePreference}
              disabled={languageMutation.isPending}
              onChange={(preference) => languageMutation.mutate(preference)}
            />
            <button className="icon-button" type="button" onClick={() => void refresh()} aria-label={label(locale, "common.refresh")}>
              <RefreshCw size={17} />
            </button>
            <button className="icon-button" type="button" onClick={() => logoutMutation.mutate()} aria-label={label(locale, "owner_auth.session.logout")}>
              <LogOut size={17} />
            </button>
          </div>
        </header>

        <div className="local-toolbar">
          <div className="page-tabs" role="tablist" aria-label={label(locale, "common.page_sections")}>
            {subpages.map((page) => (
              <button
                key={page.id}
                type="button"
                role="tab"
                aria-selected={page.id === activePage?.id}
                className={page.id === activePage?.id ? "page-tab selected" : "page-tab"}
                onClick={() => navigate(page.id)}
              >
                {label(locale, page.nav_label_key)}
              </button>
            ))}
          </div>
          {stale ? <span className="sync-badge">{label(locale, "common.syncing")}</span> : null}
        </div>

        {refreshError ? <div className="app-banner danger" role="alert">{refreshError}</div> : null}
        <main className="workspace" id="page-content" tabIndex={-1}>
          {activePage ? (
            <>
              <PageContent
                page={activePage}
                snapshot={snapshot}
                locale={locale}
                selectedNetwork={selectedNetwork}
                onSelectNetwork={setSelectedNetworkState}
                onNavigate={navigate}
              />
              <ActionPanel
                actions={activePage.actions}
                snapshot={snapshot}
                selectedNetwork={selectedNetwork}
                locale={locale}
                pendingAction={pendingAction}
                onSubmit={submitAction}
              />
            </>
          ) : (
            <div className="empty-state">{label(locale, "common.manifest_not_loaded")}</div>
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
  const stale = useNodeEvents(authenticated, queryClient);
  const locale = useMemo(
    () => resolveLocale(languageQuery.data),
    [languageQuery.data],
  );

  if (sessionQuery.isPending) return <LoadingScreen locale={locale} />;
  if (sessionQuery.error instanceof AuthenticationError || sessionQuery.data?.authenticated === false) {
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
  if (manifestQuery.isPending || snapshotQuery.isPending) return <LoadingScreen locale={locale} />;
  if (manifestQuery.error instanceof AuthenticationError || snapshotQuery.error instanceof AuthenticationError) {
    return <LoginScreen locale={locale} />;
  }
  if (manifestQuery.error || snapshotQuery.error) {
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
  if (!manifestQuery.data || !snapshotQuery.data) {
    return <LoadingScreen locale={locale} />;
  }
  return (
    <AppShell
      manifest={manifestQuery.data}
      snapshot={snapshotQuery.data}
      locale={locale}
      languagePreference={languageQuery.data ?? "system"}
      stale={stale || snapshotQuery.isFetching}
      queryClient={queryClient}
    />
  );
}
