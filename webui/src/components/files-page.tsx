import {
  AlertTriangle,
  ArrowDownToLine,
  ArrowLeft,
  ArrowUpFromLine,
  File as FileIcon,
  Folder,
  FolderOpen,
  HardDrive,
  LoaderCircle,
  Pause,
  Play,
  RefreshCw,
  Trash2,
  Upload,
  Users,
} from "lucide-react";
import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ChangeEvent,
  type JSX,
} from "react";
import type { NodeSnapshot } from "../../../contracts/typescript/node-contract-v1";
import {
  AuthenticationError,
  HttpError,
  apiRequest,
  requestJson,
} from "../api/client";
import { label } from "../contract/invariants";
import { tr } from "../i18n";

const FILE_TARGETS_PATH = "/api/v1/file/targets";
const FILE_DIRECTORY_PATH = "/api/v1/file/directory";
const FILE_TRANSFERS_PATH = "/api/v1/file/transfers";
const FILE_UPLOADS_PATH = "/api/v1/file/uploads";
const FILE_DOWNLOADS_PATH = "/api/v1/file/downloads";
const DIRECTORY_PAGE_LIMIT = 100;
const MAX_WEB_FILE_BYTES = 1024 * 1024 * 1024;
const ACTIVE_TRANSFER_POLL_MS = 750;
const IDLE_TRANSFER_POLL_MS = 2_000;

type FilePeerTarget = {
  peer: string;
  network_id: string;
  network_name: string;
  share_id: string;
};

type FileDirectoryEntry = {
  path: string;
  is_directory: boolean;
  size: number;
  modified_at_unix_ms?: number | null;
};

type FileDirectoryListing = {
  entries: FileDirectoryEntry[];
  next_cursor?: string | null;
  truncated: boolean;
};

type FileTransferDirection = "upload" | "download";
type FileTransferState =
  | "queued"
  | "active"
  | "paused"
  | "verifying"
  | "completed"
  | "failed"
  | "cancelled"
  | "expired";

type FileTransferStatus = {
  transfer_id: string;
  operation_id: string;
  direction: FileTransferDirection;
  state: FileTransferState;
  transferred_bytes: number;
  total_bytes: number;
  received_chunks: number;
  total_chunks: number;
  error_code?: string | null;
  failure_kind?: string | null;
  updated_at_unix_ms: number;
};

type StartedFileTransfer = {
  transfer_id: string;
  operation_id: string;
  size_bytes: number;
};

type PendingDownload = {
  token: string;
  transfer_id: string;
  operation_id: string;
  expires_at_unix_ms: number;
};

type FileError = {
  titleKey: string;
  detail: string;
};

type LoadState = "idle" | "loading" | "ready" | "error";

export type FilesPageProps = {
  snapshot: NodeSnapshot;
  locale: Locale;
  selectedNetwork: string;
};

function targetKey(target: FilePeerTarget): string {
  return `${target.network_id}\u0000${target.peer}`;
}

function entryName(path: string): string {
  return path.split("/").at(-1) ?? path;
}

function childPath(directory: string, name: string): string | null {
  if (
    !name ||
    name === "." ||
    name === ".." ||
    /[\\/:]/.test(name) ||
    name.endsWith(".") ||
    name.endsWith(" ") ||
    [...name].some((character) => {
      const code = character.codePointAt(0) ?? 0;
      return code < 0x20 || code === 0x7f;
    })
  ) {
    return null;
  }
  return directory ? `${directory}/${name}` : name;
}

function pathSegments(path: string): Array<{ name: string; path: string }> {
  const segments: Array<{ name: string; path: string }> = [];
  let current = "";
  for (const name of path.split("/").filter(Boolean)) {
    current = current ? `${current}/${name}` : name;
    segments.push({ name, path: current });
  }
  return segments;
}

function formatBytes(locale: Locale, value: number): string {
  try {
    return window.FuckIPV6I18n?.formatBytes(locale, value) ?? `${value} B`;
  } catch {
    return `${value} B`;
  }
}

function formatTimestamp(locale: Locale, unixMs: number): string {
  try {
    return (
      window.FuckIPV6I18n?.formatTimestamp(locale, unixMs) ??
      new Date(unixMs).toISOString()
    );
  } catch {
    return new Date(unixMs).toISOString();
  }
}

function transferStateKey(state: FileTransferState): string {
  return `file.page.transfer.state.${state}`;
}

function transferStateTone(state: FileTransferState): string {
  if (state === "completed") return "good";
  if (state === "failed" || state === "expired") return "bad";
  if (state === "active" || state === "queued" || state === "verifying") {
    return "warn";
  }
  return "neutral";
}

function transferProgress(status: FileTransferStatus): number {
  if (status.total_bytes > 0) {
    return Math.min(100, (status.transferred_bytes / status.total_bytes) * 100);
  }
  return status.state === "completed" ? 100 : 0;
}

function isActiveTransfer(status: FileTransferStatus): boolean {
  return !["completed", "failed", "cancelled", "expired"].includes(
    status.state,
  );
}

function serverDetail(error: unknown): string | null {
  if (!(error instanceof HttpError)) return null;
  const payload = error.payload as unknown;
  if (typeof payload !== "object" || payload === null) return null;
  const record = payload as Record<string, unknown>;
  if (typeof record.error === "string") return record.error;
  const problem = record.problem;
  if (typeof problem === "object" && problem !== null) {
    const message = (problem as Record<string, unknown>).message;
    if (typeof message === "string") return message;
  }
  if (typeof record.message === "string") return record.message;
  return null;
}

function fileError(locale: Locale, error: unknown): FileError {
  if (
    error instanceof AuthenticationError ||
    (error instanceof HttpError && error.status === 401)
  ) {
    return {
      titleKey: "file.page.error.permission",
      detail: label(locale, "file.page.error.permission_detail"),
    };
  }
  if (error instanceof HttpError) {
    if (error.status === 403) {
      return {
        titleKey: "file.page.error.permission",
        detail: label(locale, "file.page.error.permission_detail"),
      };
    }
    if (error.status === 422) {
      return {
        titleKey: "file.page.error.unsupported",
        detail: label(locale, "file.page.error.unsupported_detail"),
      };
    }
    if (error.status === 429) {
      return {
        titleKey: "file.page.error.rate_limited",
        detail: label(locale, "file.page.error.rate_limited_detail"),
      };
    }
    if (error.status === 503 || error.status === 504) {
      return {
        titleKey: "file.page.error.unavailable",
        detail: label(locale, "file.page.error.unavailable_detail"),
      };
    }
  }
  return {
    titleKey: "file.page.error.generic",
    detail:
      serverDetail(error) ??
      (error instanceof Error ? error.message : String(error)),
  };
}

function fileStatusMessage(
  locale: Locale,
  state: LoadState,
  error: FileError | null,
): JSX.Element | null {
  if (state === "loading") {
    return (
      <div className="loading-panel" role="status">
        <LoaderCircle className="spin" size={18} />
        <div>
          <strong>{label(locale, "file.page.loading")}</strong>
          <p>{label(locale, "file.page.loading_detail")}</p>
        </div>
      </div>
    );
  }
  if (state === "error" && error) {
    return (
      <div className="file-error" role="alert">
        <AlertTriangle size={18} />
        <div>
          <strong>{label(locale, error.titleKey)}</strong>
          <p>{error.detail}</p>
        </div>
      </div>
    );
  }
  return null;
}

function parseTargets(payload: unknown): FilePeerTarget[] {
  if (!Array.isArray(payload))
    throw new Error("File targets response is not an array");
  return payload.map((value) => {
    if (typeof value !== "object" || value === null) {
      throw new Error("File target is invalid");
    }
    const target = value as Record<string, unknown>;
    if (
      typeof target.peer !== "string" ||
      typeof target.network_id !== "string" ||
      typeof target.network_name !== "string" ||
      typeof target.share_id !== "string"
    ) {
      throw new Error("File target fields are invalid");
    }
    return {
      peer: target.peer,
      network_id: target.network_id,
      network_name: target.network_name,
      share_id: target.share_id,
    };
  });
}

function parseDirectory(payload: unknown): FileDirectoryListing {
  if (typeof payload !== "object" || payload === null) {
    throw new Error("File directory response is invalid");
  }
  const value = payload as Record<string, unknown>;
  if (!Array.isArray(value.entries) || typeof value.truncated !== "boolean") {
    throw new Error("File directory fields are invalid");
  }
  const entries = value.entries.map((entry) => {
    if (typeof entry !== "object" || entry === null) {
      throw new Error("File directory entry is invalid");
    }
    const item = entry as Record<string, unknown>;
    if (
      typeof item.path !== "string" ||
      typeof item.is_directory !== "boolean" ||
      typeof item.size !== "number"
    ) {
      throw new Error("File directory entry fields are invalid");
    }
    return {
      path: item.path,
      is_directory: item.is_directory,
      size: item.size,
      modified_at_unix_ms:
        typeof item.modified_at_unix_ms === "number"
          ? item.modified_at_unix_ms
          : null,
    };
  });
  return {
    entries,
    next_cursor:
      typeof value.next_cursor === "string" ? value.next_cursor : null,
    truncated: value.truncated,
  };
}

function parseTransfers(payload: unknown): FileTransferStatus[] {
  if (!Array.isArray(payload))
    throw new Error("File transfers response is not an array");
  return payload.map((entry) => {
    if (typeof entry !== "object" || entry === null) {
      throw new Error("File transfer is invalid");
    }
    const value = entry as Record<string, unknown>;
    if (
      typeof value.transfer_id !== "string" ||
      typeof value.operation_id !== "string" ||
      !["upload", "download"].includes(String(value.direction)) ||
      typeof value.state !== "string" ||
      typeof value.transferred_bytes !== "number" ||
      typeof value.total_bytes !== "number" ||
      typeof value.received_chunks !== "number" ||
      typeof value.total_chunks !== "number" ||
      typeof value.updated_at_unix_ms !== "number"
    ) {
      throw new Error("File transfer fields are invalid");
    }
    return {
      transfer_id: value.transfer_id,
      operation_id: value.operation_id,
      direction: value.direction as FileTransferDirection,
      state: value.state as FileTransferState,
      transferred_bytes: value.transferred_bytes,
      total_bytes: value.total_bytes,
      received_chunks: value.received_chunks,
      total_chunks: value.total_chunks,
      error_code:
        typeof value.error_code === "string" ? value.error_code : null,
      failure_kind:
        typeof value.failure_kind === "string" ? value.failure_kind : null,
      updated_at_unix_ms: value.updated_at_unix_ms,
    };
  });
}

function targetQuery(target: FilePeerTarget): URLSearchParams {
  return new URLSearchParams({
    peer: target.peer,
    network_id: target.network_id,
    share_id: target.share_id,
  });
}

function newDownloadAnchor(url: string, filename: string): HTMLAnchorElement {
  const anchor = document.createElement("a");
  anchor.href = url;
  anchor.download = filename;
  anchor.rel = "noopener";
  anchor.hidden = true;
  document.body.append(anchor);
  anchor.click();
  anchor.remove();
  return anchor;
}

export function FilesPage({
  snapshot,
  locale,
  selectedNetwork,
}: FilesPageProps): JSX.Element {
  const [targets, setTargets] = useState<FilePeerTarget[]>([]);
  const [targetsState, setTargetsState] = useState<LoadState>("loading");
  const [targetsError, setTargetsError] = useState<FileError | null>(null);
  const [selectedTargetKey, setSelectedTargetKey] = useState("");
  const [directoryPath, setDirectoryPath] = useState("");
  const [listing, setListing] = useState<FileDirectoryListing | null>(null);
  const [directoryState, setDirectoryState] = useState<LoadState>("idle");
  const [directoryError, setDirectoryError] = useState<FileError | null>(null);
  const [directoryRefresh, setDirectoryRefresh] = useState(0);
  const [loadingMore, setLoadingMore] = useState(false);
  const [transfers, setTransfers] = useState<FileTransferStatus[]>([]);
  const [transfersState, setTransfersState] = useState<LoadState>("idle");
  const [transfersError, setTransfersError] = useState<FileError | null>(null);
  const [transferRefresh, setTransferRefresh] = useState(0);
  const [controlBusy, setControlBusy] = useState<string | null>(null);
  const [uploadFile, setUploadFile] = useState<File | null>(null);
  const [uploading, setUploading] = useState(false);
  const [uploadNotice, setUploadNotice] = useState<string | null>(null);
  const [uploadError, setUploadError] = useState<FileError | null>(null);
  const [downloadingPath, setDownloadingPath] = useState<string | null>(null);
  const [downloadError, setDownloadError] = useState<FileError | null>(null);
  const uploadInputRef = useRef<HTMLInputElement>(null);

  const selectedTarget = useMemo(
    () =>
      targets.find((target) => targetKey(target) === selectedTargetKey) ?? null,
    [selectedTargetKey, targets],
  );
  const localFileState =
    snapshot.file_capability?.state ??
    snapshot.capabilities?.capabilities.find(
      (capability) => capability.kind === "file",
    )?.state ??
    "unknown";
  const activeTransfers = transfers.filter(isActiveTransfer).length;

  const loadTargets = useCallback(
    async (signal: AbortSignal) => {
      setTargetsState("loading");
      setTargetsError(null);
      try {
        const payload = await apiRequest<unknown>(FILE_TARGETS_PATH, {
          signal,
        });
        const nextTargets = parseTargets(payload);
        setTargets(nextTargets);
        setSelectedTargetKey((current) => {
          if (nextTargets.some((target) => targetKey(target) === current)) {
            return current;
          }
          const preferred = nextTargets.find(
            (target) =>
              target.network_name === selectedNetwork ||
              target.network_id === selectedNetwork,
          );
          return nextTargets.length
            ? targetKey(preferred ?? nextTargets[0])
            : "";
        });
        setTargetsState("ready");
      } catch (error) {
        if (signal.aborted) return;
        setTargets([]);
        setSelectedTargetKey("");
        setTargetsError(fileError(locale, error));
        setTargetsState("error");
      }
    },
    [locale, selectedNetwork],
  );

  useEffect(() => {
    const controller = new AbortController();
    const timer = window.setTimeout(() => {
      void loadTargets(controller.signal);
    }, 0);
    return () => {
      window.clearTimeout(timer);
      controller.abort();
    };
  }, [loadTargets]);

  useEffect(() => {
    if (!selectedTarget) return;
    const controller = new AbortController();
    const query = targetQuery(selectedTarget);
    query.set("path", directoryPath);
    query.set("limit", String(DIRECTORY_PAGE_LIMIT));
    void apiRequest<unknown>(`${FILE_DIRECTORY_PATH}?${query}`, {
      signal: controller.signal,
    })
      .then((payload) => {
        if (controller.signal.aborted) return;
        setListing(parseDirectory(payload));
        setDirectoryError(null);
        setDirectoryState("ready");
      })
      .catch((error: unknown) => {
        if (controller.signal.aborted) return;
        setListing(null);
        setDirectoryError(fileError(locale, error));
        setDirectoryState("error");
      });
    return () => controller.abort();
  }, [directoryPath, directoryRefresh, locale, selectedTarget]);

  const loadTransfers = useCallback(
    async (signal: AbortSignal) => {
      try {
        const payload = await apiRequest<unknown>(FILE_TRANSFERS_PATH, {
          signal,
        });
        const next = parseTransfers(payload);
        setTransfers(next);
        setTransfersState("ready");
        setTransfersError(null);
        return next;
      } catch (error) {
        if (signal.aborted) throw error;
        setTransfersError(fileError(locale, error));
        setTransfersState("error");
        throw error;
      }
    },
    [locale],
  );

  useEffect(() => {
    const controller = new AbortController();
    let timer: number | null = null;
    let stopped = false;
    const poll = async () => {
      let next: FileTransferStatus[] | null = null;
      try {
        next = await loadTransfers(controller.signal);
      } catch {
        // Keep the stable error state visible and retry without spamming.
      }
      if (stopped) return;
      const delay = next?.some(isActiveTransfer)
        ? ACTIVE_TRANSFER_POLL_MS
        : IDLE_TRANSFER_POLL_MS;
      timer = window.setTimeout(() => void poll(), delay);
    };
    void poll();
    return () => {
      stopped = true;
      controller.abort();
      if (timer !== null) window.clearTimeout(timer);
    };
  }, [loadTransfers, transferRefresh]);

  const selectTarget = (key: string) => {
    setSelectedTargetKey(key);
    setDirectoryPath("");
    setListing(null);
    setDirectoryState("loading");
    setDirectoryError(null);
    setUploadFile(null);
    setUploadNotice(null);
    setUploadError(null);
    setDownloadError(null);
    if (uploadInputRef.current) uploadInputRef.current.value = "";
  };

  const navigateDirectory = (path: string) => {
    if (path === directoryPath) return;
    setDirectoryPath(path);
    setListing(null);
    setDirectoryState("loading");
    setDirectoryError(null);
  };

  const loadMore = async () => {
    if (!selectedTarget || !listing?.next_cursor || loadingMore) return;
    const query = targetQuery(selectedTarget);
    query.set("path", directoryPath);
    query.set("limit", String(DIRECTORY_PAGE_LIMIT));
    query.set("cursor", listing.next_cursor);
    setLoadingMore(true);
    setDirectoryError(null);
    try {
      const page = parseDirectory(
        await apiRequest<unknown>(`${FILE_DIRECTORY_PATH}?${query}`),
      );
      setListing((current) =>
        current
          ? {
              ...page,
              entries: [...current.entries, ...page.entries],
            }
          : page,
      );
      setDirectoryState("ready");
    } catch (error) {
      setDirectoryError(fileError(locale, error));
    } finally {
      setLoadingMore(false);
    }
  };

  const refreshDirectory = () => {
    setDirectoryState("loading");
    setDirectoryError(null);
    setDirectoryRefresh((value) => value + 1);
  };

  const refreshTransfers = () => {
    setTransferRefresh((value) => value + 1);
  };

  const uploadSelectedFile = async () => {
    if (!selectedTarget || !uploadFile || uploading) return;
    if (uploadFile.size > MAX_WEB_FILE_BYTES) {
      setUploadError({
        titleKey: "file.page.error.upload_too_large",
        detail: tr(locale, "file.page.error.upload_too_large_detail", {
          limit: formatBytes(locale, MAX_WEB_FILE_BYTES),
        }),
      });
      return;
    }
    const path = childPath(directoryPath, uploadFile.name);
    if (!path) {
      setUploadError({
        titleKey: "file.page.error.invalid_filename",
        detail: label(locale, "file.page.error.invalid_filename_detail"),
      });
      return;
    }
    const query = targetQuery(selectedTarget);
    query.set("path", path);
    setUploading(true);
    setUploadError(null);
    setUploadNotice(null);
    try {
      const { response, payload } = await requestJson<StartedFileTransfer>(
        `${FILE_UPLOADS_PATH}?${query}`,
        {
          method: "POST",
          headers: { "Content-Type": "application/octet-stream" },
          body: uploadFile,
        },
      );
      if (!response.ok || payload === null) {
        throw new HttpError(response.status, null);
      }
      setUploadNotice(
        tr(locale, "file.page.upload_started", {
          name: entryName(path),
        }),
      );
      setUploadFile(null);
      if (uploadInputRef.current) uploadInputRef.current.value = "";
      refreshDirectory();
      refreshTransfers();
    } catch (error) {
      setUploadError(fileError(locale, error));
    } finally {
      setUploading(false);
    }
  };

  const downloadEntry = async (entry: FileDirectoryEntry) => {
    if (!selectedTarget || entry.is_directory || downloadingPath) return;
    if (entry.size > MAX_WEB_FILE_BYTES) {
      setDownloadError({
        titleKey: "file.page.error.download_too_large",
        detail: tr(locale, "file.page.error.download_too_large_detail", {
          limit: formatBytes(locale, MAX_WEB_FILE_BYTES),
        }),
      });
      return;
    }
    setDownloadingPath(entry.path);
    setDownloadError(null);
    try {
      const pending = await apiRequest<PendingDownload>(FILE_DOWNLOADS_PATH, {
        method: "POST",
        body: JSON.stringify({
          peer: selectedTarget.peer,
          network_id: selectedTarget.network_id,
          share_id: selectedTarget.share_id,
          path: entry.path,
        }),
      });
      newDownloadAnchor(
        `${FILE_DOWNLOADS_PATH}/${encodeURIComponent(pending.token)}`,
        entryName(entry.path),
      );
      refreshTransfers();
    } catch (error) {
      setDownloadError(fileError(locale, error));
    } finally {
      setDownloadingPath(null);
    }
  };

  const controlTransfer = async (
    status: FileTransferStatus,
    operation: "pause" | "resume" | "cancel",
  ) => {
    const key = `${status.transfer_id}\u0000${status.operation_id}\u0000${operation}`;
    if (controlBusy) return;
    setControlBusy(key);
    setTransfersError(null);
    try {
      const suffix = operation === "cancel" ? "" : `/${operation}`;
      await apiRequest<{ status: string }>(
        `${FILE_TRANSFERS_PATH}/${encodeURIComponent(status.transfer_id)}/${encodeURIComponent(status.operation_id)}${suffix}`,
        { method: operation === "cancel" ? "DELETE" : "POST" },
      );
      refreshTransfers();
    } catch (error) {
      setTransfersError(fileError(locale, error));
    } finally {
      setControlBusy(null);
    }
  };

  const segments = pathSegments(directoryPath);
  const entries = listing?.entries ?? [];

  return (
    <div className="page-content-stack files-page" data-testid="files-page">
      <section className="surface-card files-local-state">
        <div className="card-heading">
          <h3>{label(locale, "file.page.local_state.title")}</h3>
          <span
            className={`status-pill ${localFileState === "enabled" ? "good" : localFileState === "degraded" ? "warn" : "neutral"}`}
          >
            {label(locale, `status.capability.${localFileState}`)}
          </span>
        </div>
        <p className="muted-copy">
          {label(locale, "file.page.local_state.detail")}
        </p>
        <p className="field-help">
          {label(locale, "file.page.local_state.note")}
        </p>
      </section>

      {fileStatusMessage(locale, targetsState, targetsError)}
      {targetsState === "error" ? (
        <button
          className="secondary-button"
          type="button"
          onClick={() => {
            const controller = new AbortController();
            void loadTargets(controller.signal);
          }}
        >
          <RefreshCw size={16} />
          {label(locale, "file.page.retry")}
        </button>
      ) : null}

      {targetsState === "ready" && targets.length === 0 ? (
        <section
          className="surface-card files-empty"
          data-testid="file-no-targets"
        >
          <Users size={22} />
          <div>
            <h3>{label(locale, "file.page.no_targets.title")}</h3>
            <p>{label(locale, "file.page.no_targets.detail")}</p>
          </div>
        </section>
      ) : null}

      {targets.length > 0 ? (
        <section className="surface-card file-target-card">
          <div className="card-heading">
            <h3>{label(locale, "file.page.target.title")}</h3>
            <HardDrive size={18} />
          </div>
          <label className="form-field">
            <span>{label(locale, "file.page.target.label")}</span>
            <select
              value={selectedTargetKey}
              onChange={(event) => selectTarget(event.target.value)}
            >
              {targets.map((target) => (
                <option key={targetKey(target)} value={targetKey(target)}>
                  {tr(locale, "file.page.target.option", {
                    network: target.network_name,
                    peer: target.peer,
                  })}
                </option>
              ))}
            </select>
          </label>
          {selectedTarget ? (
            <dl className="file-target-meta">
              <div>
                <dt>{label(locale, "file.page.target.network_id")}</dt>
                <dd>
                  <code>{selectedTarget.network_id}</code>
                </dd>
              </div>
              <div>
                <dt>{label(locale, "file.page.target.peer_id")}</dt>
                <dd>
                  <code>{selectedTarget.peer}</code>
                </dd>
              </div>
            </dl>
          ) : null}
        </section>
      ) : null}

      {selectedTarget ? (
        <section
          className="surface-card file-browser"
          aria-labelledby="file-browser-title"
        >
          <div className="card-heading file-browser-heading">
            <div>
              <h3 id="file-browser-title">
                {label(locale, "file.page.directory.title")}
              </h3>
              <p>{label(locale, "file.page.directory.detail")}</p>
            </div>
            <button
              className="icon-button"
              type="button"
              onClick={refreshDirectory}
              disabled={directoryState === "loading"}
            >
              <RefreshCw
                className={directoryState === "loading" ? "spin" : ""}
                size={16}
              />
              <span className="sr-only">
                {label(locale, "file.page.refresh")}
              </span>
            </button>
          </div>

          <div className="file-browser-toolbar">
            <button
              className="secondary-button"
              type="button"
              onClick={() =>
                navigateDirectory(
                  directoryPath.split("/").slice(0, -1).join("/"),
                )
              }
              disabled={!directoryPath}
            >
              <ArrowLeft size={15} />
              {label(locale, "file.page.up")}
            </button>
            <nav
              className="file-breadcrumb"
              aria-label={label(locale, "file.page.breadcrumb")}
            >
              <button
                type="button"
                onClick={() => navigateDirectory("")}
                aria-current={!directoryPath ? "page" : undefined}
              >
                {label(locale, "file.page.root")}
              </button>
              {segments.map((segment, index) => (
                <span key={segment.path}>
                  <span aria-hidden="true">/</span>
                  <button
                    type="button"
                    onClick={() => navigateDirectory(segment.path)}
                    aria-current={
                      index === segments.length - 1 ? "page" : undefined
                    }
                  >
                    {segment.name}
                  </button>
                </span>
              ))}
            </nav>
          </div>

          <div className="file-upload-row">
            <label className="file-picker">
              <Upload size={15} />
              <span>
                {uploadFile?.name ?? label(locale, "file.page.choose_file")}
              </span>
              <input
                ref={uploadInputRef}
                type="file"
                onChange={(event: ChangeEvent<HTMLInputElement>) => {
                  setUploadFile(event.target.files?.[0] ?? null);
                  setUploadNotice(null);
                  setUploadError(null);
                }}
                disabled={uploading}
              />
            </label>
            <button
              className="primary-button"
              type="button"
              onClick={() => void uploadSelectedFile()}
              disabled={!uploadFile || uploading}
            >
              {uploading ? (
                <LoaderCircle className="spin" size={16} />
              ) : (
                <ArrowUpFromLine size={16} />
              )}
              {uploading
                ? label(locale, "file.page.uploading")
                : label(locale, "file.page.upload")}
            </button>
          </div>
          <p className="field-help">
            {tr(locale, "file.page.upload_limit", {
              limit: formatBytes(locale, MAX_WEB_FILE_BYTES),
            })}
          </p>

          {uploadNotice ? (
            <p className="form-success" role="status">
              {uploadNotice}
            </p>
          ) : null}
          {uploadError ? (
            <div className="file-inline-error" role="alert">
              <strong>{label(locale, uploadError.titleKey)}</strong>
              <span>{uploadError.detail}</span>
            </div>
          ) : null}
          {downloadError ? (
            <div className="file-inline-error" role="alert">
              <strong>{label(locale, downloadError.titleKey)}</strong>
              <span>{downloadError.detail}</span>
            </div>
          ) : null}

          {directoryState === "loading" ? (
            <div className="loading-panel" role="status">
              <LoaderCircle className="spin" size={18} />
              <div>
                <strong>{label(locale, "file.page.directory.loading")}</strong>
                <p>{label(locale, "file.page.directory.loading_detail")}</p>
              </div>
            </div>
          ) : null}
          {directoryState === "error"
            ? fileStatusMessage(locale, directoryState, directoryError)
            : null}
          {directoryState === "ready" &&
          entries.length === 0 &&
          !directoryError ? (
            <div
              className="file-directory-empty"
              data-testid="file-directory-empty"
            >
              <FolderOpen size={24} />
              <div>
                <strong>{label(locale, "file.page.directory.empty")}</strong>
                <p>{label(locale, "file.page.directory.empty_detail")}</p>
              </div>
            </div>
          ) : null}

          {entries.length > 0 ? (
            <div className="table-shell file-table-shell">
              <table>
                <thead>
                  <tr>
                    <th>{label(locale, "file.page.table.name")}</th>
                    <th>{label(locale, "file.page.table.size")}</th>
                    <th>{label(locale, "file.page.table.modified")}</th>
                    <th>
                      <span className="sr-only">
                        {label(locale, "file.page.table.actions")}
                      </span>
                    </th>
                  </tr>
                </thead>
                <tbody>
                  {entries.map((entry) => {
                    const name = entryName(entry.path);
                    return (
                      <tr key={entry.path}>
                        <td>
                          <button
                            className="file-entry-name"
                            type="button"
                            onClick={() => {
                              if (entry.is_directory) {
                                navigateDirectory(entry.path);
                                setUploadNotice(null);
                                setUploadError(null);
                                setDownloadError(null);
                              }
                            }}
                            disabled={!entry.is_directory}
                          >
                            {entry.is_directory ? (
                              <Folder size={16} />
                            ) : (
                              <FileIcon size={16} />
                            )}
                            <span>{name}</span>
                          </button>
                        </td>
                        <td>
                          {entry.is_directory
                            ? label(locale, "file.page.folder")
                            : formatBytes(locale, entry.size)}
                        </td>
                        <td>
                          {entry.modified_at_unix_ms
                            ? formatTimestamp(locale, entry.modified_at_unix_ms)
                            : label(locale, "common.unknown")}
                        </td>
                        <td>
                          {!entry.is_directory ? (
                            <button
                              className="secondary-button file-row-action"
                              type="button"
                              onClick={() => void downloadEntry(entry)}
                              disabled={
                                downloadingPath !== null ||
                                entry.size > MAX_WEB_FILE_BYTES
                              }
                              aria-label={tr(
                                locale,
                                "file.page.download_aria",
                                { name },
                              )}
                            >
                              {downloadingPath === entry.path ? (
                                <LoaderCircle className="spin" size={15} />
                              ) : (
                                <ArrowDownToLine size={15} />
                              )}
                              {downloadingPath === entry.path
                                ? label(locale, "file.page.downloading")
                                : label(locale, "file.page.download")}
                            </button>
                          ) : null}
                        </td>
                      </tr>
                    );
                  })}
                </tbody>
              </table>
            </div>
          ) : null}

          {listing?.next_cursor ? (
            <button
              className="secondary-button file-load-more"
              type="button"
              onClick={() => void loadMore()}
              disabled={loadingMore}
            >
              {loadingMore ? (
                <LoaderCircle className="spin" size={15} />
              ) : (
                <Folder size={15} />
              )}
              {loadingMore
                ? label(locale, "file.page.loading")
                : label(locale, "file.page.load_more")}
            </button>
          ) : null}
        </section>
      ) : null}

      <section
        className="surface-card file-transfers"
        aria-labelledby="file-transfers-title"
      >
        <div className="card-heading file-transfers-heading">
          <div>
            <h3 id="file-transfers-title">
              {label(locale, "file.page.transfers.title")}
            </h3>
            <p>
              {tr(locale, "file.page.transfers.detail", {
                active: activeTransfers,
              })}
            </p>
          </div>
          <button
            className="icon-button"
            type="button"
            onClick={refreshTransfers}
            disabled={transfersState === "loading"}
          >
            <RefreshCw
              className={transfersState === "loading" ? "spin" : ""}
              size={16}
            />
            <span className="sr-only">
              {label(locale, "file.page.refresh")}
            </span>
          </button>
        </div>

        {fileStatusMessage(locale, transfersState, transfersError)}
        {transfersState === "ready" &&
        transfers.length === 0 &&
        !transfersError ? (
          <div
            className="file-directory-empty"
            data-testid="file-transfers-empty"
          >
            <ArrowDownToLine size={24} />
            <div>
              <strong>{label(locale, "file.page.transfers.empty")}</strong>
              <p>{label(locale, "file.page.transfers.empty_detail")}</p>
            </div>
          </div>
        ) : null}

        {transfers.length > 0 ? (
          <div className="file-transfer-list">
            {transfers.map((status) => {
              const paused = status.state === "paused";
              const canPause =
                status.state === "active" || status.state === "queued";
              const terminal = !isActiveTransfer(status);
              const busyPrefix = `${status.transfer_id}\u0000${status.operation_id}\u0000`;
              const busy = controlBusy?.startsWith(busyPrefix) ?? false;
              const progress = transferProgress(status);
              return (
                <article
                  className="file-transfer-card"
                  key={`${status.transfer_id}:${status.operation_id}`}
                >
                  <div className="file-transfer-heading">
                    <div>
                      {status.direction === "upload" ? (
                        <ArrowUpFromLine size={17} />
                      ) : (
                        <ArrowDownToLine size={17} />
                      )}
                      <strong>
                        {label(
                          locale,
                          `file.page.transfer.direction.${status.direction}`,
                        )}
                      </strong>
                      <code>{status.transfer_id}</code>
                    </div>
                    <span
                      className={`status-pill ${transferStateTone(status.state)}`}
                    >
                      {label(locale, transferStateKey(status.state))}
                    </span>
                  </div>
                  <div className="file-transfer-progress">
                    <progress
                      max={100}
                      value={progress}
                      aria-label={tr(
                        locale,
                        "file.page.transfer.progress_aria",
                        {
                          direction: label(
                            locale,
                            `file.page.transfer.direction.${status.direction}`,
                          ),
                          transfer: status.transfer_id,
                        },
                      )}
                    />
                    <span>
                      {tr(locale, "file.page.transfer.progress", {
                        transferred: formatBytes(
                          locale,
                          status.transferred_bytes,
                        ),
                        total: formatBytes(locale, status.total_bytes),
                        chunks: status.total_chunks,
                      })}
                    </span>
                  </div>
                  <dl className="file-transfer-meta">
                    <div>
                      <dt>{label(locale, "file.page.transfer.operation")}</dt>
                      <dd>
                        <code>{status.operation_id}</code>
                      </dd>
                    </div>
                    <div>
                      <dt>{label(locale, "file.page.transfer.updated")}</dt>
                      <dd>
                        {formatTimestamp(locale, status.updated_at_unix_ms)}
                      </dd>
                    </div>
                    {status.error_code ? (
                      <div>
                        <dt>
                          {label(locale, "file.page.transfer.error_code")}
                        </dt>
                        <dd>
                          <code>{status.error_code}</code>
                        </dd>
                      </div>
                    ) : null}
                  </dl>
                  {!terminal ? (
                    <div className="file-transfer-actions">
                      {canPause ? (
                        <button
                          className="secondary-button"
                          type="button"
                          onClick={() => void controlTransfer(status, "pause")}
                          disabled={busy}
                        >
                          {busy ? (
                            <LoaderCircle className="spin" size={15} />
                          ) : (
                            <Pause size={15} />
                          )}
                          {label(locale, "file.page.pause")}
                        </button>
                      ) : null}
                      {paused ? (
                        <button
                          className="secondary-button"
                          type="button"
                          onClick={() => void controlTransfer(status, "resume")}
                          disabled={busy}
                        >
                          {busy ? (
                            <LoaderCircle className="spin" size={15} />
                          ) : (
                            <Play size={15} />
                          )}
                          {label(locale, "file.page.resume")}
                        </button>
                      ) : null}
                      <button
                        className="danger-button"
                        type="button"
                        onClick={() => void controlTransfer(status, "cancel")}
                        disabled={busy}
                      >
                        {busy ? (
                          <LoaderCircle className="spin" size={15} />
                        ) : (
                          <Trash2 size={15} />
                        )}
                        {label(locale, "file.page.cancel")}
                      </button>
                    </div>
                  ) : null}
                </article>
              );
            })}
          </div>
        ) : null}
      </section>

      <p className="field-help files-privacy-note">
        {label(locale, "file.page.privacy_note")}
      </p>
    </div>
  );
}
