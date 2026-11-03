import { cleanup, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import manifestFixture from "../../../contracts/typescript/fixtures/node-manifest.json";
import productFixture from "../../../contracts/typescript/fixtures/product-visual-snapshot.json";
import type {
  NetworkProfileProjections,
  NetworkPublicProfileDocument,
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
const previewProfile = manifest.pages
  .flatMap((page) => page.actions)
  .find((action) => action.id === "preview_network_profile") as NodeUiAction;
const updateProfile = manifest.pages
  .flatMap((page) => page.actions)
  .find((action) => action.id === "update_network_profile") as NodeUiAction;
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

const publicProfile: NetworkPublicProfileDocument = {
  schema_version: 1,
  network_id: "net_stable_home",
  public_key: "network-public-key",
  owner: "node-owner",
  name: "home",
  purpose: "Family devices across two homes",
  icon: "home.router_2",
  language: "zh-CN",
  region: "CN",
  tags: ["family", "smart-home"],
  capability_declarations: ["relay"],
  visibility: "unlisted",
  join_mode: "invite_only",
  revision: 4,
  issued_at_unix_ms: 1_700_000_000_000,
  expires_at_unix_ms: 1_700_086_400_000,
};

const profileProjections: NetworkProfileProjections = {
  public_card: {
    candidate: publicProfile,
    currently_published: {
      document: { ...publicProfile, revision: 3 },
      signature: "signature",
    },
    withheld_categories: [
      "member_identity",
      "address",
      "topology",
      "activity",
      "invitation",
      "private_key",
    ],
  },
  member_view: {
    profile: publicProfile,
    member_count: 3,
    administrator_count: 1,
  },
  administrator_view: {
    profile: publicProfile,
    member_count: 3,
    administrator_count: 1,
    revision: 4,
    policy_revision: 2,
  },
  local_secret: {
    identity_present: true,
    secret_material_included: false,
    public_key_fingerprint: "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
    storage: "identity-keychain",
  },
};

const profileSnapshot: NodeSnapshot = {
  ...snapshot,
  network_profiles: [
    {
      name: "home",
      network_id: "net_stable_home",
      purpose: publicProfile.purpose,
      icon: publicProfile.icon,
      language: publicProfile.language,
      region: publicProfile.region,
      tags: publicProfile.tags,
      capability_declarations: publicProfile.capability_declarations,
      visibility: publicProfile.visibility,
      join_mode: publicProfile.join_mode,
      revision: publicProfile.revision,
      policy_revision: 2,
      signed_profile: {
        schema_version: 1,
        issued_at_unix_ms: publicProfile.issued_at_unix_ms,
        expires_at_unix_ms: publicProfile.expires_at_unix_ms,
      },
    },
  ],
};

const profileWarnings = [
  {
    code: "capability_not_observed" as const,
    field: "capability_declarations",
    capability: "relay" as const,
    message: "Relay capability has not been observed",
  },
  {
    code: "profile_expired" as const,
    field: "expires_at_unix_ms",
    message: "The published profile has expired",
  },
];

const profilePreviewOutcome = {
  status: "network_profile_preview" as const,
  data: {
    network_name: "home",
    expected_revision: 3,
    current_revision: 4,
    candidate_revision: 5,
    conflict: true,
    can_apply: false,
    projections: profileProjections,
    diff: [
      { path: "purpose", before: "Old purpose", after: publicProfile.purpose },
      { path: "tags", before: null, after: "family,smart-home" },
    ],
    warnings: profileWarnings,
  },
};

const profileUpdateOutcome = {
  status: "network_profile_updated" as const,
  data: {
    network_name: "home",
    revision: 5,
    previous_revision: 4,
    applied_at_unix_ms: 1_700_000_100_000,
    changed_paths: ["purpose", "tags"],
    signed_profile: {
      document: { ...publicProfile, revision: 5 },
      signature: "signature",
    },
    projections: {
      ...profileProjections,
      public_card: {
        ...profileProjections.public_card,
        candidate: { ...publicProfile, revision: 5 },
      },
    },
    warnings: profileWarnings,
    audit_action: "network.profile.update",
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

  it("renders the four profile projections, diff, warnings and conflict", async () => {
    const user = userEvent.setup();
    render(
      <ActionPanel
        actions={[previewProfile]}
        snapshot={profileSnapshot}
        selectedNetwork="home"
        locale="zh-CN"
        pendingAction={null}
        interactionBlocked={null}
        requestedActionId={null}
        operation={null}
        onRequestedActionOpened={() => {}}
        onCancel={vi.fn(async () => {})}
        onSubmit={vi.fn(async (): Promise<ActionSubmission> => ({
          outcome: profilePreviewOutcome as NodeCommandOutcome,
        }))}
      />,
    );

    await user.click(
      screen.getByRole("button", {
        name: "translated:action.preview_network_profile",
      }),
    );
    await user.click(
      screen.getByRole("button", { name: "translated:common.execute" }),
    );

    const result = await screen.findByTestId("network-profile-result");
    expect(result).toHaveTextContent("translated:profile.preview_title");
    for (const projection of [
      "public_card",
      "member_view",
      "admin_view",
      "local_secret",
    ]) {
      expect(result).toHaveTextContent(`translated:profile.${projection}`);
    }
    expect(result).toHaveTextContent("translated:profile.withheld");
    expect(result).toHaveTextContent("member_identity");
    expect(result).toHaveTextContent("translated:profile.conflict");
    expect(result).toHaveTextContent("translated:profile.cannot_apply");
    expect(result).toHaveTextContent("purpose");
    expect(result).toHaveTextContent("Old purpose");
    expect(result).toHaveTextContent(publicProfile.purpose);
    expect(result).toHaveTextContent("Relay capability has not been observed");
    expect(result).toHaveTextContent("The published profile has expired");
    expect(result).toHaveTextContent("translated:profile.signature_valid");
  });

  it("renders applied profile audit data and changed paths", async () => {
    const user = userEvent.setup();
    const onSubmit = vi.fn(
      async (): Promise<ActionSubmission> => ({
        outcome: profileUpdateOutcome as NodeCommandOutcome,
      }),
    );
    render(
      <ActionPanel
        actions={[updateProfile]}
        snapshot={profileSnapshot}
        selectedNetwork="home"
        locale="zh-CN"
        pendingAction={null}
        interactionBlocked={null}
        requestedActionId={null}
        operation={null}
        onRequestedActionOpened={() => {}}
        onCancel={vi.fn(async () => {})}
        onSubmit={onSubmit}
      />,
    );

    await user.click(
      screen.getByRole("button", {
        name: "translated:action.update_network_profile",
      }),
    );
    await user.click(
      screen.getByRole("button", { name: "translated:common.execute" }),
    );
    const dialog = await screen.findByRole("dialog");
    await user.click(
      within(dialog).getByRole("button", { name: "translated:common.execute" }),
    );

    const result = await screen.findByTestId("network-profile-result");
    expect(onSubmit).toHaveBeenCalledOnce();
    expect(result).toHaveTextContent("translated:profile.updated_title");
    expect(result).toHaveTextContent("translated:profile.audit");
    expect(result).toHaveTextContent("purpose");
    expect(result).toHaveTextContent("tags");
    expect(result).toHaveTextContent("translated:profile.revision");
  });
});
