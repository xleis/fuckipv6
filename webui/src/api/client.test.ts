import { afterEach, describe, expect, it, vi } from "vitest";
import { newIdempotencyKey, nextRequestId, waitForOperation } from "./client";

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("request identities", () => {
  it("allocates strictly increasing positive request ids", () => {
    const first = nextRequestId();
    const second = nextRequestId();
    expect(first).toBeGreaterThan(0);
    expect(second).toBe(first + 1);
  });

  it("allocates unique UUID-shaped idempotency keys", () => {
    const first = newIdempotencyKey();
    const second = newIdempotencyKey();
    expect(first).toMatch(/^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/);
    expect(second).toMatch(/^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/);
    expect(second).not.toBe(first);
  });

  it("reports every polled operation snapshot until it reaches a terminal state", async () => {
    let poll = 0;
    vi.stubGlobal(
      "fetch",
      vi.fn(async () => {
        poll += 1;
        return new Response(
          JSON.stringify({
            schema_version: 1,
            request_id: poll,
            server_time: `2026-11-02T00:00:0${poll}Z`,
            operation: {
              operation_id: "op-polled",
              state: poll === 1 ? "running" : "succeeded",
              stage: poll === 1 ? "stopping" : "stopped",
            },
          }),
          { status: 200, headers: { "Content-Type": "application/json" } },
        );
      }),
    );
    const observed: string[] = [];

    const terminal = await waitForOperation("op-polled", {
      intervalMs: 1,
      onOperation: (operation) => observed.push(operation.state),
    });

    expect(poll).toBe(2);
    expect(observed).toEqual(["running", "succeeded"]);
    expect(terminal.state).toBe("succeeded");
  });
});
