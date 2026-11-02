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
  if (response.status === 401 && recentAuthenticationRequired(payload)) {
    throw new ReauthenticationRequiredError();
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
