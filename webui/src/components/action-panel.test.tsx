import { cleanup, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import manifestFixture from "../../../contracts/typescript/fixtures/node-manifest.json";
import productFixture from "../../../contracts/typescript/fixtures/product-visual-snapshot.json";
import type {
  NodeCommandOutcome,
  NodeOperationSnapshot,
  NodeSnapshot,
  NodeUiAction,
  NodeUiManifest,
} from "../../../contracts/typescript/node-contract-v1";
import type { PageStateKind } from "../state-matrix";
import { installTestI18nRuntime } from "../test/runtime";
import { ActionPanel, type ActionSubmission } from "./action-panel";

const manifest = manifestFixture as NodeUiManifest;
const snapshot = productFixture.snapshot as NodeSnapshot;
const restart = manifest.pages
  .flatMap((page) => page.actions)
  .find((action) => action.id === "restart_node") as NodeUiAction;

function renderPanel(
  interactionBlocked: PageStateKind | null,
  onSubmit = vi.fn(async (): Promise<ActionSubmission> => ({
    outcome: { status: "completed", data: {} } as NodeCommandOutcome,
  })),
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
      operation={null}
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
        operation={null}
        onRequestedActionOpened={() => {}}
        onSubmit={vi.fn(async (): Promise<ActionSubmission> => ({
          outcome: { status: "completed", data: {} } as NodeCommandOutcome,
        }))}
      />,
    );

    expect(confirm).toBeDisabled();
    expect(within(dialog).getByRole("status")).toHaveTextContent(
      "translated:state.stale.title",
    );
  });
});

describe("action panel operation status", () => {
  it("shows structured failure details and copies the diagnostic id", async () => {
    const user = userEvent.setup();
    const writeText = vi
      .spyOn(navigator.clipboard, "writeText")
      .mockResolvedValue(undefined);
    const operation = {
      operation_id: "op-diagnostic-42",
      state: "failed",
      stage: "cleanup",
      attempt: 2,
      max_attempts: 3,
      cancellable: false,
      cancel_blocked_reason: "operation.cancel.not_supported",
      error: {
        code: "tun.route_conflict",
        message_key: "error.tun_route_conflict",
        retryable: true,
        recovery: {
          kind: "retry",
          hint_key: "recovery.retry_after_route_check",
        },
      },
    } as NodeOperationSnapshot;

    render(
      <ActionPanel
        actions={[restart]}
        snapshot={snapshot}
        selectedNetwork=""
        locale="zh-CN"
        pendingAction={null}
        interactionBlocked={null}
        requestedActionId={null}
        operation={operation}
        onRequestedActionOpened={() => {}}
        onSubmit={vi.fn(async (): Promise<ActionSubmission> => ({ operation }))}
      />,
    );

    const status = screen.getByTestId("operation-status");
    expect(status).toHaveTextContent("translated:operation.state.failed");
    expect(status).toHaveTextContent("cleanup");
    expect(status).toHaveTextContent("translated:operation.attempt");
    expect(status).toHaveTextContent(
      "translated:operation.cancellable_no",
    );
    expect(status).toHaveTextContent("tun.route_conflict");
    expect(status).toHaveTextContent(
      "translated:error.tun_route_conflict · translated:recovery.retry_after_route_check",
    );
    expect(status).toHaveTextContent("translated:operation.retryable");
    expect(status).toHaveTextContent("op-diagnostic-42");

    await user.click(
      within(status).getByRole("button", {
        name: "translated:operation.copy_diagnostic_id",
      }),
    );
    expect(writeText).toHaveBeenCalledWith("op-diagnostic-42");
    expect(
      within(status).getByRole("status"),
    ).toHaveTextContent("translated:operation.diagnostic_copied");
  });
});
