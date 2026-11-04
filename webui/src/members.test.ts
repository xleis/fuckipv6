import { beforeEach, describe, expect, it } from "vitest";
import type {
  NetworkMember,
  NodeNetworkMembershipSnapshot,
} from "../../contracts/typescript/node-contract-v1";
import {
  EMPTY_MEMBER_FILTERS,
  exportMemberDirectory,
  filterMembers,
  memberFreshnessText,
  memberPresenceText,
  resolveNetworkMembershipId,
  retainMemberSelection,
  sortMembers,
} from "./members";
import { installTestI18nRuntime } from "./test/runtime";

const now = 1_700_000_100_000;

function member(overrides: Partial<NetworkMember>): NetworkMember {
  return {
    node_id: "node-member",
    display_name: "Member",
    identity_fingerprint: "fingerprint",
    role: "member",
    groups: ["member"],
    capabilities: ["file"],
    status: "approved",
    presence: {
      state: "unknown",
      source: "not_observed",
      observed_at_unix_ms: null,
    },
    joined_at_unix_ms: now - 10_000,
    ...overrides,
  };
}

const members: NetworkMember[] = [
  member({
    node_id: "node-z",
    display_name: "Zulu",
    role: "administrator",
    groups: ["administrator"],
    capabilities: ["relay", "file"],
    status: "degraded",
    presence: {
      state: "offline",
      source: "disconnect_observed",
      observed_at_unix_ms: now - 30_000,
    },
  }),
  member({
    node_id: "node-a",
    display_name: "Alpha",
    role: "owner",
    groups: ["owner"],
    capabilities: null,
    status: "active",
    presence: {
      state: "online",
      source: "live_session",
      observed_at_unix_ms: now - 20_000,
    },
  }),
  member({
    node_id: "node-m",
    display_name: "Mike",
    status: "pending",
  }),
];

beforeEach(() => {
  installTestI18nRuntime();
});

describe("member directory projection", () => {
  it("never presents an unknown presence as offline", () => {
    const unknown = members.find((entry) => entry.node_id === "node-m")!;
    expect(memberPresenceText("en", unknown)).toBe(
      "translated:member_directory.presence.unknown",
    );
    expect(memberPresenceText("en", unknown)).not.toBe(
      "translated:member_directory.presence.offline",
    );
  });

  it("filters role, group, capability, and health then sorts by node id", () => {
    expect(
      filterMembers(members, {
        ...EMPTY_MEMBER_FILTERS,
        role: "owner",
      }).map((entry) => entry.node_id),
    ).toEqual(["node-a"]);
    expect(
      filterMembers(members, {
        ...EMPTY_MEMBER_FILTERS,
        group: "administrator",
        capability: "relay",
        health: "presence:offline",
      }).map((entry) => entry.node_id),
    ).toEqual(["node-z"]);
    expect(
      filterMembers(members, {
        ...EMPTY_MEMBER_FILTERS,
        capability: "unknown",
      }).map((entry) => entry.node_id),
    ).toEqual(["node-a"]);
    expect(sortMembers(members).map((entry) => entry.node_id)).toEqual([
      "node-a",
      "node-m",
      "node-z",
    ]);
  });

  it("exports exactly the filtered rows with stable ordering", () => {
    const filtered = filterMembers(members, {
      ...EMPTY_MEMBER_FILTERS,
      health: "presence:offline",
    });
    const exported = exportMemberDirectory(filtered, "en", now);
    const lines = exported.split("\n");
    expect(lines).toHaveLength(2);
    expect(lines[1].split("\t")[1]).toBe("node-z");
    expect(exported).toContain("translated:member_directory.presence.offline");
    expect(exported).not.toContain("node-a");
  });

  it("keeps a valid selection across refresh and clears a vanished member", () => {
    expect(retainMemberSelection("node-m", members)).toBe("node-m");
    expect(
      retainMemberSelection(
        "node-m",
        members.filter((entry) => entry.node_id !== "node-m"),
      ),
    ).toBeNull();
  });

  it("exports only sanitized member DTO fields", () => {
    const withUnfilteredServerFields = {
      ...members[0],
      approval_reason: "SECRET-APPROVAL-REASON",
      endpoint: "127.0.0.1:6666",
      invitation_code: "SECRET-INVITATION-CODE",
    } as NetworkMember;
    const exported = exportMemberDirectory([withUnfilteredServerFields], "en", now);
    expect(exported).not.toContain("SECRET-APPROVAL-REASON");
    expect(exported).not.toContain("127.0.0.1:6666");
    expect(exported).not.toContain("SECRET-INVITATION-CODE");
  });

  it("formats observation freshness from the directory generation time", () => {
    expect(memberFreshnessText("en", now - 20_000, now)).toBe(
      "translated:member_directory.freshness.just_now",
    );
    expect(memberFreshnessText("en", now - 2 * 60_000, now)).toBe(
      "translated:member_directory.freshness.minutes",
    );
    expect(memberFreshnessText("en", now - 2 * 60 * 60_000, now)).toBe(
      "translated:member_directory.freshness.hours",
    );
    expect(memberFreshnessText("en", null, now)).toBe("");
  });

  it("resolves the real network id from the joined membership", () => {
    const memberships = [
      {
        network_id: "net-real-id",
        network_name: "home",
      },
      {
        network_id: "net-other-id",
        network_name: "office",
      },
    ] as NodeNetworkMembershipSnapshot[];
    expect(resolveNetworkMembershipId(memberships, "home")).toBe("net-real-id");
    expect(resolveNetworkMembershipId(memberships, "missing")).toBeNull();
    expect(resolveNetworkMembershipId(undefined, "home")).toBeNull();
  });
});
