import { describe, expect, it } from "vitest";
import { NAVIGATION_MODULES, navigationModuleForPage } from "./navigation";

describe("shared navigation contract", () => {
  it("exposes the five product modules in their stable order", () => {
    expect(NAVIGATION_MODULES.map((module) => module.id)).toEqual([
      "overview",
      "nodes",
      "networks",
      "activity",
      "settings",
    ]);
  });

  it("maps every manifest page to exactly one module", () => {
    const pages = NAVIGATION_MODULES.flatMap((module) => module.pageIds);
    expect(pages).toEqual([
      "overview",
      "peers",
      "node_runtime",
      "routing",
      "identity",
      "networks",
      "topology",
      "exit_node",
      "relay",
      "traffic",
      "logs",
      "settings",
    ]);
    expect(new Set(pages).size).toBe(12);
    expect(navigationModuleForPage("identity").id).toBe("nodes");
    expect(navigationModuleForPage("missing").id).toBe("overview");
  });
});
