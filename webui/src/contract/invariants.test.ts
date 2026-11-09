import { beforeEach, describe, expect, it } from "vitest";
import manifestFixture from "../../../contracts/typescript/fixtures/node-manifest.json";
import productVisualFixture from "../../../contracts/typescript/fixtures/product-visual-snapshot.json";
import discoveryParityFixture from "../../../contracts/typescript/fixtures/discovery-ui-parity.json";
import type {
  NetworkProfileProjections,
  NetworkPublicProfileDocument,
  NodeApiManifestResponse,
  NodeCommand,
  NodeCommandOutcome,
  NodeOperationSnapshot,
  NodeSnapshot,
  NetworkMemberDirectory,
  NodeUiAction,
  NodeUiManifest,
  PolicyApplyResult,
  PolicyHistoryEntry,
  PolicyPreview,
  PolicyRiskSummary,
  PolicyRollbackResult,
  PolicySnapshot,
  PolicyTestInput,
  PolicyTestResult,
  TrafficPolicy,
  TrafficPolicyDraft,
  TrafficPolicyTestInput,
} from "../../../contracts/typescript/node-contract-v1";
import {
  buildCommand,
  commandEnvelope,
  executionBlockReason,
  initialActionValues,
  operationStateIsTerminal,
  outcomeSummary,
  primaryStatus,
  trafficProtectionStatus,
  validateAcceptedCommandResponse,
  validateActionValues,
  validateCommandOutcome,
  validateCommandResponse,
  validateManifestResponse,
  validateOperationResponse,
  validateSnapshot,
  validateSnapshotResponse,
} from "./invariants";
import { installTestI18nRuntime } from "../test/runtime";
import {
  parsePolicySnapshotJson,
  parsePolicyTestInputJson,
  PolicyJsonError,
  validatePolicySnapshot,
  validatePolicyTestResult,
} from "./policy-test";
import {
  parseTrafficPolicyDraftJson,
  parseTrafficPolicyJson,
  parseTrafficPolicyTestInputJson,
  TrafficPolicyJsonError,
} from "./traffic-policy";

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
  it("accepts the frozen thirteen-page manifest fixture", () => {
    const validated = validateManifestResponse(manifestResponse(manifest));
    expect(validated.pages.map((page) => page.id)).toEqual([
      "overview",
      "networks",
      "peers",
      "node_runtime",
      "topology",
      "traffic",
      "activity",
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
    expect(() => validateManifestResponse(manifestResponse(invalid))).toThrow(
      "page set/order mismatch",
    );
  });

  it("rejects manifest page-state or block-reason vocabulary drift", () => {
    const invalidPageStates = structuredClone(manifest);
    invalidPageStates.page_states.reverse();
    expect(() =>
      validateManifestResponse(manifestResponse(invalidPageStates)),
    ).toThrow("page state vocabulary mismatch");

    const invalidReasons = structuredClone(manifest);
    invalidReasons.action_block_reasons =
      invalidReasons.action_block_reasons.filter(
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

    const invalid = emptySnapshot() as unknown as {
      lifecycle: { phase: string };
    };
    invalid.lifecycle.phase = "unknown";
    expect(() => validateSnapshot(invalid as unknown as NodeSnapshot)).toThrow(
      "lifecycle phase invalid",
    );
  });

  it("keeps node lifecycle and traffic protection status independent", () => {
    const value = emptySnapshot();
    value.lifecycle.phase = "running";
    value.traffic_session = {
      state: "fail_closed",
      changed_at_unix_ms: 1_700_000_100_000,
      reason_code: "exit.recovery.handshake_interrupted",
      failure: "handshake_interrupted",
      validation: null,
      events: [
        {
          kind: "failure_detected",
          state: "fail_closed",
          occurred_at_unix_ms: 1_700_000_100_000,
          failure: "handshake_interrupted",
          reason_code: "exit.recovery.handshake_interrupted",
        },
      ],
    };

    expect(validateSnapshot(value)).toEqual(value);
    expect(primaryStatus(value, "zh-CN").id).toBe("running");
    expect(trafficProtectionStatus(value, "zh-CN")).toMatchObject({
      id: "fail_closed",
      label: "translated:traffic.session.state.fail_closed",
      detail: "exit.recovery.handshake_interrupted",
      tone: "bad",
    });

    value.traffic_session = {
      state: "protected",
      changed_at_unix_ms: 1_700_000_100_000,
      validation: null,
      events: [],
    };
    expect(() => validateSnapshot(value)).toThrow(
      "protected traffic session lacks complete validation",
    );
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
        exit_contribution_mode: "disabled",
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
    expect(envelope.idempotency_key).toBe(
      "8a4229b8-4088-4b4f-b1fe-adcbed27ac59",
    );
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
        exit_contribution_mode: "disabled",
        exit_contribution: "not_required",
        exit_contribution_scope: "not_applicable",
        exit_selection_policy: {
          mode: { mode: "member_choice" },
          available_exits: [],
          denied_exits: [],
          groups: [],
          members: [],
          session_transition: "fail_closed",
        },
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
      expect(() =>
        commandEnvelope(42, join, values, "zh-CN", "idempotency"),
      ).toThrow(error);
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
        exit_contribution_mode: "disabled",
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
        exit_contribution_mode: "disabled",
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

  it("round-trips Exit selection policy only when the operator edits it", () => {
    const snapshot = structuredClone(
      productVisualFixture.snapshot as NodeSnapshot,
    );
    const policy = {
      mode: { mode: "fixed" as const, exit: "node_exit_primary" },
      default_exit: "node_exit_primary",
      available_exits: ["node_exit_primary", "node_exit_backup"],
      session_transition: "fail_closed" as const,
    };
    snapshot.network_profiles = [
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
        exit_contribution_mode: "disabled",
        revision: publicProfile.revision,
        policy_revision: 2,
        signed_profile: {
          schema_version: 1,
          issued_at_unix_ms: publicProfile.issued_at_unix_ms,
          expires_at_unix_ms: publicProfile.expires_at_unix_ms,
        },
      },
    ];
    snapshot.exit_selections = [
      {
        network_id: "net_stable_home",
        policy_revision: 2,
        policy,
        evaluated_at_unix_ms: 1_700_000_100_000,
      },
    ];
    const create = manifest.pages
      .flatMap((page) => page.actions)
      .find((action) => action.id === "create_network")!;
    const preview = manifest.pages
      .flatMap((page) => page.actions)
      .find((action) => action.id === "preview_network_profile")!;
    const update = manifest.pages
      .flatMap((page) => page.actions)
      .find((action) => action.id === "update_network_profile")!;
    const createValues = {
      name: "home",
      purpose: publicProfile.purpose,
      icon: publicProfile.icon,
      language: publicProfile.language,
      region: publicProfile.region,
      tags: "",
      capability_declarations: "",
      visibility: "unlisted",
      join_mode: "invite_only",
      exit_contribution_mode: "disabled",
      exit_contribution: "not_required",
      exit_contribution_scope: "not_applicable",
      signaling_port: "6666",
      relay_port: "6667",
      external_host: "",
      auto_connect: true,
    };
    expect(
      initialActionValues(create, snapshot, "home")
        .exit_selection_policy_json,
    ).toBe("{}");
    expect(
      buildCommand(create, createValues, "en"),
    ).toMatchObject({
      payload: {
        exit_selection_policy: {
          mode: { mode: "member_choice" },
          available_exits: [],
          denied_exits: [],
          groups: [],
          members: [],
          session_transition: "fail_closed",
        },
      },
    });

    const values = initialActionValues(preview, snapshot, "home");
    expect(values.exit_selection_policy_json).toBe(JSON.stringify(policy));
    expect(validateActionValues(update, values, "en")).toEqual([]);

    const unchanged = buildCommand(
      update,
      { ...values, exit_selection_policy_json: "" },
      "en",
    ) as { payload: { update: Record<string, unknown> } };
    expect(unchanged.payload.update).not.toHaveProperty(
      "exit_selection_policy",
    );

    const edited = buildCommand(
      update,
      { ...values, exit_selection_policy_json: JSON.stringify(policy) },
      "en",
    ) as { payload: { update: { exit_selection_policy: unknown } } };
    expect(edited.payload.update.exit_selection_policy).toEqual({
      ...policy,
      denied_exits: [],
      groups: [],
      members: [],
    });

    expect(
      validateActionValues(
        update,
        { ...values, exit_selection_policy_json: "{" },
        "en",
      ),
    ).toEqual(["translated:validation.exit_selection_policy_json_invalid"]);
    expect(
      validateActionValues(
        update,
        {
          ...values,
          exit_selection_policy_json: '{"mode":{"mode":"fixed"}}',
        },
        "en",
      ),
    ).toEqual(["translated:validation.exit_selection_policy_json_invalid"]);
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
      exit_contribution_mode: "disabled",
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
        {
          ...valid,
          tags: Array.from({ length: 13 }, (_, i) => `tag${i}`).join(","),
        },
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

  it("canonicalizes Exit contribution compatibility fields from the mode", () => {
    const createNetwork = manifest.pages
      .flatMap((page) => page.actions)
      .find((action) => action.id === "create_network")!;
    const values = {
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
      exit_contribution_scope: "all_members",
      signaling_port: "6666",
      relay_port: "6667",
      external_host: "",
      auto_connect: true,
    };

    for (const [mode, obligation, scope] of [
      ["autonomous", "not_required", "not_applicable"],
      ["required_on_join", "required", "all_members"],
      ["disabled", "not_required", "not_applicable"],
      ["approval_required", "not_required", "not_applicable"],
    ] as const) {
      const command = buildCommand(
        createNetwork,
        { ...values, exit_contribution_mode: mode },
        "en",
      ) as {
        payload: {
          exit_contribution: string;
          exit_contribution_scope: string;
          exit_contribution_mode: string;
        };
      };
      expect(command.payload).toMatchObject({
        exit_contribution: obligation,
        exit_contribution_scope: scope,
        exit_contribution_mode: mode,
      });
    }

    expect(
      validateActionValues(
        createNetwork,
        {
          ...values,
          exit_contribution_mode: "required_on_join",
          exit_contribution_scope: "not_applicable",
        },
        "en",
      ),
    ).toEqual([
      "translated:validation.exit_contribution_scope_required",
    ]);
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
      exit_contribution_mode: "disabled",
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

  it("builds and strictly validates the authoritative member directory", () => {
    const action = manifest.pages
      .flatMap((page) => page.actions)
      .find((candidate) => candidate.id === "list_network_members")!;
    const networkId = "net_stable_home";
    expect(buildCommand(action, { network_id: networkId }, "en")).toEqual({
      command: "list_network_members",
      payload: { network_id: networkId },
    });

    const outcome = {
      status: "network_members" as const,
      data: {
        network_id: networkId,
        generated_at_unix_ms: 1_700_000_100_000,
        viewer_role: "owner" as const,
        can_manage_members: true,
        members: [
          {
            node_id: "node-owner",
            display_name: "Owner",
            identity_fingerprint: "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
            role: "owner" as const,
            groups: ["owner" as const],
            capabilities: null,
            status: "approved" as const,
            lifecycle_state: "approved" as const,
            revision: 4,
            access_epoch: 4,
            available_actions: [],
            presence: {
              state: "unknown" as const,
              source: "not_observed" as const,
              observed_at_unix_ms: null,
            },
            joined_at_unix_ms: 1_700_000_000_000,
          },
          {
            node_id: "node-member",
            role: "member" as const,
            groups: ["member" as const],
            capabilities: ["file" as const, "relay" as const],
            status: "active" as const,
            lifecycle_state: "active" as const,
            revision: 2,
            access_epoch: 2,
            available_actions: ["suspend", "remove"] as const,
            presence: {
              state: "online" as const,
              source: "live_session" as const,
              observed_at_unix_ms: 1_700_000_090_000,
            },
          },
        ],
      },
    } satisfies NodeCommandOutcome;
    expect(
      validateCommandResponse(
        {
          schema_version: 1,
          request_id: 47,
          server_time: "2026-11-01T00:00:00Z",
          result: { request_id: 47, outcome },
        },
        47,
      ),
    ).toEqual(outcome);
    expect(outcomeSummary(outcome, "zh-CN")).toBe(
      "translated:outcome.network_members_loaded",
    );

    const memberViewer = structuredClone(outcome) as unknown as {
      data: NetworkMemberDirectory;
    };
    memberViewer.data.viewer_role = "member";
    expect(
      validateCommandResponse(
        {
          schema_version: 1,
          request_id: 471,
          server_time: "2026-11-01T00:00:00Z",
          result: {
            request_id: 471,
            outcome: memberViewer as unknown as NodeCommandOutcome,
          },
        },
        471,
      ),
    ).toEqual(memberViewer);

    const duplicate = structuredClone(outcome) as unknown as {
      data: NetworkMemberDirectory;
    };
    duplicate.data.members.push({ ...duplicate.data.members[1] });
    expect(() =>
      validateCommandResponse(
        {
          schema_version: 1,
          request_id: 48,
          server_time: "2026-11-01T00:00:00Z",
          result: {
            request_id: 48,
            outcome: duplicate as unknown as NodeCommandOutcome,
          },
        },
        48,
      ),
    ).toThrow("duplicate member node id: node-member");

    const inconsistent = structuredClone(outcome) as unknown as {
      data: NetworkMemberDirectory;
    };
    inconsistent.data.members[0].presence.state = "offline";
    expect(() =>
      validateCommandResponse(
        {
          schema_version: 1,
          request_id: 49,
          server_time: "2026-11-01T00:00:00Z",
          result: {
            request_id: 49,
            outcome: inconsistent as unknown as NodeCommandOutcome,
          },
        },
        49,
      ),
    ).toThrow("member 0 offline presence is inconsistent");
  });

  it("builds member lifecycle governance commands from server truth", () => {
    const actions = manifest.pages.flatMap((page) => page.actions);
    const preview = actions.find(
      (candidate) => candidate.id === "preview_member_removal",
    )!;
    const change = actions.find(
      (candidate) => candidate.id === "change_member_lifecycle",
    )!;
    const networkId = "net_stable_home";
    const targetNodeId = `node_${"A".repeat(43)}`;

    expect(
      buildCommand(
        preview,
        { network_id: networkId, target_node_id: targetNodeId },
        "en",
      ),
    ).toEqual({
      command: "preview_member_removal",
      payload: { network_id: networkId, target_node_id: targetNodeId },
    });

    expect(
      buildCommand(
        change,
        {
          network_id: networkId,
          target_node_id: targetNodeId,
          action: "remove",
          role: "member",
          expected_revision: "7",
          reason: "  duplicate identity  ",
        },
        "en",
      ),
    ).toEqual({
      command: "change_member_lifecycle",
      payload: {
        network_id: networkId,
        target_node_id: targetNodeId,
        action: "remove",
        role: "member",
        expected_revision: 7,
        reason: "duplicate identity",
      },
    });

    expect(
      buildCommand(
        change,
        {
          network_id: networkId,
          target_node_id: targetNodeId,
          action: "invite",
          role: "member",
          expected_revision: "0",
          reason: "   ",
        },
        "en",
      ),
    ).toEqual({
      command: "change_member_lifecycle",
      payload: {
        network_id: networkId,
        target_node_id: targetNodeId,
        action: "invite",
        role: "member",
        expected_revision: 0,
      },
    });

    expect(() =>
      buildCommand(
        preview,
        { network_id: networkId, target_node_id: "not-a-node" },
        "en",
      ),
    ).toThrow("translated:validation.node_id");
    expect(() =>
      buildCommand(
        change,
        {
          network_id: networkId,
          target_node_id: targetNodeId,
          action: "promote",
          role: "member",
          expected_revision: "1",
        },
        "en",
      ),
    ).toThrow("translated:validation.member_lifecycle_action");
  });

  it("validates member removal preview and lifecycle change outcomes", () => {
    const preview: NodeCommandOutcome = {
      status: "member_removal_preview",
      data: {
        network_id: "net_stable_home",
        target_node_id: `node_${"B".repeat(43)}`,
        role: "member",
        status: "active",
        lifecycle_state: "active",
        revision: 3,
        access_epoch: 3,
        presence: {
          state: "online",
          source: "live_session",
          observed_at_unix_ms: 1_700_000_090_000,
        },
        can_remove: true,
        network_access_revoked: true,
        file_access_revoked: true,
        incomplete_transfers_resume_blocked: true,
        credentials_revoked: true,
        old_access_epoch_invalidated: true,
        local_files_deleted: false,
        safe_undo_requires_reapproval: true,
        safe_undo_restores_old_credentials: false,
        impact_keys: [
          "member_removal.impact.network_access",
          "member_removal.impact.file_access",
          "member_removal.impact.incomplete_transfers",
          "member_removal.impact.credentials",
          "member_removal.impact.local_files_kept",
          "member_removal.impact.safe_undo",
        ],
      },
    } satisfies NodeCommandOutcome;

    expect(
      validateCommandResponse(
        {
          schema_version: 1,
          request_id: 61,
          server_time: "2026-11-01T00:00:00Z",
          result: { request_id: 61, outcome: preview },
        },
        61,
      ),
    ).toEqual(preview);

    const deleted = structuredClone(preview) as unknown as {
      data: Record<string, unknown>;
    };
    deleted.data.local_files_deleted = true;
    expect(() =>
      validateCommandResponse(
        {
          schema_version: 1,
          request_id: 62,
          server_time: "2026-11-01T00:00:00Z",
          result: {
            request_id: 62,
            outcome: deleted as unknown as NodeCommandOutcome,
          },
        },
        62,
      ),
    ).toThrow("member removal preview must never delete local files");

    const revived = structuredClone(preview) as unknown as {
      data: Record<string, unknown>;
    };
    revived.data.safe_undo_restores_old_credentials = true;
    expect(() =>
      validateCommandResponse(
        {
          schema_version: 1,
          request_id: 63,
          server_time: "2026-11-01T00:00:00Z",
          result: {
            request_id: 63,
            outcome: revived as unknown as NodeCommandOutcome,
          },
        },
        63,
      ),
    ).toThrow("member removal preview must never restore old credentials");

    const change: NodeCommandOutcome = {
      status: "member_lifecycle_changed",
      data: {
        network_id: "net_stable_home",
        target_node_id: `node_${"B".repeat(43)}`,
        action: "remove",
        status: "removed",
        revision: 4,
        access_epoch: 4,
        operation_id: `op_${"C".repeat(43)}`,
        audit_id: `audit_${"D".repeat(43)}`,
        idempotent: false,
        access_revoked: true,
        requires_reauthentication: true,
        requires_reapproval: true,
      },
    } satisfies NodeCommandOutcome;

    expect(
      validateCommandResponse(
        {
          schema_version: 1,
          request_id: 64,
          server_time: "2026-11-01T00:00:00Z",
          result: { request_id: 64, outcome: change },
        },
        64,
      ),
    ).toEqual(change);

    const stale = structuredClone(change) as unknown as {
      data: Record<string, unknown>;
    };
    stale.data.revision = 0;
    expect(() =>
      validateCommandResponse(
        {
          schema_version: 1,
          request_id: 65,
          server_time: "2026-11-01T00:00:00Z",
          result: {
            request_id: 65,
            outcome: stale as unknown as NodeCommandOutcome,
          },
        },
        65,
      ),
    ).toThrow("member lifecycle revision invalid");
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
        {
          network_id: networkId,
          reason: "Need access from the branch office.",
        },
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

  it("matches the shared discovery flow fixture", () => {
    const invitationCode = `fkinv_${"A".repeat(43)}`;
    const fixture = JSON.parse(
      JSON.stringify(discoveryParityFixture).replaceAll(
        "$INVITATION_CODE",
        invitationCode,
      ),
    ) as {
      schema_version: number;
      steps: Array<{
        action_id: string;
        values: Record<string, string>;
        wire: NodeCommand;
      }>;
    };
    expect(fixture.schema_version).toBe(1);
    expect(fixture.steps.map((step) => step.action_id)).toEqual([
      "create_network",
      "publish_network_discovery",
      "preview_join_network",
      "join_network",
      "review_join_request",
      "create_invitation",
      "preview_invitation",
      "redeem_invitation",
      "retry_discovery_index",
    ]);

    const actions = new Map(
      manifest.pages
        .flatMap((page) => page.actions)
        .map((action) => [action.id, action]),
    );
    for (const step of fixture.steps) {
      const action = actions.get(step.action_id);
      expect(action, `missing manifest action ${step.action_id}`).toBeDefined();
      expect(action!.command).toBe(step.wire.command);
      for (const field of action!.fields) {
        expect(
          step.values,
          `${step.action_id} fixture value ${field.id}`,
        ).toHaveProperty(field.id);
      }
      expect(
        buildCommand(action!, step.values, "en"),
        `WebUI command mismatch for ${step.action_id}`,
      ).toEqual(step.wire);
    }
  });

  it("validates the policy tester manifest contract and builds its strict command", () => {
    const nodeId = `node_${"N".repeat(43)}`;
    const networkId = `net_${"W".repeat(43)}`;
    const policyAction: NodeUiAction = {
      id: "test_policy",
      label_key: "action.test_policy",
      command: "test_policy",
      backend_status: "implemented",
      permission: "manage_networks",
      audit: { policy: "none", action: null },
      availability: "always",
      fields: [
        {
          id: "policy_json",
          label_key: "field.policy_json",
          input: "policy_json",
          required: true,
          validation: "policy_json",
        },
      ],
      confirmation: null,
    };
    const settings = manifest.pages.find((page) => page.id === "settings");
    expect(settings).toBeDefined();
    const manifestPolicyAction = settings!.actions.find(
      (action) => action.id === "test_policy",
    );
    expect(manifestPolicyAction).toEqual(policyAction);
    expect(validateManifestResponse(manifestResponse(manifest))).toEqual(
      manifest,
    );

    const input: PolicyTestInput = {
      network_id: networkId,
      actor: nodeId,
      role: "member",
      groups: [],
      membership_active: true,
      recent_reauthentication: false,
      emergency_coordinator: false,
      now_unix_ms: 1_700_000_000_000,
      request: {
        action: "read",
        resource: "network",
        resource_network_id: networkId,
        object_id: null,
        intent: "read_only",
      },
      local_policy: null,
      network_acl: null,
    };
    expect(
      buildCommand(
        manifestPolicyAction!,
        { policy_json: JSON.stringify(input) },
        "zh-CN",
      ),
    ).toEqual({ command: "test_policy", payload: input });
  });

  it("rejects malformed policy JSON with exact paths", () => {
    const nodeId = `node_${"N".repeat(43)}`;
    const networkId = `net_${"W".repeat(43)}`;
    const valid = {
      network_id: networkId,
      actor: nodeId,
      role: "member",
      groups: [] as string[],
      membership_active: true,
      recent_reauthentication: false,
      emergency_coordinator: false,
      now_unix_ms: 1_700_000_000_000,
      request: {
        action: "read",
        resource: "network",
        resource_network_id: networkId,
        object_id: null,
        intent: "read_only",
      },
      local_policy: null,
      network_acl: null,
    };

    function parseError(source: string): PolicyJsonError {
      try {
        parsePolicyTestInputJson(source);
      } catch (error) {
        expect(error).toBeInstanceOf(PolicyJsonError);
        return error as PolicyJsonError;
      }
      throw new Error("expected policy JSON to be rejected");
    }

    const duplicate = JSON.stringify(valid).replace(
      `{"network_id":${JSON.stringify(networkId)}`,
      `{"network_id":${JSON.stringify(networkId)},"network_id":${JSON.stringify(networkId)}`,
    );
    expect(parseError(duplicate).path).toBe("/network_id");

    const unknown = JSON.stringify({ ...valid, unexpected: true });
    expect(parseError(unknown).path).toBe("/unexpected");
    expect(parseError(`${JSON.stringify(valid)} trailing`).path).toBe("/");

    const invalidArrayElement = JSON.stringify({
      ...valid,
      groups: [`group_${"G".repeat(43)}`, "not-a-group"],
    });
    expect(parseError(invalidArrayElement).path).toBe("/groups/1");
  });

  it("validates and summarizes an explainable policy test result", () => {
    const ruleId = `srule_${"R".repeat(43)}`;
    const result: PolicyTestResult = {
      decision: {
        allowed: false,
        code: "explicit_deny",
        evaluated_at_unix_ms: 1_700_000_000_100,
        expired_grant_ids: [],
        layer: "local",
        local_policy_revision: 7,
        matched_allow_rule_ids: [],
        matched_deny_rule_ids: [ruleId],
        network_acl_revision: 4,
        reason: "explicit_deny",
        reauthentication_required_grant_ids: [],
        reauthentication_satisfied: true,
        revoked_grant_ids: [],
        rule_id: ruleId,
      },
      explanations: [
        {
          source: "local_policy_rule",
          source_path: "/local_policy/rules/0",
          rule_id: ruleId,
          effect: "deny",
          subjects: [{ kind: "any_member" }],
          resources: ["network"],
          actions: ["read"],
          conditions: [{ kind: "membership_active" }],
          matched: true,
          applied: true,
          precedence: "explicit_deny",
        },
      ],
      precedence: [
        {
          precedence: "explicit_deny",
          triggered: true,
          source: "local_policy_rule",
          rule_id: ruleId,
        },
      ],
      validation_issues: [
        {
          path: "/local_policy/rules/0/actions/0",
          code: "unknown_action",
          source: "local_policy",
          related_rule_id: ruleId,
          message: "Unknown policy action",
        },
      ],
    };
    expect(validatePolicyTestResult(result)).toEqual(result);
    const outcome = validateCommandOutcome({
      status: "policy_test",
      data: result,
    });
    expect(outcome).toEqual({ status: "policy_test", data: result });
    expect(outcomeSummary(outcome, "zh-CN")).toBe(
      "translated:outcome.policy_test",
    );
  });

  it("strictly parses policy snapshots and rejects unknown, duplicate, or malformed data", () => {
    const snapshot: PolicySnapshot = {
      local_policy: null,
      network_acls: [],
    };
    const source = JSON.stringify(snapshot);
    expect(parsePolicySnapshotJson(source)).toEqual(snapshot);
    expect(validatePolicySnapshot(snapshot)).toEqual(snapshot);

    function parseError(input: string): PolicyJsonError {
      try {
        parsePolicySnapshotJson(input);
      } catch (error) {
        expect(error).toBeInstanceOf(PolicyJsonError);
        return error as PolicyJsonError;
      }
      throw new Error("expected policy snapshot to be rejected");
    }

    expect(parseError('{"network_acls":[],"unexpected":true}').path).toBe(
      "/unexpected",
    );
    expect(parseError('{"network_acls":[],"network_acls":[]}').path).toBe(
      "/network_acls",
    );
    try {
      validatePolicySnapshot({ local_policy: [] });
      throw new Error("expected malformed policy snapshot to be rejected");
    } catch (error) {
      expect(error).toBeInstanceOf(PolicyJsonError);
      expect((error as PolicyJsonError).path).toBe("/local_policy");
    }
  });

  it("builds revisioned preview, apply, rollback, and policy read commands", () => {
    const actions = new Map(
      manifest.pages
        .flatMap((page) => page.actions)
        .map((action) => [action.id, action]),
    );
    const networkId = `net_${"P".repeat(43)}`;
    const snapshot: PolicySnapshot = {
      local_policy: null,
      network_acls: [],
    };
    const candidateJson = JSON.stringify(snapshot);

    expect(buildCommand(actions.get("get_policy")!, {}, "en")).toEqual({
      command: "get_policy",
    });
    expect(
      buildCommand(
        actions.get("preview_policy")!,
        { expected_revision: "3", candidate_json: candidateJson },
        "en",
      ),
    ).toEqual({
      command: "preview_policy",
      payload: { expected_revision: 3, candidate: snapshot },
    });
    expect(
      buildCommand(
        actions.get("preview_policy_rollback")!,
        { expected_revision: "5", target_revision: "2" },
        "en",
      ),
    ).toEqual({
      command: "preview_policy_rollback",
      payload: { expected_revision: 5, target_revision: 2 },
    });
    expect(
      buildCommand(
        actions.get("apply_policy")!,
        {
          expected_revision: "5",
          candidate_json: candidateJson,
          network_ids: networkId,
          includes_local_policy: "true",
          confirm_privilege_expansion: "false",
        },
        "en",
      ),
    ).toEqual({
      command: "apply_policy",
      payload: {
        expected_revision: 5,
        candidate: snapshot,
        network_ids: [networkId],
        includes_local_policy: true,
        confirm_privilege_expansion: false,
      },
    });
    expect(
      buildCommand(
        actions.get("rollback_policy")!,
        {
          expected_revision: "5",
          target_revision: "2",
          network_ids: networkId,
          includes_local_policy: "true",
          confirm_privilege_expansion: "true",
        },
        "en",
      ),
    ).toEqual({
      command: "rollback_policy",
      payload: {
        expected_revision: 5,
        target_revision: 2,
        network_ids: [networkId],
        includes_local_policy: true,
        confirm_privilege_expansion: true,
      },
    });
    expect(buildCommand(actions.get("get_policy_history")!, {}, "en")).toEqual({
      command: "get_policy_history",
    });
  });

  it("builds strict traffic policy preview, apply, rollback, read, and test commands", () => {
    const actions = new Map(
      manifest.pages
        .flatMap((page) => page.actions)
        .map((action) => [action.id, action]),
    );
    const nodeId = `node_${"T".repeat(43)}`;
    const exitId = `node_${"E".repeat(43)}`;
    const networkId = `net_${"R".repeat(43)}`;
    const ruleId = `rule_${"I".repeat(43)}`;
    const draft: TrafficPolicyDraft = {
      node_id: nodeId,
      default_policy: {
        action: { kind: "reject" },
        confirmed_by: "owner",
        confirmed_at_unix_ms: 1_700_000_000_000,
      },
      failure_policy: { direct_fallback: null },
      rules: [
        {
          rule_id: ruleId,
          priority: 1,
          enabled: true,
          description: "Work traffic",
          matcher: {
            domain_suffixes: ["example.com"],
            destination_ports: [{ start: 443, end: 443 }],
            protocols: ["tcp"],
            applications: [{ kind: "package_id", value: "com.example.work" }],
          },
          action: { kind: "exit", value: exitId },
        },
      ],
    };
    const current: TrafficPolicy = { ...draft, revision: 7 };
    const candidate: TrafficPolicy = { ...draft, revision: 8 };
    const testInput: TrafficPolicyTestInput = {
      network_id: networkId,
      flow: {
        domain: "api.example.com",
        destination_ip: "203.0.113.10",
        destination_port: 443,
        protocol: "tcp",
        application: { kind: "package_id", value: "com.example.work" },
      },
      source: { kind: "manual" },
    };

    expect(buildCommand(actions.get("get_traffic_policy")!, {}, "en")).toEqual({
      command: "get_traffic_policy",
    });
    expect(
      buildCommand(
        actions.get("preview_traffic_policy")!,
        { expected_revision: "7", draft_json: JSON.stringify(draft) },
        "en",
      ),
    ).toEqual({
      command: "preview_traffic_policy",
      payload: { expected_revision: 7, draft },
    });
    expect(
      buildCommand(
        actions.get("preview_traffic_policy_rollback")!,
        { expected_revision: "8", target_revision: "6" },
        "en",
      ),
    ).toEqual({
      command: "preview_traffic_policy_rollback",
      payload: { expected_revision: 8, target_revision: 6 },
    });
    expect(
      buildCommand(
        actions.get("apply_traffic_policy")!,
        {
          expected_revision: "7",
          draft_json: JSON.stringify(draft),
          confirm_risk: "true",
        },
        "en",
      ),
    ).toEqual({
      command: "apply_traffic_policy",
      payload: { expected_revision: 7, draft, confirm_risk: true },
    });
    expect(
      buildCommand(
        actions.get("rollback_traffic_policy")!,
        { expected_revision: "8", target_revision: "6", confirm_risk: "true" },
        "en",
      ),
    ).toEqual({
      command: "rollback_traffic_policy",
      payload: { expected_revision: 8, target_revision: 6, confirm_risk: true },
    });
    expect(
      buildCommand(actions.get("get_traffic_policy_history")!, {}, "en"),
    ).toEqual({ command: "get_traffic_policy_history" });
    expect(
      buildCommand(
        actions.get("test_traffic_policy")!,
        {
          current_json: JSON.stringify(current),
          candidate_json: JSON.stringify(candidate),
          input_json: JSON.stringify(testInput),
        },
        "en",
      ),
    ).toEqual({
      command: "test_traffic_policy",
      payload: { current, candidate, input: testInput },
    });
  });

  it("rejects malformed traffic policy JSON with exact paths", () => {
    const nodeId = `node_${"T".repeat(43)}`;
    const draft = {
      node_id: nodeId,
      default_policy: {
        action: { kind: "reject" },
        confirmed_by: "owner",
        confirmed_at_unix_ms: 1,
      },
      rules: [
        {
          rule_id: `rule_${"I".repeat(43)}`,
          priority: 1,
          matcher: { protocols: ["tcp"] },
          action: { kind: "direct" },
        },
      ],
    };
    expect(parseTrafficPolicyDraftJson(JSON.stringify(draft))).toEqual({
      ...draft,
      rules: [{ ...draft.rules[0], enabled: true, description: "" }],
    });

    function draftError(value: unknown): TrafficPolicyJsonError {
      try {
        parseTrafficPolicyDraftJson(JSON.stringify(value));
      } catch (error) {
        expect(error).toBeInstanceOf(TrafficPolicyJsonError);
        return error as TrafficPolicyJsonError;
      }
      throw new Error("expected traffic policy JSON to be rejected");
    }

    expect(draftError({ ...draft, revision: 2 }).path).toBe("/revision");
    const duplicate = JSON.stringify(draft).replace(
      `"node_id":${JSON.stringify(nodeId)}`,
      `"node_id":${JSON.stringify(nodeId)},"node_id":${JSON.stringify(nodeId)}`,
    );
    try {
      parseTrafficPolicyDraftJson(duplicate);
      throw new Error("expected duplicate traffic policy fields to be rejected");
    } catch (error) {
      expect(error).toBeInstanceOf(TrafficPolicyJsonError);
      expect((error as TrafficPolicyJsonError).path).toBe("/node_id");
    }
    expect(
      draftError({
        ...draft,
        rules: [{ ...draft.rules[0], priority: 2 }],
      }).path,
    ).toBe("/rules/0/priority");
    expect(
      draftError({
        ...draft,
        failure_policy: {
          direct_fallback: {
            allowed_failures: ["dns_pollution_detected"],
            confirmed_by: "owner",
            confirmed_at_unix_ms: 1,
          },
        },
      }).path,
    ).toBe("/failure_policy/direct_fallback/allowed_failures/0");

    const testInput: TrafficPolicyTestInput = {
      network_id: `net_${"R".repeat(43)}`,
      flow: {
        domain: null,
        destination_ip: "203.0.113.10",
        destination_port: 443,
        protocol: "tcp",
        application: { kind: "unknown" },
      },
      source: { kind: "manual" },
    };
    expect(parseTrafficPolicyTestInputJson(JSON.stringify(testInput))).toEqual(
      testInput,
    );
    const parsedPolicy = parseTrafficPolicyJson(
      JSON.stringify({ ...draft, revision: 9 }),
    );
    expect(parsedPolicy.revision).toBe(9);
  });

  it("validates policy, preview, apply, history, and rollback outcomes", () => {
    const networkId = `net_${"P".repeat(43)}`;
    const snapshot: PolicySnapshot = {
      local_policy: null,
      network_acls: [],
    };
    const risk: PolicyRiskSummary = {
      level: "none",
      privilege_expansion: false,
      reasons: [],
      requires_explicit_confirmation: false,
      requires_recent_reauthentication: false,
    };
    const audit = (
      action: "apply" | "rollback",
      revision: number,
    ): PolicyHistoryEntry => ({
      action,
      actor: "node-admin",
      affected_objects: [],
      diffs: [],
      local_policy_revision: null,
      network_acl_revisions: {},
      occurred_at: "2026-11-08T00:00:00Z",
      previous_revision: revision - 1,
      revision,
      risk,
      source: "webui",
      transaction_id: `txn_${action}_${revision}`,
      ...(action === "rollback" ? { target_revision: 2 } : {}),
    });
    const preview: PolicyPreview = {
      affected_objects: [],
      can_apply: false,
      candidate: snapshot,
      current: snapshot,
      current_revision: 4,
      diffs: [],
      expected_revision: 4,
      has_changes: false,
      includes_local_policy: false,
      network_ids: [networkId],
      revision_conflict: false,
      risk,
      rollback_target_revision: null,
    };
    const applied: PolicyApplyResult = {
      affected_objects: [],
      audit: audit("apply", 5),
      diffs: [],
      no_op: false,
      previous_revision: 4,
      revision: 5,
      risk,
      snapshot,
    };
    const history: PolicyHistoryEntry[] = [audit("apply", 5)];
    const rolledBack: PolicyRollbackResult = {
      affected_objects: [],
      audit: audit("rollback", 6),
      diffs: [],
      previous_revision: 5,
      revision: 6,
      risk,
      snapshot,
      target_revision: 2,
    };

    for (const outcome of [
      { status: "policy" as const, data: { revision: 5, snapshot } },
      { status: "policy_preview" as const, data: preview },
      { status: "policy_applied" as const, data: applied },
      {
        status: "policy_history" as const,
        data: { current_revision: 5, entries: history },
      },
      { status: "policy_rolled_back" as const, data: rolledBack },
    ]) {
      expect(validateCommandOutcome(outcome)).toEqual(outcome);
    }
  });

  it("keeps no-op policy applies at the same revision without audit", () => {
    const snapshot: PolicySnapshot = {
      local_policy: null,
      network_acls: [],
    };
    const risk: PolicyRiskSummary = {
      level: "none",
      privilege_expansion: false,
      reasons: [],
      requires_explicit_confirmation: false,
      requires_recent_reauthentication: false,
    };
    const noOp: PolicyApplyResult = {
      affected_objects: [],
      diffs: [],
      no_op: true,
      previous_revision: 5,
      revision: 5,
      risk,
      snapshot,
    };
    expect(
      validateCommandOutcome({ status: "policy_applied", data: noOp }),
    ).toEqual({ status: "policy_applied", data: noOp });

    expect(() =>
      validateCommandOutcome({
        status: "policy_applied",
        data: { ...noOp, revision: 6 },
      }),
    ).toThrow("policy no-op moved revision");
    expect(() =>
      validateCommandOutcome({
        status: "policy_applied",
        data: {
          ...noOp,
          audit: {
            action: "apply",
            actor: "node-admin",
            affected_objects: [],
            diffs: [],
            network_acl_revisions: {},
            occurred_at: "2026-11-08T00:00:00Z",
            previous_revision: 5,
            revision: 5,
            risk,
            source: "webui",
            transaction_id: "txn_noop",
          },
        },
      }),
    ).toThrow("policy no-op unexpectedly has audit");
  });

  it("requires rollback results to carry a rollback audit event", () => {
    const snapshot: PolicySnapshot = {
      local_policy: null,
      network_acls: [],
    };
    const risk: PolicyRiskSummary = {
      level: "none",
      privilege_expansion: false,
      reasons: [],
      requires_explicit_confirmation: false,
      requires_recent_reauthentication: false,
    };
    const result: PolicyRollbackResult = {
      affected_objects: [],
      audit: {
        action: "rollback",
        actor: "node-admin",
        affected_objects: [],
        diffs: [],
        network_acl_revisions: {},
        occurred_at: "2026-11-08T00:00:00Z",
        previous_revision: 6,
        revision: 7,
        risk,
        source: "webui",
        target_revision: 3,
        transaction_id: "txn_rollback_7",
      },
      diffs: [],
      previous_revision: 6,
      revision: 7,
      risk,
      snapshot,
      target_revision: 3,
    };
    expect(
      validateCommandOutcome({ status: "policy_rolled_back", data: result }),
    ).toEqual({ status: "policy_rolled_back", data: result });

    const mismatched = structuredClone(result);
    mismatched.audit.action = "apply";
    expect(() =>
      validateCommandOutcome({
        status: "policy_rolled_back",
        data: mismatched,
      }),
    ).toThrow("policy rollback audit action mismatch");
  });

  it("strictly validates traffic policy outcomes and summarizes them", () => {
    const nodeId = `node_${"T".repeat(43)}`;
    const exitId = `node_${"E".repeat(43)}`;
    const networkId = `net_${"R".repeat(43)}`;
    const ruleId = `rule_${"I".repeat(43)}`;
    const defaultPolicy = {
      action: { kind: "reject" as const },
      confirmed_by: "owner" as const,
      confirmed_at_unix_ms: 1_700_000_000_000,
    };
    const currentPolicy: TrafficPolicy = {
      node_id: nodeId,
      revision: 7,
      default_policy: defaultPolicy,
      rules: [],
    };
    const candidatePolicy: TrafficPolicy = {
      ...currentPolicy,
      revision: 8,
      rules: [
        {
          rule_id: ruleId,
          priority: 1,
          enabled: true,
          description: "Send work traffic through Exit",
          matcher: {
            domain_suffixes: ["example.com"],
            protocols: ["tcp"],
          },
          action: { kind: "exit", value: exitId },
        },
      ],
    };
    const historyEntry = {
      action: "apply" as const,
      actor: "node-owner",
      affected_objects: [{ kind: "rule" as const, object_id: ruleId }],
      applied_revision: 8,
      candidate_revision: 8,
      content_hash: "hash-8",
      diffs: [
        {
          path: "/rules/0",
          change: "rule_added" as const,
          after: candidatePolicy.rules?.[0],
        },
      ],
      occurred_at: "2026-11-08T00:00:00Z",
      outcome: "applied" as const,
      previous_revision: 7,
      source: "webui",
      transaction_id: "txn-traffic-8",
    };
    const preview = {
      expected_revision: 7,
      current_revision: 7,
      candidate_revision: 8,
      revision_conflict: false,
      current: currentPolicy,
      candidate: candidatePolicy,
      has_changes: true,
      can_apply: true,
      diffs: historyEntry.diffs,
      affected_objects: historyEntry.affected_objects,
      risks: [
        {
          rule_id: ruleId,
          priority: 1,
          enabled: true,
          risks: [{ kind: "any_port" as const }],
        },
      ],
      rollback_target_revision: null,
    };
    const applied = {
      previous_revision: 7,
      revision: 8,
      no_op: false,
      policy: candidatePolicy,
      content_hash: "hash-8",
      diffs: historyEntry.diffs,
      affected_objects: historyEntry.affected_objects,
      history: historyEntry,
    };
    const rolledBackHistory = {
      ...historyEntry,
      action: "rollback" as const,
      outcome: "rolled_back" as const,
      target_revision: 7,
      transaction_id: "txn-traffic-9",
    };
    const rolledBack = {
      previous_revision: 8,
      revision: 9,
      target_revision: 7,
      policy: { ...currentPolicy, revision: 9 },
      content_hash: "hash-9",
      diffs: [],
      affected_objects: [],
      history: rolledBackHistory,
    };
    const comparison = {
      network_id: networkId,
      source: { kind: "manual" as const },
      before: {
        visibility: "full" as const,
        action: { kind: "direct" as const },
        reason: "default" as const,
        matched_rule: null,
        failure_decisions: [],
      },
      after: {
        visibility: "full" as const,
        action: { kind: "exit" as const, value: exitId },
        reason: "first_match" as const,
        matched_rule: ruleId,
        failure_decisions: [],
      },
      delta: {
        changed: true,
        route_changed: true,
        matched_rule_changed: true,
        failure_outcomes_changed: false,
      },
    };
    const outcomes: NodeCommandOutcome[] = [
      {
        status: "traffic_policy",
        data: { revision: 7, policy: currentPolicy, content_hash: "hash-7" },
      },
      { status: "traffic_policy_preview", data: preview },
      { status: "traffic_policy_applied", data: applied },
      {
        status: "traffic_policy_history",
        data: { current_revision: 8, entries: [historyEntry] },
      },
      { status: "traffic_policy_rolled_back", data: rolledBack },
      { status: "traffic_policy_test", data: comparison },
    ];

    for (const outcome of outcomes)
      expect(validateCommandOutcome(outcome)).toEqual(outcome);
    expect(outcomeSummary(outcomes[0], "zh-CN")).toBe(
      "translated:outcome.traffic_policy_loaded",
    );
    expect(outcomeSummary(outcomes[1], "zh-CN")).toBe(
      "translated:outcome.traffic_policy_previewed",
    );
    expect(outcomeSummary(outcomes[2], "zh-CN")).toBe(
      "translated:outcome.traffic_policy_applied",
    );
    expect(outcomeSummary(outcomes[3], "zh-CN")).toBe(
      "translated:outcome.traffic_policy_history",
    );
    expect(outcomeSummary(outcomes[4], "zh-CN")).toBe(
      "translated:outcome.traffic_policy_rolled_back",
    );
    expect(outcomeSummary(outcomes[5], "zh-CN")).toBe(
      "translated:outcome.traffic_policy_test",
    );

    expect(() =>
      validateCommandOutcome({
        status: "traffic_policy_applied",
        data: { ...applied, history: undefined },
      }),
    ).toThrow("traffic policy apply is missing history");
    expect(() =>
      validateCommandOutcome({
        status: "traffic_policy_preview",
        data: {
          ...preview,
          candidate: { ...candidatePolicy, priority: 0 },
        },
      }),
    ).toThrow("traffic policy preview candidate invalid");
  });
});
