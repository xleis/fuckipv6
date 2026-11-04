import { beforeEach, describe, expect, it } from "vitest";
import manifestFixture from "../../../contracts/typescript/fixtures/node-manifest.json";
import productVisualFixture from "../../../contracts/typescript/fixtures/product-visual-snapshot.json";
import type {
  NetworkProfileProjections,
  NetworkPublicProfileDocument,
  NodeApiManifestResponse,
  NodeCommandOutcome,
  NodeOperationSnapshot,
  NodeSnapshot,
  NodeUiManifest,
} from "../../../contracts/typescript/node-contract-v1";
import {
  buildCommand,
  commandEnvelope,
  executionBlockReason,
  initialActionValues,
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

const publicProfile: NetworkPublicProfileDocument = {
  schema_version: 1,
  network_id: "net_stable_home",
  public_key: "network-public-key",
  owner: "node-owner",
  name: "home",
  purpose: "Family devices across two homes",
  icon: "home.router_2",
  language: "zh-CN",
  region: "CN",
  tags: ["family", "smart-home"],
  capability_declarations: ["relay"],
  visibility: "unlisted",
  join_mode: "invite_only",
  revision: 4,
  issued_at_unix_ms: 1_700_000_000_000,
  expires_at_unix_ms: 1_700_086_400_000,
};

const profileProjections: NetworkProfileProjections = {
  public_card: {
    candidate: publicProfile,
    currently_published: {
      document: { ...publicProfile, revision: 3 },
      signature: "signature",
    },
    withheld_categories: [
      "member_identity",
      "address",
      "topology",
      "activity",
      "invitation",
      "private_key",
    ],
  },
  member_view: {
    profile: publicProfile,
    member_count: 3,
    administrator_count: 1,
  },
  administrator_view: {
    profile: publicProfile,
    member_count: 3,
    administrator_count: 1,
    revision: 4,
    policy_revision: 2,
  },
  local_secret: {
    identity_present: true,
    secret_material_included: false,
    public_key_fingerprint: "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
    storage: "identity-keychain",
  },
};

const profilePreview = {
  network_name: "home",
  expected_revision: 3,
  current_revision: 4,
  candidate_revision: 5,
  conflict: true,
  can_apply: false,
  projections: profileProjections,
  diff: [
    { path: "purpose", before: "Old purpose", after: publicProfile.purpose },
    { path: "tags", before: null, after: "family,smart-home" },
  ],
  warnings: [
    {
      code: "capability_not_observed" as const,
      field: "capability_declarations",
      capability: "relay" as const,
      message: "Relay capability has not been observed",
    },
  ],
};

const profileUpdate = {
  network_name: "home",
  revision: 5,
  previous_revision: 4,
  applied_at_unix_ms: 1_700_000_100_000,
  changed_paths: ["purpose", "tags"],
  signed_profile: {
    document: { ...publicProfile, revision: 5 },
    signature: "signature",
  },
  projections: profileProjections,
  warnings: profilePreview.warnings,
  audit_action: "network.profile.update",
};

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
        region: "ZZ",
        tags: "",
        capability_declarations: "",
        visibility: "unlisted",
        join_mode: "invite_only",
        exit_contribution: "not_required",
        exit_contribution_scope: "not_applicable",
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
        region: "ZZ",
        tags: [],
        capability_declarations: [],
        visibility: "unlisted",
        join_mode: "invite_only",
        exit_contribution: "not_required",
        exit_contribution_scope: "not_applicable",
        signaling_port: 6666,
        relay_port: 6667,
        external_host: null,
        auto_connect: true,
      },
    });
  });

  it("requires and builds the complete signed network join confirmation", () => {
    const join = manifest.pages
      .flatMap((page) => page.actions)
      .find((action) => action.id === "join_network")!;
    const confirmed = {
      target: "",
      network_name: "family",
      start: true,
      network_id: "network-family",
      preview_digest: "preview-digest-family",
      network_policy_revision: "4",
      privacy_policy_revision: "3",
      terms_hash: "terms-hash-family",
      acknowledged_at_unix_ms: "1700000000123",
    };

    expect(validateActionValues(join, confirmed, "zh-CN")).toEqual([]);
    expect(
      commandEnvelope(
        41,
        join,
        confirmed,
        "zh-CN",
        "8a4229b8-4088-4b4f-b1fe-adcbed27ac59",
      ).request.command,
    ).toEqual({
      command: "join_network",
      payload: {
        target: "",
        network_name: "family",
        start: true,
        network_id: "network-family",
        preview_digest: "preview-digest-family",
        network_policy_revision: 4,
        privacy_policy_revision: 3,
        terms_hash: "terms-hash-family",
        acknowledged_at_unix_ms: 1_700_000_000_123,
      },
    });

    for (const [values, error] of [
      [
        { ...confirmed, preview_digest: "" },
        "translated:validation.join_confirmation_incomplete",
      ],
      [
        { ...confirmed, terms_hash: " " },
        "translated:validation.join_confirmation_incomplete",
      ],
      [
        { ...confirmed, network_policy_revision: "0" },
        "translated:validation.join_policy_revision_invalid",
      ],
      [
        { ...confirmed, privacy_policy_revision: "-1" },
        "translated:validation.join_policy_revision_invalid",
      ],
      [
        { ...confirmed, acknowledged_at_unix_ms: "0" },
        "translated:validation.join_acknowledgement_invalid",
      ],
    ] as const) {
      expect(validateActionValues(join, values, "zh-CN")).toEqual([error]);
      expect(() => commandEnvelope(42, join, values, "zh-CN", "idempotency")).toThrow(
        error,
      );
    }
  });

  it("summarizes join preview, joined, and leave outcomes", () => {
    const preview = {
      status: "network_join_preview",
      data: { network_name: "family", member_count: 8 },
    } as unknown as NodeCommandOutcome;
    const joined = {
      status: "network_joined",
      data: { membership: { network_name: "family" }, idempotent: false },
    } as unknown as NodeCommandOutcome;
    const left = {
      status: "network_left",
      data: { network_name: "family" },
    } as unknown as NodeCommandOutcome;

    expect(outcomeSummary(preview, "zh-CN")).toBe(
      "translated:outcome.network_join_previewed",
    );
    expect(outcomeSummary(joined, "zh-CN")).toBe(
      "translated:outcome.network_joined",
    );
    expect(outcomeSummary(left, "zh-CN")).toBe(
      "translated:outcome.network_left",
    );
  });

  it("builds typed profile preview and update commands from snapshot defaults", () => {
    const profileSnapshot = structuredClone(
      productVisualFixture.snapshot as NodeSnapshot,
    );
    profileSnapshot.network_profiles = [
      {
        name: "home",
        network_id: "net_stable_home",
        purpose: publicProfile.purpose,
        icon: publicProfile.icon,
        language: publicProfile.language,
        region: publicProfile.region,
        tags: publicProfile.tags,
        capability_declarations: publicProfile.capability_declarations,
        visibility: publicProfile.visibility,
        join_mode: publicProfile.join_mode,
        exit_contribution: "not_required",
        exit_contribution_scope: "not_applicable",
        revision: publicProfile.revision,
        policy_revision: 2,
        signed_profile: {
          schema_version: 1,
          issued_at_unix_ms: publicProfile.issued_at_unix_ms,
          expires_at_unix_ms: publicProfile.expires_at_unix_ms,
        },
      },
    ];
    const preview = manifest.pages
      .flatMap((page) => page.actions)
      .find((action) => action.id === "preview_network_profile")!;
    const update = manifest.pages
      .flatMap((page) => page.actions)
      .find((action) => action.id === "update_network_profile")!;
    const values = initialActionValues(preview, profileSnapshot, "home");
    expect(values).toMatchObject({
      network_name: "home",
      expected_revision: "4",
      purpose: publicProfile.purpose,
      icon: publicProfile.icon,
      language: "zh-CN",
      region: "CN",
      tags: "family,smart-home",
      capability_declarations: "relay",
      visibility: "unlisted",
      join_mode: "invite_only",
    });
    expect(validateActionValues(preview, values, "zh-CN")).toEqual([]);

    const edited = {
      ...values,
      purpose: "Updated family network",
      tags: "family, smart-home",
      capability_declarations: "relay,exit",
    };
    const payload = {
      network_name: "home",
      expected_revision: 4,
      update: {
        purpose: "Updated family network",
        icon: "home.router_2",
        language: "zh-CN",
        region: "CN",
        tags: ["family", "smart-home"],
        capability_declarations: ["relay", "exit"],
        visibility: "unlisted",
        join_mode: "invite_only",
        exit_contribution: "not_required",
        exit_contribution_scope: "not_applicable",
      },
    };
    expect(
      commandEnvelope(
        43,
        preview,
        edited,
        "zh-CN",
        "8a4229b8-4088-4b4f-b1fe-adcbed27ac59",
      ).request.command,
    ).toEqual({ command: "preview_network_profile", payload });
    expect(
      commandEnvelope(
        44,
        update,
        edited,
        "zh-CN",
        "8a4229b8-4088-4b4f-b1fe-adcbed27ac59",
      ).request.command,
    ).toEqual({ command: "update_network_profile", payload });
  });

  it("validates region, tag and capability profile constraints", () => {
    const createNetwork = manifest.pages
      .flatMap((page) => page.actions)
      .find((action) => action.id === "create_network")!;
    const valid = {
      name: "home",
      purpose: "Family devices across two homes",
      icon: "home.router_2",
      language: "zh-CN",
      region: "ZZ",
      tags: "",
      capability_declarations: "",
      visibility: "unlisted",
      join_mode: "invite_only",
      exit_contribution: "not_required",
      exit_contribution_scope: "not_applicable",
      signaling_port: "6666",
      relay_port: "6667",
      external_host: "",
      auto_connect: true,
    };

    expect(
      validateActionValues(createNetwork, { ...valid, region: "cn" }, "zh-CN"),
    ).toEqual(["translated:validation.network_region_invalid"]);
    expect(
      validateActionValues(
        createNetwork,
        { ...valid, tags: Array.from({ length: 13 }, (_, i) => `tag${i}`).join(",") },
        "zh-CN",
      ),
    ).toEqual(["translated:validation.network_tags_too_many"]);
    expect(
      validateActionValues(
        createNetwork,
        { ...valid, tags: "home,bad tag" },
        "zh-CN",
      ),
    ).toEqual(["translated:validation.network_tag_invalid"]);
    expect(
      validateActionValues(
        createNetwork,
        { ...valid, tags: "home,HOME" },
        "zh-CN",
      ),
    ).toEqual(["translated:validation.network_tag_duplicate"]);
    expect(
      validateActionValues(
        createNetwork,
        { ...valid, capability_declarations: "relay,storage" },
        "zh-CN",
      ),
    ).toEqual(["translated:validation.network_capability_invalid"]);
    expect(
      validateActionValues(
        createNetwork,
        { ...valid, capability_declarations: "relay,relay" },
        "zh-CN",
      ),
    ).toEqual(["translated:validation.network_capability_duplicate"]);
  });

  it("validates signed profile preview and applied-result contracts", () => {
    const previewOutcome = {
      status: "network_profile_preview" as const,
      data: profilePreview,
    };
    expect(
      validateCommandResponse(
        {
          schema_version: 1,
          request_id: 45,
          server_time: "2026-11-01T00:00:00Z",
          result: { request_id: 45, outcome: previewOutcome },
        },
        45,
      ),
    ).toEqual(previewOutcome);
    expect(outcomeSummary(previewOutcome, "zh-CN")).toBe(
      "translated:outcome.network_profile_previewed",
    );

    const updateOutcome = {
      status: "network_profile_updated" as const,
      data: profileUpdate,
    };
    expect(
      validateCommandResponse(
        {
          schema_version: 1,
          request_id: 46,
          server_time: "2026-11-01T00:00:01Z",
          result: { request_id: 46, outcome: updateOutcome },
        },
        46,
      ),
    ).toEqual(updateOutcome);
    expect(outcomeSummary(updateOutcome, "zh-CN")).toBe(
      "translated:outcome.network_profile_updated",
    );

    const invalidPreview = structuredClone(profilePreview) as unknown as {
      projections: Record<string, unknown>;
    };
    delete invalidPreview.projections.administrator_view;
    expect(() =>
      validateCommandResponse(
        {
          schema_version: 1,
          request_id: 47,
          server_time: "2026-11-01T00:00:02Z",
          result: {
            request_id: 47,
            outcome: {
              status: "network_profile_preview",
              data: invalidPreview,
            } as unknown as typeof previewOutcome,
          },
        },
        47,
      ),
    ).toThrow("network administrator projection invalid");

    const invalidUpdate = structuredClone(profileUpdate) as unknown as {
      changed_paths: unknown;
    };
    invalidUpdate.changed_paths = "purpose";
    expect(() =>
      validateCommandResponse(
        {
          schema_version: 1,
          request_id: 48,
          server_time: "2026-11-01T00:00:03Z",
          result: {
            request_id: 48,
            outcome: {
              status: "network_profile_updated",
              data: invalidUpdate,
            } as unknown as typeof updateOutcome,
          },
        },
        48,
      ),
    ).toThrow("network profile changed paths invalid");
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
      region: "ZZ",
      tags: "",
      capability_declarations: "",
      visibility: "unlisted",
      join_mode: "invite_only",
      exit_contribution: "not_required",
      exit_contribution_scope: "not_applicable",
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

  it("builds typed join request commands and validates review batches", () => {
    const actions = new Map(
      manifest.pages
        .flatMap((page) => page.actions)
        .map((action) => [action.id, action]),
    );
    const requestId = `join_${"A".repeat(43)}`;
    const networkId = "net_stable_home";

    expect(
      buildCommand(
        actions.get("submit_join_request")!,
        { network_id: networkId, reason: "Need access from the branch office." },
        "en",
      ),
    ).toEqual({
      command: "submit_join_request",
      payload: {
        network_id: networkId,
        reason: "Need access from the branch office.",
      },
    });
    expect(
      buildCommand(
        actions.get("list_join_requests")!,
        { network_id: networkId },
        "en",
      ),
    ).toEqual({
      command: "list_join_requests",
      payload: { network_id: networkId },
    });
    expect(
      buildCommand(
        actions.get("review_join_request")!,
        {
          network_id: networkId,
          request_id: requestId,
          expected_revision: "2",
          action: "request_information",
          note: "Please describe the devices that need access.",
        },
        "en",
      ),
    ).toEqual({
      command: "review_join_request",
      payload: {
        network_id: networkId,
        review: {
          request_id: requestId,
          expected_revision: 2,
          action: "request_information",
          note: "Please describe the devices that need access.",
        },
      },
    });

    const batch = actions.get("batch_review_join_requests")!;
    expect(
      buildCommand(
        batch,
        {
          network_id: networkId,
          reviews: JSON.stringify([
            {
              request_id: requestId,
              expected_revision: 2,
              action: "approve",
            },
          ]),
        },
        "en",
      ),
    ).toEqual({
      command: "batch_review_join_requests",
      payload: {
        network_id: networkId,
        reviews: [
          {
            request_id: requestId,
            expected_revision: 2,
            action: "approve",
          },
        ],
      },
    });
    expect(
      validateActionValues(
        batch,
        {
          network_id: networkId,
          reviews: JSON.stringify([
            {
              request_id: requestId,
              expected_revision: 2,
              action: "request_information",
            },
          ]),
        },
        "en",
      ),
    ).toContain("translated:join_request.error.information_required");

    expect(
      buildCommand(
        actions.get("withdraw_join_request")!,
        { network_id: networkId, request_id: requestId },
        "en",
      ),
    ).toEqual({
      command: "withdraw_join_request",
      payload: { network_id: networkId, request_id: requestId },
    });
    expect(
      buildCommand(
        actions.get("resubmit_join_request")!,
        {
          network_id: networkId,
          request_id: requestId,
          expected_revision: "3",
          reason: "Added the requested device details.",
        },
        "en",
      ),
    ).toEqual({
      command: "resubmit_join_request",
      payload: {
        network_id: networkId,
        request_id: requestId,
        expected_revision: 3,
        reason: "Added the requested device details.",
      },
    });
  });
});
