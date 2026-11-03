import { cleanup, render, screen, waitFor, within } from "@testing-library/react";
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
const rotate = manifest.pages
  .flatMap((page) => page.actions)
  .find((action) => action.id === "rotate_node_identity") as NodeUiAction;
const currentFingerprint = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
const nextFingerprint = "BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB";
const editedFingerprint = "CCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCC";
const identitySnapshot: NodeSnapshot = {
  ...snapshot,
  identity: {
    node_id: "node-current",
    display_name: "Current node",
    public_key_fingerprint: currentFingerprint,
    created_at_unix_ms: 1_700_000_000_000,
  },
};

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
      onCancel={vi.fn(async () => {})}
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
    expect(within(dialog).getByText("translated:confirm.reauthentication_required"))
      .toBeInTheDocument();

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
        onCancel={vi.fn(async () => {})}
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

describe("action panel field synchronization", () => {
  it("adopts a refreshed fingerprint until the owner edits the field", async () => {
    const user = userEvent.setup();
    const view = render(
      <ActionPanel
        actions={[rotate]}
        snapshot={identitySnapshot}
        selectedNetwork=""
        locale="zh-CN"
        pendingAction={null}
        interactionBlocked={null}
        requestedActionId={null}
        operation={null}
        onRequestedActionOpened={() => {}}
        onCancel={vi.fn(async () => {})}
        onSubmit={vi.fn(async (): Promise<ActionSubmission> => ({
          outcome: { status: "completed", data: {} } as NodeCommandOutcome,
        }))}
      />,
    );

    await user.click(
      screen.getByRole("button", {
        name: "translated:action.rotate_node_identity",
      }),
    );
    const input = screen.getByRole("textbox", {
      name: "translated:field.expected_fingerprint",
    });
    expect(input).toHaveValue(currentFingerprint);

    view.rerender(
      <ActionPanel
        actions={[rotate]}
        snapshot={{
          ...identitySnapshot,
          identity: {
            ...identitySnapshot.identity!,
            public_key_fingerprint: nextFingerprint,
          },
        }}
        selectedNetwork=""
        locale="zh-CN"
        pendingAction={null}
        interactionBlocked={null}
        requestedActionId={null}
        operation={null}
        onRequestedActionOpened={() => {}}
        onCancel={vi.fn(async () => {})}
        onSubmit={vi.fn(async (): Promise<ActionSubmission> => ({
          outcome: { status: "completed", data: {} } as NodeCommandOutcome,
        }))}
      />,
    );
    expect(input).toHaveValue(nextFingerprint);

    await user.clear(input);
    await user.type(input, editedFingerprint);
    view.rerender(
      <ActionPanel
        actions={[rotate]}
        snapshot={identitySnapshot}
        selectedNetwork=""
        locale="zh-CN"
        pendingAction={null}
        interactionBlocked={null}
        requestedActionId={null}
        operation={null}
        onRequestedActionOpened={() => {}}
        onCancel={vi.fn(async () => {})}
        onSubmit={vi.fn(async (): Promise<ActionSubmission> => ({
          outcome: { status: "completed", data: {} } as NodeCommandOutcome,
        }))}
      />,
    );
    expect(input).toHaveValue(editedFingerprint);
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
        onCancel={vi.fn(async () => {})}
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

  it("only offers cancellation for a non-terminal cancellable operation", async () => {
    const user = userEvent.setup();
    const onCancel = vi.fn(async () => {});
    const operation = {
      operation_id: "op-cancellable",
      state: "running",
      stage: "probing",
      attempt: 1,
      max_attempts: 1,
      cancellable: true,
    } as NodeOperationSnapshot;

    const view = render(
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
        onCancel={onCancel}
        onSubmit={vi.fn(async (): Promise<ActionSubmission> => ({ operation }))}
      />,
    );

    const status = screen.getByTestId("operation-status");
    await user.click(
      within(status).getByRole("button", {
        name: "translated:operation.cancel",
      }),
    );
    expect(onCancel).toHaveBeenCalledWith("op-cancellable");

    view.rerender(
      <ActionPanel
        actions={[restart]}
        snapshot={snapshot}
        selectedNetwork=""
        locale="zh-CN"
        pendingAction={null}
        interactionBlocked={null}
        requestedActionId={null}
        operation={{ ...operation, state: "cancelled" }}
        onRequestedActionOpened={() => {}}
        onCancel={onCancel}
        onSubmit={vi.fn(async (): Promise<ActionSubmission> => ({ operation }))}
      />,
    );
    expect(
      screen.queryByRole("button", { name: "translated:operation.cancel" }),
    ).not.toBeInTheDocument();
  });

  it("surfaces a localized cancellation failure without replacing the operation error", async () => {
    if (!window.FuckIPV6I18n) throw new Error("i18n runtime is missing");
    window.FuckIPV6I18n.tr = (_locale, key, params) =>
      `translated:${key}:${JSON.stringify(params ?? {})}`;
    const user = userEvent.setup();
    const operation = {
      operation_id: "op-cancel-failed",
      state: "running",
      stage: "probing",
      attempt: 1,
      max_attempts: 1,
      cancellable: true,
      error: {
        code: "node.diagnostic_progress",
        message_key: "diagnostic.state.failed",
        retryable: false,
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
        onCancel={vi.fn(async () => {
          throw new Error("cancel conflict");
        })}
        onSubmit={vi.fn(async (): Promise<ActionSubmission> => ({ operation }))}
      />,
    );

    await user.click(
      within(screen.getByTestId("operation-status")).getByRole("button", {
        name: "translated:operation.cancel",
      }),
    );

    const status = screen.getByTestId("operation-status");
    await waitFor(() => {
      expect(status.querySelector(".inline-notice")).toHaveTextContent(
        'translated:operation.cancel_failed:{"error":"cancel conflict"}',
      );
    });
    expect(status).toHaveTextContent("translated:diagnostic.state.failed");
  });
});
