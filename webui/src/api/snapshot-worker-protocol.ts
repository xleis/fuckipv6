import type { NodeSnapshot } from "../../../contracts/typescript/node-contract-v1";
import type { SnapshotPatch } from "./snapshot-patch";

export type SnapshotWorkerRequest = {
  requestId: number;
  url: string;
  baseToken?: number;
};

export type SnapshotWorkerResult =
  | {
      kind: "snapshot";
      status: number;
      token: number;
      snapshot: NodeSnapshot;
    }
  | {
      kind: "patch";
      status: number;
      token: number;
      patch: SnapshotPatch;
    }
  | {
      kind: "http";
      status: number;
      payload: unknown;
    }
  | {
      kind: "contract";
      status: number;
      message: string;
    }
  | {
      kind: "failure";
      message: string;
    };

export type SnapshotWorkerResponse = {
  requestId: number;
} & SnapshotWorkerResult;
