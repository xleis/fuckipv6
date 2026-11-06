import type { TopologySnapshot } from "../../contracts/typescript/node-contract-v1";
import { layoutTopology, type TopologyLayout } from "./topology";

export type TopologyLayoutWorkerRequest = {
  requestId: number;
  snapshot: TopologySnapshot;
  width: number;
  height: number;
};

export type TopologyLayoutWorkerResponse = {
  requestId: number;
  layout: TopologyLayout;
};

const scope = globalThis as unknown as {
  onmessage: ((event: MessageEvent<TopologyLayoutWorkerRequest>) => void) | null;
  postMessage(message: TopologyLayoutWorkerResponse): void;
};

scope.onmessage = (event) => {
  const { requestId, snapshot, width, height } = event.data;
  scope.postMessage({
    requestId,
    layout: layoutTopology(snapshot, width, height),
  });
};
