import { beforeEach, describe, expect, it } from "vitest";
import manifestFixture from "../../../contracts/typescript/fixtures/node-manifest.json";
import productVisualFixture from "../../../contracts/typescript/fixtures/product-visual-snapshot.json";
import type {
  NodeApiManifestResponse,
  NodeOperationSnapshot,
  NodeSnapshot,
  NodeUiManifest,
} from "../../../contracts/typescript/node-contract-v1";
import {
  commandEnvelope,
  executionBlockReason,
  operationStateIsTerminal,
  validateAcceptedCommandResponse,
  validateManifestResponse,
  validateOperationResponse,
  validateSnapshot,
  validateSnapshotResponse,
} from "./invariants";
import { installTestI18nRuntime } from "../test/runtime";

const manifest = manifestFixture as NodeUiManifest;

function manifestResponse(value: NodeUiManifest): NodeApiManifestResponse {
  return {
    schema_version: 1,
    request_id: 7,
    server_time: "2026-11-01T00:00:00Z",
    manifest: value,
  };
}

function emptySnapshot(): NodeSnapshot {
  return {
    revision: 3,
    lifecycle: {
      active_networks: 0,
      configured_networks: 0,
      failed_networks: [],
      phase: "stopped",
    },
    networks: [],
    peers: [],
    logs: [],
    embedded_services: [],
    configuration: { networks: [] },
    permissions: {
      access_model: "trusted_control_plane",
      permissions: ["view_state", "manage_networks"],
    },
    tun_capabilities: {
      schema_version: 1,
      platform: "linux",
      supported: true,
      can_rebuild: true,
      reasons: [],
    },
    control_health: "available",
    identity: null,
    last_error: null,
    capabilities: { capabilities: [] },
  };
}

beforeEach(() => {
  installTestI18nRuntime();
});

describe("node UI contract invariants", () => {
  it("accepts the frozen twelve-page manifest fixture", () => {
    const validated = validateManifestResponse(manifestResponse(manifest));
    expect(validated.pages.map((page) => page.id)).toEqual([
      "overview",
      "networks",
      "peers",
      "node_runtime",
      "topology",
      "traffic",
      "routing",
      "exit_node",
      "relay",
      "logs",
      "identity",
      "settings",
    ]);
  });

  it("rejects a manifest with a missing page", () => {
    const invalid = structuredClone(manifest);
    invalid.pages = invalid.pages.filter((page) => page.id !== "topology");
    expect(() => validateManifestResponse(manifestResponse(invalid))).toThrow("page set/order mismatch");
  });

  it("accepts the shared deterministic product visual fixture verbatim", () => {
    const fixture = productVisualFixture as {
      schema_version: number;
      locale: string;
      language_preference: string;
      snapshot: NodeSnapshot;
    };

    expect(fixture.schema_version).toBe(1);
    expect(fixture.locale).toBe("en");
    expect(fixture.language_preference).toBe("en");
    expect(validateSnapshot(fixture.snapshot)).toEqual(fixture.snapshot);
    expect(fixture.snapshot.revision).toBe(4242);
    expect(fixture.snapshot.networks).toHaveLength(2);
    expect(fixture.snapshot.peers).toHaveLength(2);
    expect(fixture.snapshot.logs).toHaveLength(5);
  });

  it("accepts a well-formed snapshot and rejects an invalid lifecycle phase", () => {
    expect(validateSnapshot(emptySnapshot())).toEqual(emptySnapshot());
    expect(
      validateSnapshotResponse({
        schema_version: 1,
        request_id: 9,
        server_time: "2026-11-01T00:00:00Z",
        snapshot: emptySnapshot(),
      }).revision,
    ).toBe(3);

    const invalid = emptySnapshot() as unknown as { lifecycle: { phase: string } };
    invalid.lifecycle.phase = "unknown";
    expect(() => validateSnapshot(invalid as unknown as NodeSnapshot)).toThrow("lifecycle phase invalid");
  });

  it("builds a versioned command envelope with request and idempotency identities", () => {
    const createNetwork = manifest.pages
      .flatMap((page) => page.actions)
      .find((action) => action.id === "create_network");
    expect(createNetwork).toBeDefined();

    const envelope = commandEnvelope(
      42,
      createNetwork!,
      {
        name: "home",
        signaling_port: "6666",
        relay_port: "6667",
        external_host: "",
        auto_connect: true,
      },
      "zh-CN",
      "8a4229b8-4088-4b4f-b1fe-adcbed27ac59",
    );

    expect(envelope.schema_version).toBe(1);
    expect(envelope.request.request_id).toBe(42);
    expect(envelope.idempotency_key).toBe("8a4229b8-4088-4b4f-b1fe-adcbed27ac59");
    expect(envelope.request.command).toEqual({
      command: "create_network",
      payload: {
        name: "home",
        signaling_port: 6666,
        relay_port: 6667,
        external_host: null,
        auto_connect: true,
      },
    });
  });

  it("fails closed when a rebuild action has no TUN capability data", () => {
    const action = manifest.pages
      .flatMap((page) => page.actions)
      .find((candidate) => candidate.id === "rebuild_tun");
    expect(action).toBeDefined();
    const snapshot = structuredClone(emptySnapshot()) as Partial<NodeSnapshot>;
    delete snapshot.tun_capabilities;

    expect(
      executionBlockReason(action!, snapshot as NodeSnapshot, "home"),
    ).toBe("tun_platform_unsupported");
  });

  it("maps TUN blocking reasons and permits only rebuildable capabilities", () => {
    const action = manifest.pages
      .flatMap((page) => page.actions)
      .find((candidate) => candidate.id === "rebuild_tun")!;
    const snapshot = emptySnapshot();
    snapshot.networks = [
      {
        name: "home",
        tun_name: "fk-home",
        status: "online",
        assigned_ipv6: "fd00::2",
        assigned_ipv4: null,
        p2p_listen_addr: null,
        uptime_secs: 1,
        roles: ["member"],
        peer_count: 0,
        traffic: {
          tun_rx_bytes: 0,
          tun_tx_bytes: 0,
          tun_rx_packets: 0,
          tun_tx_packets: 0,
          p2p_rx_bytes: 0,
          p2p_tx_bytes: 0,
          p2p_rx_packets: 0,
          p2p_tx_packets: 0,
          error_count: 0,
        },
      },
    ];
    snapshot.tun_capabilities = {
      schema_version: 1,
      platform: "linux",
      supported: true,
      can_rebuild: false,
      reasons: ["permission_denied", "device_unavailable"],
    };
    expect(executionBlockReason(action, snapshot, "home")).toBe(
      "tun_permission_denied",
    );

    snapshot.tun_capabilities.supported = false;
    snapshot.tun_capabilities.reasons = ["unknown"];
    expect(executionBlockReason(action, snapshot, "home")).toBe(
      "tun_platform_unsupported",
    );

    snapshot.tun_capabilities.supported = true;
    snapshot.tun_capabilities.can_rebuild = true;
    snapshot.tun_capabilities.reasons = [];
    expect(executionBlockReason(action, snapshot, "home")).toBeNull();
  });

  it("validates asynchronous command acceptance and terminal operation snapshots", () => {
    const operation = {
      operation_id: "op-42",
      state: "running",
      result: null,
    } as unknown as NodeOperationSnapshot;

    expect(
      validateAcceptedCommandResponse(
        {
          schema_version: 1,
          request_id: 42,
          server_time: "2026-11-01T00:00:00Z",
          operation,
        },
        42,
      ).operation_id,
    ).toBe("op-42");
    expect(operationStateIsTerminal("running")).toBe(false);
    expect(operationStateIsTerminal("succeeded")).toBe(true);

    const completed = {
      ...operation,
      state: "succeeded",
      result: { request_id: 42, outcome: { status: "completed" } },
    } as NodeOperationSnapshot;
    expect(
      validateOperationResponse(
        {
          schema_version: 1,
          request_id: 43,
          server_time: "2026-11-01T00:00:01Z",
          operation: completed,
        },
        "op-42",
      ).result?.outcome.status,
    ).toBe("completed");

    expect(() =>
      validateAcceptedCommandResponse(
        {
          schema_version: 1,
          request_id: 41,
          server_time: "2026-11-01T00:00:00Z",
          operation,
        },
        42,
      ),
    ).toThrow("command request id mismatch");
  });
});
