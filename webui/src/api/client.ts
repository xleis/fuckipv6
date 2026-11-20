import type {
  NodeApiCommandResponse,
  NodeApiErrorResponse,
  NodeApiManifestResponse,
  NodeApiOperationResponse,
  NodeApiSnapshotResponse,
  NodeCommandHttpRequest,
  NodeOperationSnapshot,
  NodeUiManifest,
  NodeSnapshot,
} from "../../../contracts/typescript/node-contract-v1";
import {
  operationStateIsTerminal,
  validateAcceptedCommandResponse,
  validateCommandResponse,
  validateManifestResponse,
  validateOperationResponse,
  validateSnapshotResponse,
} from "../contract/invariants";
import { NODE_CONTRACT_SCHEMA_VERSION } from "../../../contracts/typescript/node-contract-v1";
import {
  applySnapshotPatch,
} from "./snapshot-patch";
import type {
  SnapshotWorkerResponse,
} from "./snapshot-worker-protocol";

export class AuthenticationError extends Error {
  constructor(message = "authentication required") {
    super(message);
    this.name = "AuthenticationError";
  }
}

export class ReauthenticationRequiredError extends Error {
  constructor(message = "recent owner reauthentication required") {
    super(message);
    this.name = "ReauthenticationRequiredError";
  }
}

export class ContractError extends Error {
  constructor(message: string) {
    super(`node-ui-contract: ${message}`);
    this.name = "ContractError";
  }
}

export class HttpError extends Error {
  constructor(
    readonly status: number,
    readonly payload: NodeApiErrorResponse | null,
  ) {
    super(payload?.message || `HTTP ${status}`);
    this.name = "HttpError";
  }
}

export type JsonResult<T> = {
  response: Response;
  payload: T | null;
};

let requestSequence = Date.now() * 1000;

function record(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}

function recentAuthenticationRequired(payload: unknown): boolean {
  if (!record(payload)) return false;
  if (payload.code === "recent_authentication_required") {
    return true;
  }

  const result = payload.result;
  if (record(result) && record(result.outcome)) {
    const outcome = result.outcome;
    if (
      outcome.status === "rejected" &&
      record(outcome.data) &&
      outcome.data.code === "authentication_required" &&
      record(outcome.data.params) &&
      outcome.data.params.authorization_code ===
        "permission.recent_authentication_required"
    ) {
      return true;
    }
  }

  if (record(payload.problem)) {
    const problem = payload.problem;
    return (
      problem.code === "authentication_required" &&
      record(problem.params) &&
      problem.params.authorization_code ===
        "permission.recent_authentication_required"
    );
  }
  return false;
}

export function nextRequestId(): number {
  requestSequence += 1;
  return requestSequence;
}

export function newIdempotencyKey(): string {
  const alphabet =
    "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
  const bytes = crypto.getRandomValues(new Uint8Array(32));
  let encoded = "";
  for (let index = 0; index < bytes.length; index += 3) {
    const first = bytes[index];
    const second = bytes[index + 1];
    const third = bytes[index + 2];
    encoded += alphabet[first >> 2];
    encoded += alphabet[((first & 0x03) << 4) | ((second ?? 0) >> 4)];
    if (second !== undefined) {
      encoded += alphabet[((second & 0x0f) << 2) | ((third ?? 0) >> 6)];
    }
    if (third !== undefined) encoded += alphabet[third & 0x3f];
  }
  return `idem_${encoded}`;
}

function throwHttpError(status: number, payload: unknown): void {
  if (
    (status === 401 || status === 403) &&
    recentAuthenticationRequired(payload)
  ) {
    throw new ReauthenticationRequiredError();
  }
  if (status === 401) {
    throw new AuthenticationError();
  }
}

export async function requestJson<T>(path: string, init: RequestInit = {}): Promise<JsonResult<T>> {
  const headers = new Headers(init.headers);
  if (init.body !== undefined && !headers.has("Content-Type")) {
    headers.set("Content-Type", "application/json");
  }
  const response = await fetch(path, {
    ...init,
    headers,
    cache: "no-store",
    credentials: "same-origin",
  });
  let payload: T | null = null;
  try {
    payload = (await response.json()) as T;
  } catch {
    payload = null;
  }
  throwHttpError(response.status, payload);
  return { response, payload };
}

export async function apiRequest<T>(path: string, init: RequestInit = {}): Promise<T> {
  const { response, payload } = await requestJson<T>(path, init);
  if (!response.ok) {
    throw new HttpError(response.status, payload as NodeApiErrorResponse | null);
  }
  if (payload === null) throw new ContractError(`empty response from ${path}`);
  return payload;
}

export async function loadManifest(): Promise<NodeUiManifest> {
  const payload = await apiRequest<NodeApiManifestResponse>(
    `/api/v1/manifest?schema_version=${NODE_CONTRACT_SCHEMA_VERSION}`,
  );
  return validateManifestResponse(payload);
}


/** The node omits empty optional collections; fill them once so every
 *  consumer can iterate without null checks. */
function normalizeSnapshot(snapshot: NodeSnapshot): NodeSnapshot {
  return {
    ...snapshot,
    networks: snapshot.networks ?? [],
    peers: snapshot.peers ?? [],
    logs: snapshot.logs ?? [],
    embedded_services: snapshot.embedded_services ?? [],
    memberships: snapshot.memberships ?? [],
    network_profiles: snapshot.network_profiles ?? [],
    exit_selections: snapshot.exit_selections ?? [],
    routing_summaries: snapshot.routing_summaries ?? [],
    interfaces: snapshot.interfaces ?? [],
    interface_problems: snapshot.interface_problems ?? [],
    recent_activity: snapshot.recent_activity ?? [],
  };
}

function loadSnapshotInline(): Promise<NodeSnapshot> {
  return apiRequest<NodeApiSnapshotResponse>(
    `/api/v1/snapshot?schema_version=${NODE_CONTRACT_SCHEMA_VERSION}`,
  )
    .then(validateSnapshotResponse)
    .then(normalizeSnapshot);
}

type PendingSnapshotRequest = {
  resolve(result: SnapshotWorkerResponse): void;
  reject(error: unknown): void;
};

let snapshotWorker: Worker | null = null;
let snapshotWorkerUnavailable = false;
let snapshotWorkerRequestSequence = 0;
let lastWorkerSnapshot: NodeSnapshot | null = null;
let lastWorkerSnapshotToken = 0;
const pendingSnapshotRequests = new Map<number, PendingSnapshotRequest>();

function getSnapshotWorker(): Worker | null {
  if (snapshotWorker) return snapshotWorker;
  if (snapshotWorkerUnavailable) return null;
  let worker: Worker;
  try {
    worker = new Worker(new URL("./snapshot-worker.ts", import.meta.url), {
      type: "module",
    });
  } catch {
    // Some embedded webviews cannot load module workers (or the host does not
    // serve the worker chunk). Snapshot parsing then runs on the main thread.
    snapshotWorkerUnavailable = true;
    return null;
  }
  worker.onmessage = (event: MessageEvent<SnapshotWorkerResponse>) => {
    const pending = pendingSnapshotRequests.get(event.data.requestId);
    if (!pending) return;
    pendingSnapshotRequests.delete(event.data.requestId);
    pending.resolve(event.data);
  };
  worker.onerror = () => {
    snapshotWorkerUnavailable = true;
    worker.terminate();
    if (snapshotWorker === worker) snapshotWorker = null;
    // Re-run the in-flight requests inline instead of surfacing a hard error:
    // the worker is an optimisation, never the only path to a snapshot.
    const pending = [...pendingSnapshotRequests.values()];
    pendingSnapshotRequests.clear();
    for (const request of pending) {
      loadSnapshotInline().then(
        (snapshot) => request.resolve({ kind: "snapshot", snapshot } as SnapshotWorkerResponse),
        request.reject,
      );
    }
  };
  snapshotWorker = worker;
  return worker;
}

function loadSnapshotInWorker(previous?: NodeSnapshot): Promise<NodeSnapshot> {
  const worker = getSnapshotWorker();
  if (!worker) return loadSnapshotInline();
  const requestId = ++snapshotWorkerRequestSequence;
  const baseMatches = previous !== undefined && previous === lastWorkerSnapshot;
  return new Promise<SnapshotWorkerResponse>((resolve, reject) => {
    pendingSnapshotRequests.set(requestId, { resolve, reject });
    worker.postMessage({
      requestId,
      url: `/api/v1/snapshot?schema_version=${NODE_CONTRACT_SCHEMA_VERSION}`,
      baseToken: baseMatches ? lastWorkerSnapshotToken : undefined,
    });
  }).then((result) => {
    if (result.kind === "failure") throw new Error(result.message);
    if (result.kind === "contract") {
      throw new ContractError(result.message.replace(/^node-ui-contract: /, ""));
    }
    if (result.kind === "http") {
      throwHttpError(result.status, result.payload);
      throw new HttpError(
        result.status,
        result.payload as NodeApiErrorResponse | null,
      );
    }

    let next: NodeSnapshot;
    if (result.kind === "snapshot") {
      next = result.snapshot;
    } else {
      if (!previous || !baseMatches) {
        throw new ContractError("snapshot patch has no matching base snapshot");
      }
      next = applySnapshotPatch(previous, result.patch);
    }
    next = normalizeSnapshot(next);
    lastWorkerSnapshot = next;
    lastWorkerSnapshotToken = result.token;
    return next;
  });
}

export async function loadSnapshot(previous?: NodeSnapshot): Promise<NodeSnapshot> {
  if (typeof Worker === "undefined") return loadSnapshotInline();
  return loadSnapshotInWorker(previous);
}

export async function sendCommand(envelope: NodeCommandHttpRequest): Promise<NodeApiCommandResponse> {
  const payload = await apiRequest<NodeApiCommandResponse>("/api/v1/commands", {
    method: "POST",
    body: JSON.stringify(envelope),
  });
  if (payload.result) {
    validateCommandResponse(payload, envelope.request.request_id);
  } else {
    validateAcceptedCommandResponse(payload, envelope.request.request_id);
  }
  return payload;
}

export async function loadOperation(operationId: string): Promise<NodeOperationSnapshot> {
  const payload = await apiRequest<NodeApiOperationResponse>(
    `/api/v1/operations/${encodeURIComponent(operationId)}?schema_version=${NODE_CONTRACT_SCHEMA_VERSION}`,
  );
  return validateOperationResponse(payload, operationId);
}

export async function cancelOperation(
  operationId: string,
): Promise<NodeOperationSnapshot> {
  const payload = await apiRequest<NodeApiOperationResponse>(
    `/api/v1/operations/${encodeURIComponent(operationId)}/cancel?schema_version=${NODE_CONTRACT_SCHEMA_VERSION}`,
    { method: "POST" },
  );
  return validateOperationResponse(payload, operationId);
}

function abortableDelay(milliseconds: number, signal?: AbortSignal): Promise<void> {
  return new Promise((resolve, reject) => {
    if (signal?.aborted) {
      reject(new Error("operation wait aborted"));
      return;
    }
    const timer = window.setTimeout(resolve, milliseconds);
    signal?.addEventListener(
      "abort",
      () => {
        window.clearTimeout(timer);
        reject(new Error("operation wait aborted"));
      },
      { once: true },
    );
  });
}

export async function waitForOperation(
  operationId: string,
  options: {
    intervalMs?: number;
    timeoutMs?: number;
    signal?: AbortSignal;
    onOperation?: (operation: NodeOperationSnapshot) => void;
  } = {},
): Promise<NodeOperationSnapshot> {
  const intervalMs = options.intervalMs ?? 400;
  const timeoutMs = options.timeoutMs ?? 120_000;
  const deadline = Date.now() + timeoutMs;
  while (true) {
    const operation = await loadOperation(operationId);
    options.onOperation?.(operation);
    if (operationStateIsTerminal(operation.state)) return operation;
    if (Date.now() >= deadline)
      throw new Error(`operation ${operationId} did not reach a terminal state in time`);
    await abortableDelay(intervalMs, options.signal);
  }
}
