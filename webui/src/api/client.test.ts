import { describe, expect, it } from "vitest";
import { newIdempotencyKey, nextRequestId } from "./client";

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
});
