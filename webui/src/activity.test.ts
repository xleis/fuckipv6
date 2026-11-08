import { describe, expect, it } from "vitest";
import productFixture from "../../contracts/typescript/fixtures/product-visual-snapshot.json";
import type { ActivityEntry, NodeSnapshot } from "../../contracts/typescript/node-contract-v1";
import {
  EMPTY_ACTIVITY_FILTERS,
  activityFilterOptions,
  activityObjectTarget,
  filterActivityEntries,
  orderedActivityEntries,
} from "./activity";

function activity(overrides: Partial<ActivityEntry>): ActivityEntry {
  return {
    audit_id: "audit-a",
    actor: "node_owner",
    action: "network.create",
    resource: "node_command:create_network",
    outcome: "succeeded",
    occurred_at_unix_ms: 1_000,
    category: "user_activity",
    severity: "info",
    details: {},
    summary: "",
    ...overrides,
  };
}

describe("authoritative activity projection", () => {
  it("deduplicates stable audit IDs and applies the product ordering", () => {
    const rows = orderedActivityEntries([
      activity({ audit_id: "audit-a", occurred_at_unix_ms: 1_000, summary: "old" }),
      activity({ audit_id: "audit-b", occurred_at_unix_ms: 2_000 }),
      activity({ audit_id: "audit-a", occurred_at_unix_ms: 1_000, summary: "latest" }),
    ]);
    expect(rows.map((row) => row.audit_id)).toEqual(["audit-b", "audit-a"]);
    expect(rows[1]?.summary).toBe("latest");
  });

  it("applies all filters with inclusive time bounds", () => {
    const rows = [
      activity({
        audit_id: "audit-security",
        network_id: "net_home",
        object_kind: "member",
        object_id: "node_peer",
        actor_role: "owner",
        category: "security_event",
        severity: "warning",
        outcome: "denied",
        occurred_at_unix_ms: 2_000,
      }),
      activity({
        audit_id: "audit-failure",
        network_id: "net_office",
        object_kind: "network",
        object_id: "net_office",
        actor_role: "member",
        category: "operation_failure",
        severity: "critical",
        outcome: "failed",
        occurred_at_unix_ms: 3_000,
      }),
    ];
    expect(
      filterActivityEntries(rows, {
        ...EMPTY_ACTIVITY_FILTERS,
        networkId: "net_home",
        objectKind: "member",
        objectId: "node_peer",
        actorRole: "owner",
        category: "security_event",
        severity: "warning",
        outcome: "denied",
        fromUnixMs: 2_000,
        toUnixMs: 2_000,
      }).map((row) => row.audit_id),
    ).toEqual(["audit-security"]);
    expect(activityFilterOptions(rows)).toEqual({
      networks: ["net_home", "net_office"],
      objectKinds: ["member", "network"],
      objectIds: ["net_office", "node_peer"],
      actorRoles: ["member", "owner"],
    });
  });

  it("links only objects that still exist and remain visible", () => {
    const snapshot = productFixture.snapshot as NodeSnapshot;
    const nodeId = snapshot.identity?.node_id;
    if (!nodeId) throw new Error("fixture identity missing");
    const node = activity({ object_kind: "node", object_id: nodeId });
    expect(activityObjectTarget(snapshot, node)?.pageId).toBe("identity");

    expect(
      activityObjectTarget(
        { ...snapshot, permissions: { ...snapshot.permissions!, permissions: [] } },
        node,
      ),
    ).toBeNull();
    expect(
      activityObjectTarget(
        snapshot,
        activity({ object_kind: "node", object_id: "deleted-node" }),
      ),
    ).toBeNull();
  });
});
