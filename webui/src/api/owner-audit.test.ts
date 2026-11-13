import { afterEach, describe, expect, it, vi } from "vitest";
import {
  HttpError,
  ReauthenticationRequiredError,
} from "./client";
import {
  ownerAuditExportFilename,
  requestOwnerAuditExport,
} from "./owner-audit";

function exportBundle(recordCount = 1) {
  return {
    manifest: {
      schema_version: 1,
      generated_at_unix_ms: 1_700_000_100_000,
      filter: {
        network_id: "net_owner_relay",
        resource_prefix: "node_command:",
      },
      verification: {
        first_sequence: recordCount > 0 ? 1 : null,
        last_sequence: recordCount > 0 ? recordCount : null,
        last_hash: "a".repeat(64),
        record_count: recordCount,
      },
      checkpoint: {
        schema_version: 1,
        sequence: recordCount,
        entry_hash: "a".repeat(64),
        record_count: recordCount,
      },
    },
    records:
      recordCount === 0
        ? []
        : [
            {
              schema_version: 1,
              sequence: 1,
              previous_hash: "0".repeat(64),
              entry: {
                audit_id: "audit-relay-enable",
                operation_id: "op-relay-enable",
                actor: "node-owner",
                network_id: "net_owner_relay",
                action: "relay.enable",
                resource: "node_command:enable_relay",
                outcome: "succeeded",
                occurred_at_unix_ms: 1_700_000_000_000,
                summary: "Relay enabled",
                details: { enabled: "true" },
                sensitive_keys: [],
              },
              entry_hash: "b".repeat(64),
            },
          ],
  };
}

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("owner durable audit export", () => {
  it("requests a confirmed network-scoped node command export", async () => {
    const fetchMock = vi.fn<typeof fetch>(
      async () =>
        new Response(JSON.stringify(exportBundle()), {
          status: 200,
          headers: {
            "Content-Type": "application/json",
            "Content-Disposition":
              'attachment; filename="owner-security-audit-export.json"',
          },
        }),
    );
    vi.stubGlobal("fetch", fetchMock);

    const result = await requestOwnerAuditExport({
      network_id: "net_owner_relay",
      from_unix_ms: 1_700_000_000_000,
      to_unix_ms: 1_700_000_360_000,
      resource_prefix: "node_command:",
    });

    expect(fetchMock).toHaveBeenCalledOnce();
    const [path, init] = fetchMock.mock.calls[0];
    expect(path).toBe("/api/v1/owner/security/audit/export");
    if (!init) throw new Error("audit export request init is missing");
    expect(init).toEqual(
      expect.objectContaining({
        method: "POST",
        credentials: "same-origin",
      }),
    );
    expect(JSON.parse(String(init.body))).toEqual({
      confirmed: true,
      filter: {
        network_id: "net_owner_relay",
        from_unix_ms: 1_700_000_000_000,
        to_unix_ms: 1_700_000_360_000,
        resource_prefix: "node_command:",
      },
    });
    expect(String(init.body)).not.toContain("PAYLOAD_SENTINEL");
    expect(result.filename).toBe("owner-security-audit-export.json");
    expect(result.bundle.records).toHaveLength(1);
    expect(result.bundle.records[0].entry.action).toBe("relay.enable");
  });

  it("keeps empty exports valid", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async () =>
        new Response(JSON.stringify(exportBundle(0)), {
          status: 200,
          headers: { "Content-Type": "application/json" },
        }),
      ),
    );

    const result = await requestOwnerAuditExport({
      network_id: "net_owner_relay",
      resource_prefix: "node_command:",
    });

    expect(result.bundle.records).toEqual([]);
    expect(result.filename).toBe("owner-security-audit-export.json");
  });

  it("maps recent-authentication failures to the shared reauthentication flow", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async () =>
        new Response(
          JSON.stringify({
            error: "recent owner reauthentication required",
            code: "recent_authentication_required",
            problem: null,
          }),
          { status: 403, headers: { "Content-Type": "application/json" } },
        ),
      ),
    );

    await expect(
      requestOwnerAuditExport({
        network_id: "net_owner_relay",
        resource_prefix: "node_command:",
      }),
    ).rejects.toBeInstanceOf(ReauthenticationRequiredError);
  });

  it("surfaces confirmation and permission failures as HTTP errors", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async () =>
        new Response(
          JSON.stringify({
            error: "audit export confirmation required",
            code: "audit_export_confirmation_required",
            problem: null,
          }),
          { status: 403, headers: { "Content-Type": "application/json" } },
        ),
      ),
    );

    await expect(
      requestOwnerAuditExport({
        network_id: "net_owner_relay",
        resource_prefix: "node_command:",
      }),
    ).rejects.toBeInstanceOf(HttpError);
  });

  it("extracts only the basename from Content-Disposition", () => {
    expect(
      ownerAuditExportFilename(
        'attachment; filename="../../owner-security-audit-export.json"',
      ),
    ).toBe("owner-security-audit-export.json");
    expect(
      ownerAuditExportFilename(
        "attachment; filename*=UTF-8''audit%20with%20spaces.json",
      ),
    ).toBe("audit with spaces.json");
  });
});
