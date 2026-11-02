import { afterEach, describe, expect, it, vi } from "vitest";
import type { NodeCommandHttpRequest } from "../../../contracts/typescript/node-contract-v1";
import {
  AuthenticationError,
  ReauthenticationRequiredError,
  newIdempotencyKey,
  nextRequestId,
  sendCommand,
  waitForOperation,
} from "./client";

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

  it("recognizes a recent-authentication command rejection", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async () =>
        new Response(
          JSON.stringify({
            schema_version: 1,
            request_id: 31,
            server_time: "2026-11-02T00:00:00Z",
            result: {
              request_id: 31,
              outcome: {
                status: "rejected",
                data: {
                  code: "authentication_required",
                  params: {
                    authorization_code:
                      "permission.recent_authentication_required",
                  },
                },
              },
            },
            operation: null,
          }),
          { status: 401, headers: { "Content-Type": "application/json" } },
        ),
      ),
    );
    const envelope: NodeCommandHttpRequest = {
      schema_version: 1,
      idempotency_key: "reauth-test",
      request: {
        request_id: 31,
        command: { command: "shutdown" },
      },
    };

    await expect(sendCommand(envelope)).rejects.toBeInstanceOf(
      ReauthenticationRequiredError,
    );
  });

  it("keeps ordinary authentication failures distinct from reauthentication", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async () =>
        new Response(
          JSON.stringify({
            code: "authentication_required",
            message: "owner session expired",
            request_id: 32,
          }),
          { status: 401, headers: { "Content-Type": "application/json" } },
        ),
      ),
    );
    const envelope: NodeCommandHttpRequest = {
      schema_version: 1,
      idempotency_key: "reauth-test",
      request: {
        request_id: 32,
        command: { command: "shutdown" },
      },
    };

    await expect(sendCommand(envelope)).rejects.toBeInstanceOf(
      AuthenticationError,
    );
  });

  it("allocates contract-valid unique idempotency keys", () => {
    const first = newIdempotencyKey();
    const second = newIdempotencyKey();
    expect(first).toMatch(/^idem_[A-Za-z0-9_-]{43}$/);
    expect(second).toMatch(/^idem_[A-Za-z0-9_-]{43}$/);
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
