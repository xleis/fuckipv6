import { cleanup, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import manifestFixture from "../../../contracts/typescript/fixtures/node-manifest.json";
import productFixture from "../../../contracts/typescript/fixtures/product-visual-snapshot.json";
import type {
  NodeSnapshot,
  NodeUiManifest,
  NodeUiPage,
} from "../../../contracts/typescript/node-contract-v1";
import { PageContent } from "./page-content";
import { installTestI18nRuntime } from "../test/runtime";

const manifest = manifestFixture as NodeUiManifest;
const networksPage = manifest.pages.find((page) => page.id === "networks") as NodeUiPage;
const baseSnapshot = productFixture.snapshot as NodeSnapshot;
const oldTermsHash = "hash_AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
const currentTermsHash = "hash_BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB";

function policySnapshot(exitRoleEnabled: boolean): NodeSnapshot {
  return {
    ...baseSnapshot,
    memberships: [
      {
        network_name: "home",
        network_id: "net_policy_home",
        node_id: "node-visual-01",
        state: "active",
        role: "member",
        local_policy_revision: 2,
        network_policy_revision: 4,
        privacy_policy_revision: 3,
        member_limit: 16,
        exit_contribution: "required",
        exit_contribution_scope: "all_members",
        terms_hash: oldTermsHash,
        current_terms: {
          network_policy_revision: 5,
          privacy_policy_revision: 3,
          exit_contribution: "required",
          exit_contribution_scope: "all_members",
          member_limit: 16,
        },
        current_terms_hash: currentTermsHash,
        policy_confirmation_required: true,
        acknowledged_at_unix_ms: null,
        created_at_unix_ms: 1_700_000_000_000,
        updated_at_unix_ms: 1_700_000_100_000,
        suspension_reason: null,
      },
    ],
    networks: (baseSnapshot.networks ?? []).map((network) =>
      network.name === "home"
        ? {
            ...network,
            roles: exitRoleEnabled
              ? (["member", "exit_node"] as typeof network.roles)
              : (["member"] as typeof network.roles),
          }
        : network,
    ),
  };
}

function renderNetworks(snapshot: NodeSnapshot, onRequestAction = vi.fn()) {
  render(
    <PageContent
      page={networksPage}
      snapshot={snapshot}
      locale="zh-CN"
      selectedNetwork="home"
      recentDiagnostics={null}
      onSelectNetwork={() => {}}
      onNavigate={() => {}}
      onRequestAction={onRequestAction}
      onRunAction={async () => {}}
    />,
  );
  return onRequestAction;
}

beforeEach(() => {
  installTestI18nRuntime();
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

describe("membership policy reconfirmation", () => {
  it("requires separate Exit consent and reports capability separately from enabled state", async () => {
    const user = userEvent.setup();
    const now = vi.spyOn(Date, "now").mockReturnValue(1_700_000_200_123);
    const onRequestAction = renderNetworks(policySnapshot(false));
    const card = screen.getByTestId("membership-policy-net_policy_home");

    expect(card).toHaveTextContent("translated:membership.policy.confirmation_required");
    expect(card).toHaveTextContent(
      "translated:join.exit.disclosure.purpose translated:join.exit.disclosure.bandwidth translated:join.exit.disclosure.logging translated:join.exit.disclosure.disable",
    );
    expect(card).toHaveTextContent("translated:join.exit.local_supported");
    expect(card).toHaveTextContent("translated:join.exit.local_not_enabled");
    expect(
      within(card).getByText("translated:join.network_policy_revision")
        .nextElementSibling,
    ).toHaveTextContent("5");
    expect(
      within(card).getByText("translated:join.privacy_policy_revision")
        .nextElementSibling,
    ).toHaveTextContent("3");
    expect(card).toHaveTextContent(currentTermsHash);

    const confirmation = within(card).getByRole("button", {
      name: "translated:membership.policy.confirm",
    });
    expect(confirmation).toBeDisabled();

    await user.click(
      within(card).getByRole("checkbox", {
        name: "translated:membership.policy.confirm_exit",
      }),
    );
    expect(confirmation).toBeEnabled();
    await user.click(confirmation);

    expect(onRequestAction).toHaveBeenCalledTimes(1);
    expect(onRequestAction).toHaveBeenCalledWith("acknowledge_membership_policy", {
      network_id: "net_policy_home",
      expected_terms_hash: currentTermsHash,
      acknowledged_at_unix_ms: "1700000200123",
      exit_contribution_acknowledged: "true",
    });
    expect(now).toHaveBeenCalled();
  });

  it("shows the actual enabled role only when the local Exit role is active", () => {
    renderNetworks(policySnapshot(true));
    const card = screen.getByTestId("membership-policy-net_policy_home");
    expect(card).toHaveTextContent("translated:join.exit.local_supported");
    expect(card).toHaveTextContent("translated:join.exit.local_enabled");
  });
});
