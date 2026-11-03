import { describe, expect, it } from "vitest";
import type {
  NetworkDiscoveryDocument,
  NodeDiscoveryIndexRecordSnapshot,
  NodeDiscoveryIndexSnapshot,
  NodeDiscoveryIndexState,
  NodeSnapshot,
} from "../../contracts/typescript/node-contract-v1";
import {
  DISCOVERY_RESULT_STALE_AFTER_MS,
  EMPTY_DISCOVERY_FILTERS,
  discoveryHasStaleResults,
  projectDiscovery,
  type DiscoveryFilters,
} from "./discovery";

const NOW = 1_700_000_000_000;

function document(
  name: string,
  overrides: Partial<NetworkDiscoveryDocument> = {},
): NetworkDiscoveryDocument {
  return {
    schema_version: 3,
    network_id: `net_${name}`,
    public_key: `${name}-public-key`,
    publisher: "publisher-node",
    administrators: [],
    policy_revision: 1,
    privacy_policy_revision: 1,
    member_limit: null,
    member_count: 1,
    exit_contribution: "not_required",
    privacy_changes: [],
    name,
    purpose: `${name} purpose`,
    language: "en",
    region: "US",
    tags: ["private"],
    capability_summary: ["relay"],
    visibility: "public",
    join_mode: "open",
    owner: "owner-node",
    revision: 1,
    issued_at_unix_ms: NOW - 1_000,
    expires_at_unix_ms: NOW + 60_000,
    revoked_at_unix_ms: null,
    ...overrides,
  };
}

function record(
  name: string,
  overrides: {
    document?: Partial<NetworkDiscoveryDocument>;
    discoverable?: boolean;
    accepted_at_unix_ms?: number;
    invalid_reason?: string | null;
  } = {},
): NodeDiscoveryIndexRecordSnapshot {
  return {
    accepted_at_unix_ms: overrides.accepted_at_unix_ms ?? NOW,
    discoverable: overrides.discoverable ?? true,
    digest: `${name}-digest`,
    invalid_reason: overrides.invalid_reason ?? null,
    record: {
      document: document(name, overrides.document),
      signature: `${name}-signature`,
    },
  };
}

function index(
  state: NodeDiscoveryIndexState,
  records: NodeDiscoveryIndexRecordSnapshot[],
  overrides: Partial<NodeDiscoveryIndexSnapshot> = {},
): NodeDiscoveryIndexSnapshot {
  return {
    data_plane_affected: false,
    index_url: "https://discovery.example",
    last_attempt_unix_ms: NOW,
    last_error: null,
    last_success_unix_ms: state === "ready" ? NOW : null,
    records,
    rejected_record_count: 0,
    search_history_stored: false,
    state,
    ...overrides,
  };
}

function snapshot(
  state: NodeDiscoveryIndexState,
  records: NodeDiscoveryIndexRecordSnapshot[],
  overrides: Partial<NodeDiscoveryIndexSnapshot> = {},
): NodeSnapshot {
  return {
    revision: 1,
    lifecycle: {
      active_networks: 0,
      configured_networks: 0,
      failed_networks: [],
      phase: "running",
    },
    networks: [],
    peers: [],
    logs: [],
    discovery_index: index(state, records, overrides),
  };
}

function withFilters(overrides: Partial<DiscoveryFilters>): DiscoveryFilters {
  return { ...EMPTY_DISCOVERY_FILTERS, ...overrides };
}

describe("discovery projection", () => {
  it("searches names, tags, stable capability names and join-mode vocabulary", () => {
    const value = snapshot("ready", [
      record("family", {
        document: {
          tags: ["smart-home"],
          capability_summary: ["exit", "relay"],
          join_mode: "approval_required",
        },
      }),
      record("studio", {
        document: {
          language: "zh-CN",
          region: "CN",
          tags: ["creative"],
          capability_summary: ["file"],
          join_mode: "invite_only",
        },
      }),
    ]);

    expect(
      projectDiscovery(value, "family", EMPTY_DISCOVERY_FILTERS, NOW).results[0]
        .title,
    ).toBe("family");
    expect(
      projectDiscovery(value, "smart-home", EMPTY_DISCOVERY_FILTERS, NOW)
        .results[0].title,
    ).toBe("family");
    expect(
      projectDiscovery(value, "exit", EMPTY_DISCOVERY_FILTERS, NOW).results[0]
        .title,
    ).toBe("family");
    expect(
      projectDiscovery(value, "approval", EMPTY_DISCOVERY_FILTERS, NOW)
        .results[0].title,
    ).toBe("family");
    expect(
      projectDiscovery(value, "invitation", EMPTY_DISCOVERY_FILTERS, NOW)
        .results[0].title,
    ).toBe("studio");
  });

  it("filters public/private, join mode, language and region", () => {
    const value = snapshot("ready", [
      record("public-open", {
        document: {
          visibility: "public",
          join_mode: "open",
          language: "en",
          region: "US",
        },
      }),
      record("private-approval", {
        document: {
          visibility: "private",
          join_mode: "approval_required",
          language: "zh-CN",
          region: "CN",
        },
      }),
    ]);

    expect(
      projectDiscovery(
        value,
        "",
        withFilters({ visibility: "private" }),
        NOW,
      ).results.map((result) => result.title),
    ).toEqual(["private-approval"]);
    expect(
      projectDiscovery(value, "", withFilters({ join_mode: "open" }), NOW)
        .results[0].title,
    ).toBe("public-open");
    expect(
      projectDiscovery(value, "", withFilters({ language: "zh-cn" }), NOW)
        .results[0].title,
    ).toBe("private-approval");
    expect(
      projectDiscovery(value, "", withFilters({ region: "US" }), NOW).results[0]
        .title,
    ).toBe("public-open");
  });

  it("rejects expired, revoked, undiscoverable, invalid and old-schema records", () => {
    const value = snapshot("ready", [
      record("valid"),
      record("expired", { document: { expires_at_unix_ms: NOW } }),
      record("revoked", { document: { revoked_at_unix_ms: NOW - 1 } }),
      record("hidden", { discoverable: false }),
      record("invalid", { invalid_reason: "signature" }),
      record("legacy", { document: { schema_version: 1 } }),
    ]);

    const projection = projectDiscovery(
      value,
      "",
      EMPTY_DISCOVERY_FILTERS,
      NOW,
    );
    expect(projection.results.map((result) => result.title)).toEqual(["valid"]);
    expect(projection.validRecordCount).toBe(1);
    expect(projection.state).toBe("ready");
    expect(projection.noMoreResults).toBe(true);
  });

  it("distinguishes empty, no matches, failure, degraded partial and stale results", () => {
    expect(
      projectDiscovery(snapshot("ready", []), "", EMPTY_DISCOVERY_FILTERS, NOW)
        .state,
    ).toBe("empty");

    const complete = snapshot("ready", [record("home")]);
    const noMatches = projectDiscovery(
      complete,
      "missing",
      EMPTY_DISCOVERY_FILTERS,
      NOW,
    );
    expect(noMatches.state).toBe("no_matches");
    expect(noMatches.partial).toBe(false);

    const failed = snapshot("failed", [], {
      last_error: { category: "timeout", retryable: true },
    });
    expect(
      projectDiscovery(failed, "", EMPTY_DISCOVERY_FILTERS, NOW),
    ).toMatchObject({
      state: "failed",
      errorCategory: "timeout",
    });

    const degraded = snapshot("degraded", [record("home")], {
      last_error: { category: "offline", retryable: true },
    });
    const degradedProjection = projectDiscovery(
      degraded,
      "",
      EMPTY_DISCOVERY_FILTERS,
      NOW,
    );
    expect(degradedProjection.state).toBe("degraded");
    expect(degradedProjection.partial).toBe(true);
    expect(degradedProjection.noMoreResults).toBe(false);
    expect(degradedProjection.results[0].stale).toBe(true);

    const stale = projectDiscovery(
      snapshot("ready", [
        record("old", {
          accepted_at_unix_ms: NOW - DISCOVERY_RESULT_STALE_AFTER_MS - 1,
        }),
      ]),
      "",
      EMPTY_DISCOVERY_FILTERS,
      NOW,
    );
    expect(stale.results[0].stale).toBe(true);
    expect(discoveryHasStaleResults(stale)).toBe(true);
  });

  it("treats rejected records in a ready response as partial, never empty", () => {
    const rejected = snapshot("ready", [record("home")], {
      rejected_record_count: 2,
    });
    const projection = projectDiscovery(
      rejected,
      "missing",
      EMPTY_DISCOVERY_FILTERS,
      NOW,
    );
    expect(projection.state).toBe("degraded");
    expect(projection.partial).toBe(true);
    expect(projection.rejectedRecordCount).toBe(2);
    expect(projection.noMoreResults).toBe(false);
  });

  it("sorts exact and prefix name matches before metadata matches", () => {
    const value = snapshot("ready", [
      record("relay-notes", { document: { purpose: "home network" } }),
      record("home-2", { document: { purpose: "relay home" } }),
      record("home", { document: { purpose: "relay home" } }),
    ]);
    expect(
      projectDiscovery(value, "home", EMPTY_DISCOVERY_FILTERS, NOW).results.map(
        (result) => result.title,
      ),
    ).toEqual(["home", "home-2", "relay-notes"]);
  });
});
