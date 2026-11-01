import { cleanup, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import manifestFixture from "../../../contracts/typescript/fixtures/node-manifest.json";
import productFixture from "../../../contracts/typescript/fixtures/product-visual-snapshot.json";
import type {
  NodeCommandOutcome,
  NodeSnapshot,
  NodeUiAction,
  NodeUiManifest,
} from "../../../contracts/typescript/node-contract-v1";
import type { PageStateKind } from "../state-matrix";
import { installTestI18nRuntime } from "../test/runtime";
import { ActionPanel } from "./action-panel";

const manifest = manifestFixture as NodeUiManifest;
const snapshot = productFixture.snapshot as NodeSnapshot;
const restart = manifest.pages
  .flatMap((page) => page.actions)
  .find((action) => action.id === "restart_node") as NodeUiAction;

function renderPanel(
  interactionBlocked: PageStateKind | null,
  onSubmit = vi.fn(async (): Promise<NodeCommandOutcome> => ({
    status: "completed",
    data: {},
  }) as NodeCommandOutcome),
) {
  return render(
    <ActionPanel
      actions={[restart]}
      snapshot={snapshot}
      selectedNetwork=""
      locale="zh-CN"
      pendingAction={null}
      interactionBlocked={interactionBlocked}
      requestedActionId={null}
      onRequestedActionOpened={() => {}}
      onSubmit={onSubmit}
    />,
  );
}

beforeEach(() => {
  installTestI18nRuntime();
});

afterEach(() => {
  cleanup();
});

describe("action panel interaction gate", () => {
  it("disables an open confirmation and explains a newly blocked page", async () => {
    const user = userEvent.setup();
    const view = renderPanel(null);

    await user.click(
      screen.getByRole("button", { name: "translated:action.restart_node" }),
    );
    await user.click(
      screen.getByRole("button", { name: "translated:common.execute" }),
    );

    const dialog = await screen.findByRole("dialog");
    const confirm = within(dialog).getByRole("button", {
      name: "translated:common.execute",
    });
    expect(confirm).toBeEnabled();

    view.rerender(
      <ActionPanel
        actions={[restart]}
        snapshot={snapshot}
        selectedNetwork=""
        locale="zh-CN"
        pendingAction={null}
        interactionBlocked="stale"
        requestedActionId={null}
        onRequestedActionOpened={() => {}}
        onSubmit={vi.fn(async (): Promise<NodeCommandOutcome> => ({
          status: "completed",
          data: {},
        }) as NodeCommandOutcome)}
      />,
    );

    expect(confirm).toBeDisabled();
    expect(within(dialog).getByRole("status")).toHaveTextContent(
      "translated:state.stale.title",
    );
  });
});
