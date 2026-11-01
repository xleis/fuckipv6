import { describe, expect, it } from "vitest";
import productFixture from "../../contracts/typescript/fixtures/product-visual-snapshot.json";
import manifestFixture from "../../contracts/typescript/fixtures/node-manifest.json";
import type {
  NodeSnapshot,
  NodeUiManifest,
} from "../../contracts/typescript/node-contract-v1";
import { searchNodeSnapshot } from "./search";
import { installTestI18nRuntime } from "./test/runtime";

const manifest = manifestFixture as NodeUiManifest;
const snapshot = productFixture.snapshot as NodeSnapshot;

describe("global object search", () => {
  it("finds manifest pages, identity, networks, and peers", () => {
    installTestI18nRuntime();

    const identity = searchNodeSnapshot(manifest, snapshot, "identity", "en");
    expect(identity.some((result) => result.kind === "page" && result.pageId === "identity")).toBe(true);
    expect(identity.some((result) => result.kind === "identity")).toBe(true);

    const home = searchNodeSnapshot(manifest, snapshot, "home", "en");
    expect(home.some((result) => result.kind === "network" && result.networkName === "home")).toBe(true);
    expect(home.some((result) => result.kind === "peer" && result.networkName === "home")).toBe(true);
  });

  it("finds advanced settings as navigation results", () => {
    installTestI18nRuntime();

    const results = searchNodeSnapshot(manifest, snapshot, "doh", "en");
    expect(
      results.some(
        (result) =>
          result.kind === "setting" &&
          result.id === "setting:doh-upstreams" &&
          result.pageId === "settings",
      ),
    ).toBe(true);
  });

  it("does not search for blank queries and returns no fabricated matches", () => {
    installTestI18nRuntime();

    expect(searchNodeSnapshot(manifest, snapshot, "", "en")).toEqual([]);
    expect(searchNodeSnapshot(manifest, snapshot, "   ", "en")).toEqual([]);
    expect(searchNodeSnapshot(manifest, snapshot, "no-such-object", "en")).toEqual([]);
  });

  it("uses deterministic ranking and caps the result set at twelve", () => {
    installTestI18nRuntime();

    const first = searchNodeSnapshot(manifest, snapshot, "e", "en");
    const second = searchNodeSnapshot(manifest, snapshot, "e", "en");
    expect(first).toHaveLength(12);
    expect(first).toEqual(second);
  });
});
