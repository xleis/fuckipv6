import { beforeEach, describe, expect, it } from "vitest";
import manifestFixture from "../../../contracts/typescript/fixtures/node-manifest.json";
import productVisualFixture from "../../../contracts/typescript/fixtures/product-visual-snapshot.json";
import type {
  NodeApiManifestResponse,
  NodeSnapshot,
  NodeUiManifest,
} from "../../../contracts/typescript/node-contract-v1";
import {
  commandEnvelope,
  validateManifestResponse,
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
  it("accepts the frozen eleven-page manifest fixture", () => {
    const validated = validateManifestResponse(manifestResponse(manifest));
    expect(validated.pages.map((page) => page.id)).toEqual([
      "overview",
      "networks",
      "peers",
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
});
