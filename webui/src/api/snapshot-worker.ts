import type { NodeSnapshot } from "../../../contracts/typescript/node-contract-v1";
import { validateSnapshotResponse } from "../contract/invariants";
import { diffSnapshot } from "./snapshot-patch";
import type {
  SnapshotWorkerRequest,
  SnapshotWorkerResult,
  SnapshotWorkerResponse,
} from "./snapshot-worker-protocol";

const scope = globalThis as unknown as {
  onmessage: ((event: MessageEvent<SnapshotWorkerRequest>) => void) | null;
  postMessage(message: SnapshotWorkerResponse): void;
};

const SNAPSHOT_CACHE_LIMIT = 4;
const snapshotCache = new Map<number, NodeSnapshot>();
let cachedToken = 0;

function contractError(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function post(requestId: number, response: SnapshotWorkerResult): void {
  scope.postMessage({ requestId, ...response });
}

scope.onmessage = async (event) => {
  const { requestId, url, baseToken } = event.data;
  try {
    const response = await fetch(url, {
      cache: "no-store",
      credentials: "same-origin",
    });
    let payload: unknown = null;
    try {
      payload = await response.json();
    } catch {
      payload = null;
    }

    if (!response.ok || payload === null) {
      post(requestId, { kind: "http", status: response.status, payload });
      return;
    }

    let snapshot: NodeSnapshot;
    try {
      snapshot = validateSnapshotResponse(
        payload as Parameters<typeof validateSnapshotResponse>[0],
      );
    } catch (error) {
      post(requestId, {
        kind: "contract",
        status: response.status,
        message: contractError(error),
      });
      return;
    }

    const baseSnapshot =
      baseToken === undefined ? undefined : snapshotCache.get(baseToken);
    const patch = baseSnapshot ? diffSnapshot(baseSnapshot, snapshot) : null;
    cachedToken += 1;
    snapshotCache.set(cachedToken, snapshot);
    while (snapshotCache.size > SNAPSHOT_CACHE_LIMIT) {
      const oldest = snapshotCache.keys().next().value;
      if (oldest === undefined) break;
      snapshotCache.delete(oldest);
    }

    if (baseSnapshot && patch) {
      post(requestId, {
        kind: "patch",
        status: response.status,
        token: cachedToken,
        patch,
      });
      return;
    }
    post(requestId, {
      kind: "snapshot",
      status: response.status,
      token: cachedToken,
      snapshot,
    });
  } catch (error) {
    post(requestId, { kind: "failure", message: contractError(error) });
  }
};
