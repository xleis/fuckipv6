import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import manifestFixture from "../../../contracts/typescript/fixtures/node-manifest.json";
import productFixture from "../../../contracts/typescript/fixtures/product-visual-snapshot.json";
import type {
  ConfigurationApplyResult,
  ConfigurationDocument,
  ConfigurationHistoryEntry,
  ConfigurationPreview,
  NodeCommandOutcome,
  NodeSnapshot,
  NodeUiAction,
  NodeUiManifest,
} from "../../../contracts/typescript/node-contract-v1";
import type { ActionValues } from "../contract/invariants";
import { installTestI18nRuntime } from "../test/runtime";
import {
  CONFIGURATION_ACTION_IDS,
  ConfigurationWorkflow,
} from "./configuration-workflow";

const manifest = manifestFixture as NodeUiManifest;
const snapshot = productFixture.snapshot as NodeSnapshot;
const actions = (manifest.pages.find((page) => page.id === "settings")?.actions ?? []).filter(
  (action) => CONFIGURATION_ACTION_IDS.has(action.id),
);

function documentAt(revision: number, mtu = 1280): ConfigurationDocument {
  return {
    schema_version: 1,
    revision,
    networks: { home: { mtu } },
  };
}

function previewAt(
  revision: number,
  mtu: number,
  overrides: Partial<ConfigurationPreview> = {},
): ConfigurationPreview {
  return {
    expected_revision: revision,
    current_revision: revision,
    candidate_revision: revision + 1,
    revision_conflict: false,
    current: documentAt(revision),
    candidate: documentAt(revision + 1, mtu),
    diffs: [
      {
        path: "/networks/home/mtu",
        before: 1280,
        after: mtu,
        impact: "restart_required",
      },
    ],
    validation_errors: [],
    has_changes: true,
    can_apply: true,
    ...overrides,
  };
}

function appliedAt(revision: number, mtu: number): ConfigurationApplyResult {
  return {
    previous_revision: revision,
    revision: revision + 1,
    no_op: false,
    document: documentAt(revision + 1, mtu),
    diffs: previewAt(revision, mtu).diffs,
    audit: historyAt(revision + 1, "apply"),
  };
}

function historyAt(revision: number, action: "apply" | "rollback" = "apply"): ConfigurationHistoryEntry {
  return {
    transaction_id: `tx-${revision}-${action}`,
    revision,
    previous_revision: revision - 1,
    action,
    target_revision: action === "rollback" ? revision - 2 : null,
    actor: "owner",
    source: "webui",
    occurred_at: "2026-11-02T00:00:00Z",
    changed_paths: ["/networks/home/mtu"],
  };
}

function renderWorkflow(onSubmit: (
  action: NodeUiAction,
  values: ActionValues,
) => Promise<NodeCommandOutcome>) {
  return render(
    <ConfigurationWorkflow
      actions={actions}
      snapshot={snapshot}
      selectedNetwork="home"
      locale="zh-CN"
      pendingAction={null}
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

describe("transactional configuration workflow", () => {
  it("loads a document, previews an edit, confirms apply, and refreshes history", async () => {
    const user = userEvent.setup();
    const onSubmit = vi.fn(async (action: NodeUiAction): Promise<NodeCommandOutcome> => {
      switch (action.id) {
        case "get_configuration":
          return { status: "configuration", data: documentAt(2) };
        case "get_configuration_history":
          return { status: "configuration_history", data: [historyAt(2)] };
        case "preview_configuration":
          return { status: "configuration_preview", data: previewAt(2, 1400) };
        case "apply_configuration":
          return { status: "configuration_applied", data: appliedAt(2, 1400) };
        default:
          throw new Error(`unexpected action ${action.id}`);
      }
    });
    renderWorkflow(onSubmit);

    const editor = await screen.findByRole("textbox", {
      name: "translated:configuration.editor.label",
    });
    expect(editor).toHaveValue(JSON.stringify(documentAt(2).networks, null, 2));

    fireEvent.change(editor, {
      target: { value: JSON.stringify({ home: { mtu: 1400 } }, null, 2) },
    });
    await user.click(
      screen.getByRole("button", { name: "translated:configuration.preview" }),
    );

    expect((await screen.findAllByText("/networks/home/mtu")).length).toBeGreaterThan(0);
    expect(screen.getByText("translated:configuration.apply.ready")).toBeInTheDocument();
    await user.click(
      screen.getByRole("button", { name: "translated:configuration.apply" }),
    );
    await user.click(
      await screen.findByRole("button", {
        name: "translated:configuration.apply.confirm_action",
      }),
    );

    await waitFor(() => {
      expect(onSubmit).toHaveBeenCalledWith(
        expect.objectContaining({ id: "apply_configuration" }),
        expect.objectContaining({
          expected_revision: "2",
          networks: JSON.stringify({ home: { mtu: 1400 } }, null, 2),
        }),
      );
    });
    expect(await screen.findByText("translated:outcome.configuration_applied")).toBeInTheDocument();
    expect(screen.getByText("translated:configuration.audit.title")).toBeInTheDocument();
  });

  it("locates validation issues in the editor and blocks apply", async () => {
    const user = userEvent.setup();
    const invalidPreview = previewAt(2, 900, {
      can_apply: false,
      validation_errors: [
        {
          path: "/networks/home/mtu",
          code: "invalid_value",
          message: "MTU out of range",
        },
      ],
    });
    const onSubmit = vi.fn(async (action: NodeUiAction): Promise<NodeCommandOutcome> => {
      if (action.id === "get_configuration") {
        return { status: "configuration", data: documentAt(2) };
      }
      if (action.id === "get_configuration_history") {
        return { status: "configuration_history", data: [] };
      }
      if (action.id === "preview_configuration") {
        return { status: "configuration_preview", data: invalidPreview };
      }
      throw new Error(`unexpected action ${action.id}`);
    });
    renderWorkflow(onSubmit);

    const editor = await screen.findByRole("textbox", {
      name: "translated:configuration.editor.label",
    });
    fireEvent.change(editor, {
      target: { value: JSON.stringify({ home: { mtu: 900 } }, null, 2) },
    });
    await user.click(
      screen.getByRole("button", { name: "translated:configuration.preview" }),
    );

    const [issuePath] = await screen.findAllByText("/networks/home/mtu");
    await user.click(issuePath.closest("button")!);
    expect(editor).toHaveFocus();
    expect((editor as HTMLTextAreaElement).selectionStart).toBeGreaterThan(0);
    expect(
      screen.getByRole("button", { name: "translated:configuration.apply" }),
    ).toBeDisabled();
  });

  it("reports malformed JSON, imports a networks file, and reports revision conflicts", async () => {
    const user = userEvent.setup();
    let conflict = false;
    const onSubmit = vi.fn(async (action: NodeUiAction): Promise<NodeCommandOutcome> => {
      if (action.id === "get_configuration") {
        return { status: "configuration", data: documentAt(2) };
      }
      if (action.id === "get_configuration_history") {
        return { status: "configuration_history", data: [] };
      }
      if (action.id === "preview_configuration") {
        return {
          status: "configuration_preview",
          data: previewAt(2, 1500, conflict
            ? {
                current_revision: 3,
                revision_conflict: true,
                can_apply: false,
              }
            : {}),
        };
      }
      throw new Error(`unexpected action ${action.id}`);
    });
    const { container } = renderWorkflow(onSubmit);

    const editor = await screen.findByRole("textbox", {
      name: "translated:configuration.editor.label",
    });
    fireEvent.change(editor, { target: { value: "{not json" } });
    await user.click(
      screen.getByRole("button", { name: "translated:configuration.preview" }),
    );
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "translated:validation.configuration_json_invalid",
    );

    const raw = JSON.stringify({ networks: { home: { mtu: 1500 } } });
    const file = new File([raw], "settings.json", { type: "application/json" });
    Object.defineProperty(file, "text", { value: async () => raw });
    await user.upload(container.querySelector('input[type="file"]')!, file);
    expect(await screen.findByText("translated:configuration.imported_from")).toBeInTheDocument();
    expect(editor).toHaveValue(JSON.stringify({ home: { mtu: 1500 } }, null, 2));

    conflict = true;
    await user.click(
      screen.getByRole("button", { name: "translated:configuration.preview" }),
    );
    expect(
      (await screen.findAllByText("translated:configuration.revision_conflict")).length,
    ).toBeGreaterThan(0);
    expect(
      screen.getByRole("button", { name: "translated:configuration.apply" }),
    ).toBeDisabled();
  });

  it("confirms rollback and exposes the resulting audit details", async () => {
    const user = userEvent.setup();
    const rollbackEntry = historyAt(4, "rollback");
    const onSubmit = vi.fn(async (
      action: NodeUiAction,
      values: ActionValues,
    ): Promise<NodeCommandOutcome> => {
      if (action.id === "get_configuration") {
        return { status: "configuration", data: documentAt(3) };
      }
      if (action.id === "get_configuration_history") {
        return { status: "configuration_history", data: [historyAt(2)] };
      }
      if (action.id === "rollback_configuration") {
        expect(values).toEqual({ expected_revision: "3", target_revision: "2" });
        return {
          status: "configuration_applied",
          data: {
            previous_revision: 3,
            revision: 4,
            no_op: false,
            document: documentAt(4),
            diffs: previewAt(3, 1280).diffs,
            audit: rollbackEntry,
          },
        };
      }
      throw new Error(`unexpected action ${action.id}`);
    });
    renderWorkflow(onSubmit);

    await user.click(
      await screen.findByRole("button", {
        name: "translated:configuration.rollback.action",
      }),
    );
    await user.click(
      screen.getByRole("button", {
        name: "translated:configuration.rollback.confirm_action",
      }),
    );

    expect(await screen.findByText("translated:configuration.audit.title")).toBeInTheDocument();
    expect(screen.getByText(rollbackEntry.transaction_id)).toBeInTheDocument();
    expect(onSubmit).toHaveBeenCalledWith(
      expect.objectContaining({ id: "rollback_configuration" }),
      { expected_revision: "3", target_revision: "2" },
    );
  });

  it("surfaces asynchronous command failures without applying the draft", async () => {
    const user = userEvent.setup();
    const onSubmit = vi.fn(async (action: NodeUiAction): Promise<NodeCommandOutcome> => {
      if (action.id === "get_configuration") {
        return { status: "configuration", data: documentAt(2) };
      }
      if (action.id === "get_configuration_history") {
        return { status: "configuration_history", data: [] };
      }
      if (action.id === "preview_configuration") {
        throw new Error("control service unavailable");
      }
      throw new Error(`unexpected action ${action.id}`);
    });
    renderWorkflow(onSubmit);

    await screen.findByRole("textbox", {
      name: "translated:configuration.editor.label",
    });
    await user.click(
      screen.getByRole("button", { name: "translated:configuration.preview" }),
    );

    expect(await screen.findByRole("alert")).toHaveTextContent(
      "control service unavailable",
    );
    expect(onSubmit).not.toHaveBeenCalledWith(
      expect.objectContaining({ id: "apply_configuration" }),
      expect.anything(),
    );
  });
});
