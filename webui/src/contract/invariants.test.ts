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
  outcomeSummary,
  validateAcceptedCommandResponse,
  validateActionValues,
  validateCommandResponse,
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

  it("rejects manifest page-state or block-reason vocabulary drift", () => {
    const invalidPageStates = structuredClone(manifest);
    invalidPageStates.page_states.reverse();
    expect(() =>
      validateManifestResponse(manifestResponse(invalidPageStates)),
    ).toThrow("page state vocabulary mismatch");

    const invalidReasons = structuredClone(manifest);
    invalidReasons.action_block_reasons = invalidReasons.action_block_reasons.filter(
      (reason) => reason !== "exit_node_unsupported",
    );
    expect(() =>
      validateManifestResponse(manifestResponse(invalidReasons)),
    ).toThrow("action block reason vocabulary mismatch");
  });

  it("rejects a confirmation without an explicit reauthentication declaration", () => {
    const invalid = structuredClone(manifest);
    const shutdown = invalid.pages
      .flatMap((page) => page.actions)
      .find((action) => action.id === "shutdown_node")!;
    delete (shutdown.confirmation as unknown as Record<string, unknown>)
      .recent_authentication_required;
    expect(() => validateManifestResponse(manifestResponse(invalid))).toThrow(
      "confirmation reauthentication flag invalid for shutdown_node",
    );
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
        purpose: "Family devices across two homes",
        icon: "home",
        language: "zh-CN",
        visibility: "unlisted",
        join_mode: "invite_only",
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
        purpose: "Family devices across two homes",
        icon: "home",
        language: "zh-CN",
        visibility: "unlisted",
        join_mode: "invite_only",
        signaling_port: 6666,
        relay_port: 6667,
        external_host: null,
        auto_connect: true,
      },
    });
  });

  it("validates network profile fields and summarizes the created identity", () => {
    const createNetwork = manifest.pages
      .flatMap((page) => page.actions)
      .find((action) => action.id === "create_network")!;
    const valid = {
      name: "home",
      purpose: "Family devices across two homes",
      icon: "home.router_2",
      language: "zh-CN",
      visibility: "unlisted",
      join_mode: "invite_only",
      signaling_port: "6666",
      relay_port: "6667",
      external_host: "",
      auto_connect: true,
    };

    expect(validateActionValues(createNetwork, valid, "zh-CN")).toEqual([]);
    expect(
      validateActionValues(
        createNetwork,
        { ...valid, purpose: "   " },
        "zh-CN",
      ),
    ).toEqual(["translated:validation.required"]);
    expect(
      validateActionValues(
        createNetwork,
        { ...valid, icon: "Home Icon" },
        "zh-CN",
      ),
    ).toEqual(["translated:validation.network_icon_invalid"]);
    expect(
      validateActionValues(
        createNetwork,
        { ...valid, language: "zh_CN" },
        "zh-CN",
      ),
    ).toEqual(["translated:validation.network_language_invalid"]);

    const outcome = {
      status: "network_created" as const,
      data: {
        network_name: "home",
        network_id: "net_stable_home",
        public_key: "network-public-key",
        owner_role: "owner" as const,
        config_saved: true,
        started: false,
        next_steps: [
          "invite_members" as const,
          "configure_policy" as const,
          "contribute_capabilities" as const,
        ],
        invitation_ticket: "fk://owner-secret@127.0.0.1:6666",
      },
    };
    expect(outcomeSummary(outcome, "zh-CN")).toBe(
      "translated:outcome.network_created",
    );
  });

  it("validates and builds a typed node identity rotation command", () => {
    const rotate = manifest.pages
      .flatMap((page) => page.actions)
      .find((action) => action.id === "rotate_node_identity");
    expect(rotate).toBeDefined();
    const fingerprint = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";

    expect(
      validateActionValues(
        rotate!,
        { expected_fingerprint: fingerprint },
        "zh-CN",
      ),
    ).toEqual([]);
    expect(
      commandEnvelope(
        43,
        rotate!,
        { expected_fingerprint: fingerprint },
        "zh-CN",
        "8a4229b8-4088-4b4f-b1fe-adcbed27ac59",
      ).request.command,
    ).toEqual({
      command: "rotate_node_identity",
      payload: { expected_fingerprint: fingerprint },
    });
  });

  it("rejects padded, malformed, or wrong-length identity fingerprints", () => {
    const rotate = manifest.pages
      .flatMap((page) => page.actions)
      .find((action) => action.id === "rotate_node_identity")!;
    for (const fingerprint of [
      "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=",
      "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA!",
      "short",
    ]) {
      const errors = validateActionValues(
        rotate,
        { expected_fingerprint: fingerprint },
        "zh-CN",
      );
      expect(errors).toEqual(["translated:validation.fingerprint_invalid"]);
      expect(() =>
        commandEnvelope(
          44,
          rotate,
          { expected_fingerprint: fingerprint },
          "zh-CN",
          "8a4229b8-4088-4b4f-b1fe-adcbed27ac59",
        ),
      ).toThrow("translated:validation.fingerprint_invalid");
    }
  });

  it("validates and summarizes an identity rotation outcome", () => {
    const outcome = {
      status: "identity_rotated" as const,
      data: {
        old_node_id: "node-old",
        new_node_id: "node-new",
        old_fingerprint: "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
        new_fingerprint: "BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB",
        created_at_unix_ms: 1_700_000_000_000,
        affected_configured_network_count: 2,
        owner_initialized: true,
        owner_sessions_invalidated: true,
      },
    };
    expect(
      validateCommandResponse(
        {
          schema_version: 1,
          request_id: 45,
          server_time: "2026-11-01T00:00:00Z",
          result: { request_id: 45, outcome },
        },
        45,
      ),
    ).toEqual(outcome);
    expect(outcomeSummary(outcome, "zh-CN")).toBe(
      "translated:outcome.identity_rotated",
    );

    const invalid = structuredClone(outcome) as unknown as {
      data: Record<string, unknown>;
    };
    invalid.data.owner_sessions_invalidated = "yes";
    expect(() =>
      validateCommandResponse(
        {
          schema_version: 1,
          request_id: 46,
          server_time: "2026-11-01T00:00:00Z",
          result: {
            request_id: 46,
            outcome: invalid as unknown as typeof outcome,
          },
        },
        46,
      ),
    ).toThrow("owner session invalidation flag invalid");
  });

  it("uses the shared canonical reason for unsupported exit-node actions", () => {
    const action = manifest.pages
      .flatMap((page) => page.actions)
      .find((candidate) => candidate.id === "set_exit_node_role")!;
    const snapshot = emptySnapshot();
    snapshot.permissions = {
      access_model: "trusted_control_plane",
      permissions: ["view_state", "manage_networks", "manage_roles"],
    };
    snapshot.capabilities = {
      capabilities: [{ kind: "exit", state: "unsupported" }],
    };

    expect(executionBlockReason(action, snapshot, "home")).toBe(
      "exit_node_unsupported",
    );
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
