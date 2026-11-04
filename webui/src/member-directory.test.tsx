import { cleanup, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type {
  NetworkMember,
  NetworkMemberDirectory,
} from "../../contracts/typescript/node-contract-v1";
import { MemberDirectoryCard } from "./member-directory";
import { installTestI18nRuntime } from "./test/runtime";

const generatedAtUnixMs = 1_700_000_100_000;

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
    joined_at_unix_ms: generatedAtUnixMs - 60_000,
    ...overrides,
  };
}

function directory(members: NetworkMember[]): NetworkMemberDirectory {
  return {
    network_id: "net_home",
    generated_at_unix_ms: generatedAtUnixMs,
    members,
  };
}

const owner = member({
  node_id: "node-000",
  display_name: "Alpha owner",
  role: "owner",
  groups: ["owner"],
  capabilities: ["coordinator", "file"],
  status: "active",
  presence: {
    state: "online",
    source: "live_session",
    observed_at_unix_ms: generatedAtUnixMs - 5_000,
  },
});

const unknownMember = member({
  node_id: "node-001",
  display_name: "Bravo unknown",
  capabilities: null,
  presence: {
    state: "unknown",
    source: "not_observed",
    observed_at_unix_ms: null,
  },
});

const roster = Array.from({ length: 36 }, (_, index) =>
  member({
    node_id: `node-${String(index + 10).padStart(3, "0")}`,
    display_name: `Roster ${index + 1}`,
    status: index % 3 === 0 ? "degraded" : "approved",
  }),
);

beforeEach(() => {
  installTestI18nRuntime();
  vi.spyOn(Element.prototype, "getBoundingClientRect").mockReturnValue({
    x: 0,
    y: 0,
    top: 0,
    left: 0,
    right: 680,
    bottom: 360,
    width: 680,
    height: 360,
    toJSON: () => ({}),
  });
  Object.defineProperty(Element.prototype, "clientWidth", {
    configurable: true,
    value: 680,
  });
  Object.defineProperty(Element.prototype, "clientHeight", {
    configurable: true,
    value: 360,
  });
  Object.defineProperty(navigator, "clipboard", {
    configurable: true,
    value: { writeText: vi.fn(async () => {}) },
  });
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

describe("MemberDirectoryCard", () => {
  it("virtualizes the roster and keeps a selected NodeId across refresh", async () => {
    const user = userEvent.setup();
    const view = render(
      <MemberDirectoryCard
        directory={directory([owner, unknownMember, ...roster])}
        locale="en"
      />,
    );

    const listbox = screen.getByRole("listbox", {
      name: "translated:member_directory.member",
    });
    expect(listbox).toBeInTheDocument();
    const option = await screen.findByRole("option", { name: /Alpha owner/ });
    expect(within(listbox).getAllByRole("option").length).toBeLessThan(39);
    await user.click(option);
    expect(option).toHaveAttribute("aria-selected", "true");
    expect(screen.getByTestId("member-detail")).toHaveTextContent(
      "translated:member_directory.display_name",
    );

    view.rerender(
      <MemberDirectoryCard
        directory={{
          ...directory([owner, unknownMember, ...roster]),
          generated_at_unix_ms: generatedAtUnixMs + 10_000,
        }}
        locale="en"
      />,
    );
    expect(
      await screen.findByRole("option", { name: /Alpha owner/ }),
    ).toHaveAttribute("aria-selected", "true");
  });

  it("filters by role, exposes source and freshness, and never calls unknown offline", async () => {
    const user = userEvent.setup();
    render(
      <MemberDirectoryCard
        directory={directory([owner, unknownMember])}
        locale="en"
      />,
    );

    await user.selectOptions(
      screen.getByLabelText("translated:member_directory.filter.role"),
      "member",
    );
    const memberOption = await screen.findByRole("option", {
      name: /Bravo unknown/,
    });
    expect(memberOption).toHaveAccessibleName(
      /translated:member_directory.presence.unknown/,
    );
    expect(memberOption).not.toHaveAccessibleName(
      /translated:member_directory.presence.offline/,
    );
    await user.click(memberOption);
    const detail = screen.getByTestId("member-detail");
    expect(detail).toHaveTextContent("translated:member_directory.source.not_observed");
    expect(detail).toHaveTextContent("translated:member_directory.joined_at");
    expect(detail).toHaveTextContent(
      "translated:member_directory.capabilities_unknown",
    );
  });

  it("exports exactly the current filtered view as sanitized text", async () => {
    const user = userEvent.setup();
    const writeText = vi.spyOn(navigator.clipboard, "writeText");
    render(
      <MemberDirectoryCard
        directory={directory([owner, unknownMember])}
        locale="en"
      />,
    );

    await user.selectOptions(
      screen.getByLabelText("translated:member_directory.filter.role"),
      "owner",
    );
    await user.click(
      screen.getByRole("button", {
        name: "translated:member_directory.export",
      }),
    );

    await waitFor(() => expect(writeText).toHaveBeenCalledTimes(1));
    const exported = writeText.mock.calls[0][0];
    expect(exported).toContain("Alpha owner");
    expect(exported).toContain("node-000");
    expect(exported).not.toContain("Bravo unknown");
    expect(exported).not.toContain("node-001");
    expect(screen.getByRole("status")).toHaveTextContent(
      "translated:member_directory.export_succeeded",
    );
  });
});
