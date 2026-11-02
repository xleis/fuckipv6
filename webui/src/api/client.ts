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

export class AuthenticationError extends Error {
  constructor(message = "authentication required") {
    super(message);
    this.name = "AuthenticationError";
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

export function nextRequestId(): number {
  requestSequence += 1;
  return requestSequence;
}

export function newIdempotencyKey(): string {
  if (typeof crypto.randomUUID === "function") return crypto.randomUUID();
  const bytes = crypto.getRandomValues(new Uint8Array(16));
  bytes[6] = (bytes[6] & 0x0f) | 0x40;
  bytes[8] = (bytes[8] & 0x3f) | 0x80;
  const hex = Array.from(bytes, (value) => value.toString(16).padStart(2, "0")).join("");
  return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20)}`;
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
  if (response.status === 401) {
    throw new AuthenticationError();
  }
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

export async function loadSnapshot(): Promise<NodeSnapshot> {
  const payload = await apiRequest<NodeApiSnapshotResponse>(
    `/api/v1/snapshot?schema_version=${NODE_CONTRACT_SCHEMA_VERSION}`,
  );
  return validateSnapshotResponse(payload);
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
