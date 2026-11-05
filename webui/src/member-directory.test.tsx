import { cleanup, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type {
  MemberLifecycleAction,
  MemberLifecycleChange,
  MemberRemovalPreview,
  NetworkMember,
  NetworkMemberDirectory,
  NodeUiAction,
} from "../../contracts/typescript/node-contract-v1";
import {
  MemberDirectoryCard,
  type MemberGovernanceSubmit,
} from "./member-directory";
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
    lifecycle_state: "approved",
    revision: 1,
    access_epoch: 1,
    joined_at_unix_ms: generatedAtUnixMs - 60_000,
    ...overrides,
  };
}

function directory(members: NetworkMember[]): NetworkMemberDirectory {
  return {
    network_id: "net_home",
    generated_at_unix_ms: generatedAtUnixMs,
    viewer_role: "member",
    can_manage_members: false,
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


function action(id: string): NodeUiAction {
  return {
    id,
    label_key: `action.${id}`,
    command: id as NodeUiAction["command"],
    backend_status: "implemented",
    permission: "manage_networks",
    availability: "node_running",
    audit: { policy: "none" },
    fields: [],
  };
}

const previewRemovalAction = action("preview_member_removal");
const changeLifecycleAction = action("change_member_lifecycle");

function removalPreview(
  overrides: Partial<MemberRemovalPreview> = {},
): MemberRemovalPreview {
  return {
    network_id: "net_home",
    target_node_id: "node-000",
    role: "owner",
    status: "active",
    lifecycle_state: "active",
    revision: 7,
    access_epoch: 3,
    presence: {
      state: "online",
      source: "live_session",
      observed_at_unix_ms: generatedAtUnixMs - 1_000,
    },
    can_remove: true,
    network_access_revoked: true,
    file_access_revoked: true,
    incomplete_transfers_resume_blocked: true,
    credentials_revoked: true,
    local_files_deleted: false,
    old_access_epoch_invalidated: true,
    safe_undo_requires_reapproval: true,
    safe_undo_restores_old_credentials: false,
    impact_keys: [
      "member_removal.impact.network_access",
      "member_removal.impact.file_access",
      "member_removal.impact.incomplete_transfers",
      "member_removal.impact.credentials",
      "member_removal.impact.local_files_kept",
      "member_removal.impact.safe_undo",
    ],
    ...overrides,
  };
}

function lifecycleChange(
  overrides: Partial<MemberLifecycleChange> = {},
): MemberLifecycleChange {
  return {
    network_id: "net_home",
    target_node_id: "node-000",
    action: "remove",
    status: "removed",
    revision: 8,
    access_epoch: 4,
    access_revoked: true,
    requires_reapproval: true,
    requires_reauthentication: true,
    idempotent: false,
    operation_id: "op_lifecycle",
    audit_id: "audit_lifecycle",
    ...overrides,
  };
}

const ownerWithGovernance = {
  ...owner,
  available_actions: ["remove", "suspend"] as MemberLifecycleAction[],
};

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

  it("exposes governance only after the server grants management and actions", async () => {
    const user = userEvent.setup();
    const onSubmit = vi.fn(async () => ({}));
    const view = render(
      <MemberDirectoryCard
        directory={{ ...directory([owner]), can_manage_members: false }}
        locale="en"
        previewRemovalAction={previewRemovalAction}
        changeLifecycleAction={changeLifecycleAction}
        onSubmit={onSubmit}
      />,
    );
    await user.click(await screen.findByRole("option", { name: /Alpha owner/ }));
    expect(screen.queryByTestId("member-governance")).not.toBeInTheDocument();

    view.rerender(
      <MemberDirectoryCard
        directory={{
          ...directory([{ ...owner, available_actions: ["suspend"] }]),
          viewer_role: "owner",
          can_manage_members: true,
        }}
        locale="en"
        previewRemovalAction={previewRemovalAction}
        changeLifecycleAction={changeLifecycleAction}
        onSubmit={onSubmit}
      />,
    );
    const governance = await screen.findByTestId("member-governance");
    expect(
      within(governance).getByTestId("member-governance-action-suspend"),
    ).toBeInTheDocument();
    expect(
      screen.queryByTestId("member-governance-action-remove"),
    ).not.toBeInTheDocument();
    expect(onSubmit).not.toHaveBeenCalled();
  });

  it("previews removal impact before submitting the server-derived change", async () => {
    const user = userEvent.setup();
    const preview = removalPreview({ revision: 7 });
    const change = lifecycleChange({ action: "remove", revision: 8 });
    const onSubmit = vi.fn<MemberGovernanceSubmit>(
      async (submitted) =>
        submitted.id === "preview_member_removal"
          ? {
              outcome: {
                status: "member_removal_preview" as const,
                data: preview,
              },
            }
          : {
              outcome: {
                status: "member_lifecycle_changed" as const,
                data: change,
              },
            },
    );
    const onLifecycleChanged = vi.fn();
    render(
      <MemberDirectoryCard
        directory={{
          ...directory([ownerWithGovernance]),
          viewer_role: "owner",
          can_manage_members: true,
        }}
        locale="en"
        previewRemovalAction={previewRemovalAction}
        changeLifecycleAction={changeLifecycleAction}
        onSubmit={onSubmit}
        onLifecycleChanged={onLifecycleChanged}
      />,
    );
    await user.click(await screen.findByRole("option", { name: /Alpha owner/ }));
    await user.click(screen.getByTestId("member-governance-action-remove"));

    await waitFor(() => expect(onSubmit).toHaveBeenCalledTimes(1));
    expect(onSubmit.mock.calls[0][0]).toBe(previewRemovalAction);
    expect(onSubmit.mock.calls[0][1]).toEqual({
      network_id: "net_home",
      target_node_id: "node-000",
    });
    const impact = await screen.findByTestId("member-removal-impact");
    for (const key of preview.impact_keys) {
      expect(impact).toHaveTextContent(`translated:${key}`);
    }

    await user.type(
      screen.getByLabelText("translated:field.lifecycle_reason"),
      "  repeated abuse  ",
    );
    await user.click(screen.getByTestId("member-governance-confirm"));

    await waitFor(() => expect(onSubmit).toHaveBeenCalledTimes(2));
    expect(onSubmit.mock.calls[1][0]).toBe(changeLifecycleAction);
    expect(onSubmit.mock.calls[1][1]).toEqual({
      network_id: "net_home",
      target_node_id: "node-000",
      action: "remove",
      role: "owner",
      expected_revision: "7",
      reason: "repeated abuse",
    });
    expect(onLifecycleChanged).toHaveBeenCalledWith(change);
    expect(
      await screen.findByTestId("member-governance-feedback"),
    ).toHaveTextContent("translated:member_directory.governance.changed");
    expect(
      screen.queryByTestId("member-governance-dialog"),
    ).not.toBeInTheDocument();
  });

  it("submits every non-removal lifecycle action directly against the member revision", async () => {
    const user = userEvent.setup();
    const actions: MemberLifecycleAction[] = [
      "invite",
      "approve",
      "reject",
      "suspend",
      "resume",
      "exit",
      "undo_remove",
    ];
    for (const lifecycle of actions) {
      const onSubmit = vi.fn<MemberGovernanceSubmit>(async () => ({
        outcome: {
          status: "member_lifecycle_changed" as const,
          data: lifecycleChange({ action: lifecycle }),
        },
      }));
      const view = render(
        <MemberDirectoryCard
          directory={{
            ...directory([
              { ...owner, available_actions: [lifecycle] as MemberLifecycleAction[] },
            ]),
            viewer_role: "owner",
            can_manage_members: true,
          }}
          locale="en"
          changeLifecycleAction={changeLifecycleAction}
          onSubmit={onSubmit}
        />,
      );
      await user.click(
        await screen.findByRole("option", { name: /Alpha owner/ }),
      );
      await user.click(
        screen.getByTestId(`member-governance-action-${lifecycle}`),
      );
      expect(screen.queryByTestId("member-removal-impact")).not.toBeInTheDocument();
      await user.click(screen.getByTestId("member-governance-confirm"));
      await waitFor(() => expect(onSubmit).toHaveBeenCalledTimes(1));
      expect(onSubmit.mock.calls[0][1]).toEqual({
        network_id: "net_home",
        target_node_id: "node-000",
        action: lifecycle,
        role: "owner",
        expected_revision: "1",
      });
      await waitFor(() =>
        expect(
          screen.queryByTestId("member-governance-dialog"),
        ).not.toBeInTheDocument(),
      );
      view.unmount();
    }
  });

  it("blocks removal when the server denies it and reports rejection failures", async () => {
    const user = userEvent.setup();
    const denied = removalPreview({
      can_remove: false,
      denial_key: "member_lifecycle.error.owner_protected",
    });
    const rejected = {
      category: "authorization" as const,
      code: "permission_denied" as const,
      message: "denied",
      message_key: "member_lifecycle.error.permission_denied",
      retryable: false,
    };
    const onSubmit = vi.fn<MemberGovernanceSubmit>(
      async (submitted) =>
      submitted.id === "preview_member_removal"
        ? {
            outcome: {
              status: "member_removal_preview" as const,
              data: denied,
            },
          }
        : { outcome: { status: "rejected" as const, data: rejected } },
    );
    const onLifecycleChanged = vi.fn();
    render(
      <MemberDirectoryCard
        directory={{
          ...directory([ownerWithGovernance]),
          viewer_role: "owner",
          can_manage_members: true,
        }}
        locale="en"
        previewRemovalAction={previewRemovalAction}
        changeLifecycleAction={changeLifecycleAction}
        onSubmit={onSubmit}
        onLifecycleChanged={onLifecycleChanged}
      />,
    );
    await user.click(await screen.findByRole("option", { name: /Alpha owner/ }));
    await user.click(screen.getByTestId("member-governance-action-remove"));
    await screen.findByTestId("member-removal-impact");
    expect(screen.getByTestId("member-governance-confirm")).toBeDisabled();
    expect(
      screen.getByText("translated:member_directory.governance.unavailable"),
    ).toBeInTheDocument();
    expect(onLifecycleChanged).not.toHaveBeenCalled();
  });

  it("surfaces server rejections from a lifecycle change without closing the dialog", async () => {
    const user = userEvent.setup();
    const rejected = {
      category: "state" as const,
      code: "conflict" as const,
      message: "stale",
      message_key: "member_lifecycle.error.stale_revision",
      retryable: true,
    };
    const onSubmit = vi.fn<MemberGovernanceSubmit>(async () => ({
      outcome: { status: "rejected" as const, data: rejected },
    }));
    render(
      <MemberDirectoryCard
        directory={{
          ...directory([{ ...owner, available_actions: ["suspend"] }]),
          viewer_role: "owner",
          can_manage_members: true,
        }}
        locale="en"
        changeLifecycleAction={changeLifecycleAction}
        onSubmit={onSubmit}
      />,
    );
    await user.click(await screen.findByRole("option", { name: /Alpha owner/ }));
    await user.click(screen.getByTestId("member-governance-action-suspend"));
    await user.click(screen.getByTestId("member-governance-confirm"));
    await waitFor(() =>
      expect(
        screen.getByTestId("member-governance-dialog"),
      ).toHaveTextContent("translated:member_lifecycle.error.stale_revision"),
    );
    expect(
      screen.queryByTestId("member-governance-feedback"),
    ).not.toBeInTheDocument();
  });
});
