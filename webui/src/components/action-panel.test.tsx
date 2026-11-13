import { cleanup, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import manifestFixture from "../../../contracts/typescript/fixtures/node-manifest.json";
import productFixture from "../../../contracts/typescript/fixtures/product-visual-snapshot.json";
import type {
  InvitationPreview,
  MemberLifecycleChange,
  MemberRemovalPreview,
  InvitationStatusView,
  IssuedInvitation,
  JoinRequest,
  NetworkMemberDirectory,
  NetworkProfileProjections,
  NetworkPublicProfileDocument,
  NodeCommandOutcome,
  NodeNetworkJoinPreview,
  NodeOperationSnapshot,
  NodeSnapshot,
  NodeUiAction,
  NodeUiManifest,
} from "../../../contracts/typescript/node-contract-v1";
import type { ActionValues } from "../contract/invariants";
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
const previewJoin = manifest.pages
  .flatMap((page) => page.actions)
  .find((action) => action.id === "preview_join_network") as NodeUiAction;
const joinNetwork = manifest.pages
  .flatMap((page) => page.actions)
  .find((action) => action.id === "join_network") as NodeUiAction;
const leaveNetwork = manifest.pages
  .flatMap((page) => page.actions)
  .find((action) => action.id === "leave_network") as NodeUiAction;
const exitNodeActions = manifest.pages.find((page) => page.id === "exit_node")!
  .actions;
const setLocalExitCapability = exitNodeActions.find(
  (action) => action.id === "set_local_exit_capability",
) as NodeUiAction;
const setExitNodeRole = exitNodeActions.find(
  (action) => action.id === "set_exit_node_role",
) as NodeUiAction;
const listJoinRequests = manifest.pages
  .flatMap((page) => page.actions)
  .find((action) => action.id === "list_join_requests") as NodeUiAction;
const listNetworkMembers = manifest.pages
  .flatMap((page) => page.actions)
  .find((action) => action.id === "list_network_members") as NodeUiAction;
const reviewJoinRequest = manifest.pages
  .flatMap((page) => page.actions)
  .find((action) => action.id === "review_join_request") as NodeUiAction;
const batchReviewJoinRequests = manifest.pages
  .flatMap((page) => page.actions)
  .find((action) => action.id === "batch_review_join_requests") as NodeUiAction;
const submitJoinRequest = manifest.pages
  .flatMap((page) => page.actions)
  .find((action) => action.id === "submit_join_request") as NodeUiAction;
const createInvitation = manifest.pages
  .flatMap((page) => page.actions)
  .find((action) => action.id === "create_invitation") as NodeUiAction;
const listInvitations = manifest.pages
  .flatMap((page) => page.actions)
  .find((action) => action.id === "list_invitations") as NodeUiAction;
const getInvitationStatus = manifest.pages
  .flatMap((page) => page.actions)
  .find((action) => action.id === "get_invitation_status") as NodeUiAction;
const revokeInvitation = manifest.pages
  .flatMap((page) => page.actions)
  .find((action) => action.id === "revoke_invitation") as NodeUiAction;
const previewInvitation = manifest.pages
  .flatMap((page) => page.actions)
  .find((action) => action.id === "preview_invitation") as NodeUiAction;
const redeemInvitation = manifest.pages
  .flatMap((page) => page.actions)
  .find((action) => action.id === "redeem_invitation") as NodeUiAction;
const joinRequestReviewActions = [
  listJoinRequests,
  reviewJoinRequest,
  batchReviewJoinRequests,
];
const currentFingerprint = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
const nextFingerprint = "BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB";
const editedFingerprint = "CCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCC";
const memberDirectory: NetworkMemberDirectory = {
  network_id: "net_stable_home",
  generated_at_unix_ms: 1_700_000_100_000,
  viewer_role: "owner",
  can_manage_members: true,
  members: [
    {
      node_id: "node-000",
      display_name: "Alpha owner",
      identity_fingerprint: currentFingerprint,
      role: "owner",
      groups: ["owner"],
      capabilities: ["coordinator", "file"],
      status: "active",
      lifecycle_state: "active",
      revision: 3,
      access_epoch: 2,
      presence: {
        state: "online",
        source: "live_session",
        observed_at_unix_ms: 1_700_000_095_000,
      },
      joined_at_unix_ms: 1_700_000_000_000,
    },
  ],
};
const identitySnapshot: NodeSnapshot = {
  ...snapshot,
  identity: {
    node_id: "node-current",
    display_name: "Current node",
    public_key_fingerprint: currentFingerprint,
    created_at_unix_ms: 1_700_000_000_000,
  },
};

const invitationCode = `fkinv_${"A".repeat(43)}`;
const invitationNetworkId = "net_stable_home";
const invitationId = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
const invitationCreatedAt = 1_700_000_000_000;
const invitationStatus: InvitationStatusView = {
  invitation: {
    invitation_id: invitationId,
    network_id: invitationNetworkId,
    created_by: "node-owner",
    role: "member",
    capabilities: ["file"],
    max_uses: 2,
    used_count: 0,
    created_at_unix_ms: invitationCreatedAt,
    expires_at_unix_ms: invitationCreatedAt + 3_600_000,
    revision: 3,
  },
  state: "active",
  remaining_uses: 2,
};
const issuedInvitation: IssuedInvitation = {
  code: invitationCode,
  status: invitationStatus,
};
const invitationPreview: InvitationPreview = {
  invitation_id: invitationId,
  invitation_revision: 3,
  network_id: invitationNetworkId,
  network_name: "home",
  network_revision: 8,
  role: "member",
  capabilities: ["file"],
  remaining_uses: 2,
  expires_at_unix_ms: invitationCreatedAt + 3_600_000,
  state: "active",
  terms: {
    network_policy_revision: 4,
    privacy_policy_revision: 3,
    exit_contribution: "not_required",
    member_limit: 16,
  },
  terms_hash: "BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB",
  preview_digest: "CCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCC",
  permission_changes: ["File access is limited to shared folders"],
  creates_permanent_membership: false,
  requires_recent_reauthentication: true,
  can_accept: true,
  denial_key: null,
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
      exit_contribution: "not_required",
      exit_contribution_scope: "not_applicable",
      exit_contribution_mode: "disabled",
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

const joinPreview: NodeNetworkJoinPreview = {
  network_id: "network-family",
  network_name: "family",
  public_key: "network-public-key",
  owner: "owner-family",
  administrators: ["admin-one", "admin-two"],
  discovery_revision: 7,
  network_policy_revision: 4,
  privacy_policy_revision: 3,
  join_mode: "open",
  visibility: "public",
  capability_summary: ["relay"],
  endpoints: [
    { host: "relay.example.test", port: 443, transport: "websocket" },
  ],
  member_count: 8,
  member_limit: 16,
  exit_contribution: "required",
  exit_contribution_scope: "joining_members",
  exit_contribution_mode: "required_on_join",
  exit_contribution_purpose_key: "join.exit.disclosure.purpose",
  exit_contribution_bandwidth_impact_key: "join.exit.disclosure.bandwidth",
  exit_contribution_traffic_impact_key: "join.exit.disclosure.traffic",
  exit_contribution_logging_boundary_key: "join.exit.disclosure.logging",
  exit_contribution_operational_impact_key:
    "join.exit.disclosure.operational_impact",
  exit_contribution_disable_key: "join.exit.disclosure.disable",
  local_exit_available: true,
  local_exit_enabled: false,
  can_join_without_local_exit: false,
  join_requires_exit_consent: true,
  privacy_changes: ["Operators can observe connection metadata"],
  already_member: false,
  can_join: true,
  blocked_reason_key: null,
  terms_hash: "terms-hash-family",
  preview_digest: "preview-digest-family",
  issued_at_unix_ms: 1_700_000_000_000,
  expires_at_unix_ms: 1_700_086_400_000,
  locally_contributed_capabilities: [],
  contribution_policy_key: "join.contribution.local_control",
};

const joinPreviewOutcome = {
  status: "network_join_preview" as const,
  data: joinPreview,
};

const joinedOutcome = {
  status: "network_joined" as const,
  data: {
    config_saved: true,
    idempotent: false,
    membership: {
      network_id: "network-family",
      network_name: "family",
      node_id: "node-current",
      role: "member",
      state: "accepted",
      local_policy_revision: 1,
      acknowledged_at_unix_ms: 1_700_000_000_123,
      created_at_unix_ms: 1_700_000_000_000,
      updated_at_unix_ms: 1_700_000_000_123,
    },
    snapshot,
    started: true,
  },
};

const joinRequest: JoinRequest = {
  schema_version: 1,
  request_id: `join_${"A".repeat(43)}`,
  network_id: "net_stable_home",
  applicant: {
    node_id: `node_${"B".repeat(43)}`,
    display_name: "Branch office router",
    public_key_fingerprint: "SHA256:branch-office-public-key",
  },
  reason: "Connect the branch office devices to the family network.",
  reason_visibility: "owners_and_administrators",
  state: "pending",
  revision: 3,
  terms: {
    network_policy_revision: 4,
    privacy_policy_revision: 2,
    exit_contribution: "required",
    member_limit: 16,
  },
  grant_preview: {
    role: "member",
    capabilities: ["relay", "file"],
    exit_contribution: "required",
    file_access: true,
    grants_data_plane_access: true,
  },
  created_at_unix_ms: 1_700_000_000_000,
  updated_at_unix_ms: 1_700_000_100_000,
  expires_at_unix_ms: 1_700_600_000_000,
  reason_retention_until_unix_ms: 1_703_000_000_000,
};

const joinRequestsOutcome = {
  status: "join_requests" as const,
  data: {
    network_id: "net_stable_home",
    requests: [joinRequest],
  },
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
    outcome: { status: "completed", data: {} } as unknown as NodeCommandOutcome,
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
      requestedAction={null}
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

describe("action panel Exit confirmations", () => {
  it("requires confirmation before changing the local Exit capability", async () => {
    const user = userEvent.setup();
    const onSubmit = vi.fn(async (): Promise<ActionSubmission> => ({
      outcome: {
        status: "completed",
        data: {},
      } as unknown as NodeCommandOutcome,
    }));
    render(
      <ActionPanel
        actions={[setLocalExitCapability]}
        snapshot={snapshot}
        selectedNetwork=""
        locale="zh-CN"
        pendingAction={null}
        interactionBlocked={null}
        requestedAction={null}
        operation={null}
        onRequestedActionOpened={() => {}}
        onCancel={vi.fn(async () => {})}
        onSubmit={onSubmit}
      />,
    );

    await user.click(
      screen.getByRole("button", {
        name: "translated:action.set_local_exit_capability",
      }),
    );
    const enabled = screen.getByRole("checkbox", {
      name: "translated:field.enabled",
    });
    expect(enabled).not.toBeChecked();
    await user.click(enabled);
    await user.click(
      screen.getByRole("button", { name: "translated:common.execute" }),
    );

    expect(onSubmit).not.toHaveBeenCalled();
    const dialog = await screen.findByRole("dialog");
    expect(dialog).toHaveTextContent(
      "translated:confirm.set_local_exit_capability.title",
    );
    expect(dialog).toHaveTextContent(
      "translated:confirm.set_local_exit_capability.consequence",
    );
    await user.click(
      within(dialog).getByRole("button", {
        name: "translated:common.execute",
      }),
    );

    await waitFor(() => expect(onSubmit).toHaveBeenCalledOnce());
    expect(onSubmit).toHaveBeenCalledWith(
      setLocalExitCapability,
      { enabled: true },
      expect.any(Function),
    );
  });

  it("confirms the selected network and role before enabling Exit", async () => {
    const user = userEvent.setup();
    const onSubmit = vi.fn(async (): Promise<ActionSubmission> => ({
      outcome: {
        status: "completed",
        data: {},
      } as unknown as NodeCommandOutcome,
    }));
    render(
      <ActionPanel
        actions={[setExitNodeRole]}
        snapshot={snapshot}
        selectedNetwork="home"
        locale="zh-CN"
        pendingAction={null}
        interactionBlocked={null}
        requestedAction={null}
        operation={null}
        onRequestedActionOpened={() => {}}
        onCancel={vi.fn(async () => {})}
        onSubmit={onSubmit}
      />,
    );

    await user.click(
      screen.getByRole("button", {
        name: "translated:action.set_exit_node_role",
      }),
    );
    expect(
      screen.getByLabelText("translated:field.network_name"),
    ).toHaveValue("home");
    expect(screen.getByLabelText("translated:field.role")).toHaveValue(
      "exit_node",
    );
    const enabled = screen.getByRole("checkbox", {
      name: "translated:field.enabled",
    });
    expect(enabled).not.toBeChecked();
    await user.click(enabled);
    await user.click(
      screen.getByRole("button", { name: "translated:common.execute" }),
    );

    expect(onSubmit).not.toHaveBeenCalled();
    const dialog = await screen.findByRole("dialog");
    expect(dialog).toHaveTextContent(
      "translated:confirm.set_exit_node_role.title",
    );
    expect(dialog).toHaveTextContent(
      "translated:confirm.set_exit_node_role.consequence",
    );
    await user.click(
      within(dialog).getByRole("button", {
        name: "translated:common.execute",
      }),
    );

    await waitFor(() => expect(onSubmit).toHaveBeenCalledOnce());
    expect(onSubmit).toHaveBeenCalledWith(
      setExitNodeRole,
      { network_name: "home", role: "exit_node", enabled: true },
      expect.any(Function),
    );
  });
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
        requestedAction={null}
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
        requestedAction={null}
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
        requestedAction={null}
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
        requestedAction={null}
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

describe("action panel selected network synchronization", () => {
  it("updates network defaults without remounting the action card", async () => {
    const user = userEvent.setup();
    const onSubmit = vi.fn(async (): Promise<ActionSubmission> => ({
      outcome: { status: "completed", data: {} } as NodeCommandOutcome,
    }));
    const view = render(
      <ActionPanel
        actions={[leaveNetwork]}
        snapshot={snapshot}
        selectedNetwork="home"
        locale="zh-CN"
        pendingAction={null}
        interactionBlocked={null}
        requestedAction={null}
        operation={null}
        onRequestedActionOpened={() => {}}
        onCancel={vi.fn(async () => {})}
        onSubmit={onSubmit}
      />,
    );

    await user.click(
      screen.getByRole("button", { name: "translated:action.leave_network" }),
    );
    const input = screen.getByLabelText("translated:field.network_name");
    expect(input).toHaveValue("home");

    view.rerender(
      <ActionPanel
        actions={[leaveNetwork]}
        snapshot={snapshot}
        selectedNetwork="studio"
        locale="zh-CN"
        pendingAction={null}
        interactionBlocked={null}
        requestedAction={null}
        operation={null}
        onRequestedActionOpened={() => {}}
        onCancel={vi.fn(async () => {})}
        onSubmit={onSubmit}
      />,
    );

    await waitFor(() => expect(input).toHaveValue("studio"));
    expect(screen.getByLabelText("translated:field.network_name")).toBe(input);
  });
});

describe("action panel operation status", () => {
  it("shows structured failure details and copies the diagnostic id", async () => {
    const user = userEvent.setup();
    const onRetry = vi.fn(async () => {});
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
        actions={[]}
        snapshot={snapshot}
        selectedNetwork=""
        locale="zh-CN"
        pendingAction={null}
        interactionBlocked={null}
        requestedAction={null}
        operation={operation}
        onRetryOperation={onRetry}
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

    await user.click(
      within(status).getByRole("button", {
        name: "translated:operation.retry",
      }),
    );
    expect(onRetry).toHaveBeenCalledOnce();
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
        requestedAction={null}
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
        requestedAction={null}
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
        requestedAction={null}
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
        requestedAction={null}
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

  it("renders a signed join preview and submits confirmed terms at click time", async () => {
    const user = userEvent.setup();
    const now = vi.spyOn(Date, "now").mockReturnValue(1_700_000_000_123);
    const onSubmit = vi
      .fn<
        (
          action: NodeUiAction,
          values: ActionValues,
          onOperation: (operation: NodeOperationSnapshot) => void,
        ) => Promise<ActionSubmission>
      >()
      .mockResolvedValueOnce({
        outcome: joinPreviewOutcome as NodeCommandOutcome,
      })
      .mockResolvedValueOnce({
        outcome: joinedOutcome as NodeCommandOutcome,
      });
    render(
      <ActionPanel
        actions={[previewJoin, joinNetwork]}
        snapshot={snapshot}
        selectedNetwork=""
        locale="zh-CN"
        pendingAction={null}
        interactionBlocked={null}
        requestedAction={{
          actionId: "preview_join_network",
          values: { network_id: "network-family" },
        }}
        operation={null}
        onRequestedActionOpened={() => {}}
        onCancel={vi.fn(async () => {})}
        onSubmit={onSubmit}
      />,
    );

    await user.click(
      screen.getByRole("button", { name: "translated:common.execute" }),
    );
    const preview = await screen.findByTestId("network-join-preview");
    expect(preview).toHaveTextContent("translated:join.preview_title");
    expect(preview).toHaveTextContent("network-family");
    expect(preview).toHaveTextContent("owner-family");
    expect(preview).toHaveTextContent("admin-one");
    expect(preview).toHaveTextContent("Operators can observe connection metadata");
    expect(preview).toHaveTextContent("translated:join.local_capabilities_none");
    expect(preview).toHaveTextContent(
      "translated:join.contribution.local_control",
    );

    const confirm = within(preview).getByRole("button", {
      name: "translated:join.confirm",
    });
    expect(confirm).toBeDisabled();
    const terms = within(preview).getByRole("checkbox", {
      name: "translated:join.terms_checkbox",
    });
    const exitConsent = within(preview).getByRole("checkbox", {
      name: "translated:join.exit.consent",
    });
    await user.click(terms);
    expect(confirm).toBeDisabled();
    await user.click(
      within(preview).getByRole("checkbox", {
        name: "translated:join.terms_checkbox",
      }),
    );
    await user.click(terms);
    expect(confirm).toBeDisabled();
    await user.click(exitConsent);
    expect(confirm).toBeEnabled();
    expect(preview).toHaveTextContent("translated:join.exit.local_not_enabled");
    expect(preview).toHaveTextContent(
      "translated:join.exit.cannot_join_without_local_exit",
    );
    await user.click(confirm);

    await waitFor(() => expect(onSubmit).toHaveBeenCalledTimes(2));
    expect(onSubmit.mock.calls[1]?.[0]).toBe(joinNetwork);
    expect(onSubmit.mock.calls[1]?.[1]).toMatchObject({
      network_id: "network-family",
      network_name: "family",
      preview_digest: "preview-digest-family",
      network_policy_revision: "4",
      privacy_policy_revision: "3",
      terms_hash: "terms-hash-family",
      acknowledged_at_unix_ms: "1700000000123",
      start: true,
      exit_contribution_acknowledged: "true",
    });
    expect(
      await screen.findByText("translated:outcome.network_joined"),
    ).toBeInTheDocument();
    now.mockRestore();
  });

  it("keeps an unconfirmed preview available when the final join is rejected", async () => {
    const user = userEvent.setup();
    const refreshedJoinPreview = {
      ...joinPreview,
      preview_digest: "preview-digest-family-refreshed",
    };
    const onSubmit = vi
      .fn<
        (
          action: NodeUiAction,
          values: ActionValues,
          onOperation: (operation: NodeOperationSnapshot) => void,
        ) => Promise<ActionSubmission>
      >()
      .mockResolvedValueOnce({
        outcome: joinPreviewOutcome as NodeCommandOutcome,
      })
      .mockResolvedValueOnce({
        outcome: {
          status: "rejected",
          data: {
            category: "validation",
            code: "conflict",
            message: "Preview changed",
            message_key: "join.error.preview_changed",
            retryable: true,
            network_name: "family",
          },
        } as NodeCommandOutcome,
      })
      .mockResolvedValueOnce({
        outcome: {
          status: "network_join_preview",
          data: refreshedJoinPreview,
        } as NodeCommandOutcome,
      })
      .mockResolvedValueOnce({
        outcome: joinedOutcome as NodeCommandOutcome,
      });
    render(
      <ActionPanel
        actions={[previewJoin, joinNetwork]}
        snapshot={snapshot}
        selectedNetwork=""
        locale="zh-CN"
        pendingAction={null}
        interactionBlocked={null}
        requestedAction={{
          actionId: "preview_join_network",
          values: { network_id: "network-family" },
        }}
        operation={null}
        onRequestedActionOpened={() => {}}
        onCancel={vi.fn(async () => {})}
        onSubmit={onSubmit}
      />,
    );

    await user.click(
      screen.getByRole("button", { name: "translated:common.execute" }),
    );
    const preview = await screen.findByTestId("network-join-preview");
    const checkbox = within(preview).getByRole("checkbox", {
      name: "translated:join.terms_checkbox",
    });
    const exitConsent = within(preview).getByRole("checkbox", {
      name: "translated:join.exit.consent",
    });
    await user.click(checkbox);
    await user.click(exitConsent);
    await user.click(
      within(preview).getByRole("button", {
        name: "translated:join.confirm",
      }),
    );

    expect(await screen.findByRole("alert")).toHaveTextContent(
      "translated:join.error.preview_changed",
    );
    expect(screen.getByTestId("network-join-preview")).toBeInTheDocument();
    expect(checkbox).toBeChecked();
    expect(exitConsent).toBeChecked();

    await user.click(
      within(preview).getByRole("button", {
        name: "translated:join.preview_again",
      }),
    );
    await waitFor(() => expect(onSubmit).toHaveBeenCalledTimes(3));
    expect(onSubmit.mock.calls[2]?.[0]).toBe(previewJoin);
    expect(onSubmit.mock.calls[2]?.[1]).toEqual({
      network_id: "network-family",
    });
    await waitFor(() => expect(checkbox).not.toBeChecked());
    expect(exitConsent).not.toBeChecked();
    expect(
      within(preview).getByRole("button", { name: "translated:join.confirm" }),
    ).toBeDisabled();

    await user.click(checkbox);
    await user.click(exitConsent);
    await user.click(
      within(preview).getByRole("button", { name: "translated:join.confirm" }),
    );
    await waitFor(() => expect(onSubmit).toHaveBeenCalledTimes(4));
    expect(screen.queryByTestId("network-join-preview")).not.toBeInTheDocument();
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
        requestedAction={null}
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

describe("action panel private invitation workflows", () => {
  it("creates an invitation, shows its code once and copies it without putting it in the notice", async () => {
    const user = userEvent.setup();
    const writeText = vi
      .spyOn(navigator.clipboard, "writeText")
      .mockResolvedValue(undefined);
    const administratorIssued: IssuedInvitation = {
      ...issuedInvitation,
      status: {
        ...issuedInvitation.status,
        invitation: {
          ...issuedInvitation.status.invitation,
          role: "administrator",
        },
      },
    };
    const onSubmit = vi.fn<
      (
        action: NodeUiAction,
        values: ActionValues,
        onOperation: (operation: NodeOperationSnapshot) => void,
      ) => Promise<ActionSubmission>
    >(async () => ({
      outcome: {
        status: "invitation_created",
        data: administratorIssued,
      } as NodeCommandOutcome,
    }));
    render(
      <ActionPanel
        actions={[createInvitation]}
        snapshot={snapshot}
        selectedNetwork="home"
        locale="zh-CN"
        pendingAction={null}
        interactionBlocked={null}
        requestedAction={null}
        operation={null}
        onRequestedActionOpened={() => {}}
        onCancel={vi.fn(async () => {})}
        onSubmit={onSubmit}
      />,
    );

    await user.click(
      screen.getByRole("button", {
        name: "translated:action.create_invitation",
      }),
    );
    const role = screen.getByRole("combobox", {
      name: "translated:field.invitation_role",
    });
    expect(role).toHaveValue("member");
    expect(
      within(role).getAllByRole("option").map((option) => option.textContent),
    ).toEqual([
      "translated:join_request.role.member",
      "translated:join_request.role.administrator",
    ]);

    const network = screen.getByRole("textbox", {
      name: "translated:field.network_id",
    });
    await user.clear(network);
    await user.type(network, invitationNetworkId);
    await user.selectOptions(role, "administrator");
    await user.type(
      screen.getByRole("textbox", {
        name: "translated:field.capability_declarations",
      }),
      "file",
    );
    await user.type(
      screen.getByRole("textbox", {
        name: "translated:field.invitation_max_uses",
      }),
      "2",
    );
    await user.type(
      screen.getByRole("spinbutton", {
        name: "translated:field.invitation_expires_in_ms",
      }),
      "3600000",
    );
    await user.click(
      screen.getByRole("button", { name: "translated:common.execute" }),
    );
    const confirmation = await screen.findByRole("dialog");
    expect(confirmation).toHaveTextContent(
      "translated:confirm.reauthentication_required",
    );
    await user.click(
      within(confirmation).getByRole("button", {
        name: "translated:common.execute",
      }),
    );

    const created = await screen.findByTestId("invitation-created");
    expect(onSubmit).toHaveBeenCalledOnce();
    expect(onSubmit.mock.calls[0]?.[0]).toBe(createInvitation);
    expect(onSubmit.mock.calls[0]?.[1]).toMatchObject({
      network_id: invitationNetworkId,
      role: "administrator",
      capabilities: "file",
      max_uses: "2",
      expires_in_ms: "3600000",
    });
    expect(screen.getAllByText(invitationCode)).toHaveLength(1);
    expect(created).toHaveTextContent("translated:invitations.code_once");
    expect(created).toHaveTextContent("translated:join_request.role.administrator");
    expect(created).toHaveTextContent("translated:invitations.ordinary_membership");
    expect(created).toHaveTextContent("translated:invitations.not_permanent");

    const article = created.closest("article");
    expect(article?.querySelector(".form-success")).toHaveTextContent(
      "translated:invitations.created_notice",
    );
    expect(article?.querySelector(".form-success")).not.toHaveTextContent(
      invitationCode,
    );

    await user.click(
      within(created).getByRole("button", {
        name: "translated:invitations.copy",
      }),
    );
    expect(writeText).toHaveBeenCalledWith(invitationCode);
    expect(within(created).getByRole("status")).toHaveTextContent(
      "translated:invitations.copied",
    );
  });

  it("lists only redacted metadata, refreshes status and revokes with a pinned revision", async () => {
    const user = userEvent.setup();
    const refreshed: InvitationStatusView = {
      ...invitationStatus,
      invitation: { ...invitationStatus.invitation, revision: 4 },
    };
    const revoked: InvitationStatusView = {
      ...refreshed,
      state: "revoked",
      invitation: {
        ...refreshed.invitation,
        revision: 5,
        revoked_at_unix_ms: invitationCreatedAt + 120_000,
        revoked_by: "node-owner",
      },
    };
    const onSubmit = vi
      .fn<
        (
          action: NodeUiAction,
          values: ActionValues,
          onOperation: (operation: NodeOperationSnapshot) => void,
        ) => Promise<ActionSubmission>
      >()
      .mockResolvedValueOnce({
        outcome: {
          status: "invitations",
          data: [invitationStatus],
        } as NodeCommandOutcome,
      })
      .mockResolvedValueOnce({
        outcome: {
          status: "invitation_status",
          data: refreshed,
        } as NodeCommandOutcome,
      })
      .mockResolvedValueOnce({
        outcome: {
          status: "invitation_revoked",
          data: revoked,
        } as NodeCommandOutcome,
      });
    render(
      <ActionPanel
        actions={[listInvitations, getInvitationStatus, revokeInvitation]}
        snapshot={snapshot}
        selectedNetwork="home"
        locale="zh-CN"
        pendingAction={null}
        interactionBlocked={null}
        requestedAction={{
          actionId: "list_invitations",
          values: { network_id: invitationNetworkId },
        }}
        operation={null}
        onRequestedActionOpened={() => {}}
        onCancel={vi.fn(async () => {})}
        onSubmit={onSubmit}
      />,
    );

    await user.click(
      screen.getByRole("button", { name: "translated:common.execute" }),
    );
    const list = await screen.findByTestId("invitation-list");
    expect(list).toHaveTextContent("translated:invitations.state.active");
    expect(list).toHaveTextContent("translated:join_request.role.member");
    expect(list).toHaveTextContent("translated:capability.file");
    expect(list).toHaveTextContent("translated:invitations.privacy_boundary");
    expect(list).not.toHaveTextContent(invitationCode);
    expect(onSubmit.mock.calls[0]?.[1]).toEqual({
      network_id: invitationNetworkId,
    });

    await user.click(
      within(list).getByRole("button", {
        name: "translated:invitations.status_refresh",
      }),
    );
    await waitFor(() => expect(onSubmit).toHaveBeenCalledTimes(2));
    expect(onSubmit.mock.calls[1]?.[0]).toBe(getInvitationStatus);
    expect(onSubmit.mock.calls[1]?.[1]).toEqual({
      network_id: invitationNetworkId,
      invitation_id: invitationId,
    });
    await waitFor(() =>
      expect(screen.getByTestId("invitation-list")).toHaveTextContent(
        "translated:invitations.revision",
      ),
    );

    await user.click(
      within(screen.getByTestId("invitation-list")).getByRole("button", {
        name: "translated:invitations.revoke",
      }),
    );
    const confirmation = await screen.findByRole("dialog");
    expect(confirmation).toHaveTextContent(
      "translated:confirm.revoke_invitation.consequence",
    );
    expect(confirmation).toHaveTextContent(
      "translated:confirm.reauthentication_required",
    );
    await user.click(
      within(confirmation).getByRole("button", {
        name: "translated:common.execute",
      }),
    );

    await waitFor(() => expect(onSubmit).toHaveBeenCalledTimes(3));
    expect(onSubmit.mock.calls[2]?.[0]).toBe(revokeInvitation);
    expect(onSubmit.mock.calls[2]?.[1]).toEqual({
      network_id: invitationNetworkId,
      invitation_id: invitationId,
      expected_revision: "4",
    });
    expect(screen.getByTestId("invitation-list")).toHaveTextContent(
      "translated:invitations.state.revoked",
    );
    expect(screen.getByTestId("invitation-list")).not.toHaveTextContent(
      invitationCode,
    );
  });

  it("shows permission changes and submits the exact pinned preview for redemption", async () => {
    const user = userEvent.setup();
    const now = vi.spyOn(Date, "now").mockReturnValue(1_700_000_000_123);
    const onSubmit = vi
      .fn<
        (
          action: NodeUiAction,
          values: ActionValues,
          onOperation: (operation: NodeOperationSnapshot) => void,
        ) => Promise<ActionSubmission>
      >()
      .mockResolvedValueOnce({
        outcome: {
          status: "invitation_preview",
          data: invitationPreview,
        } as NodeCommandOutcome,
      })
      .mockResolvedValueOnce({
        outcome: {
          status: "invitation_redeemed",
          data: {
            invitation_id: invitationId,
            network_id: invitationNetworkId,
            node_id: "node-current",
            role: "member",
            capabilities: ["file"],
            network_revision: 8,
            invitation_revision: 3,
            joined_at_unix_ms: 1_700_000_000_123,
            already_redeemed: false,
            grants_permanent_membership: false,
          },
        } as NodeCommandOutcome,
      });
    render(
      <ActionPanel
        actions={[previewInvitation, redeemInvitation]}
        snapshot={snapshot}
        selectedNetwork="home"
        locale="zh-CN"
        pendingAction={null}
        interactionBlocked={null}
        requestedAction={{
          actionId: "preview_invitation",
          values: {
            network_id: invitationNetworkId,
            coordinator_url: "https://coordinator.example.test",
            code: invitationCode,
          },
        }}
        operation={null}
        onRequestedActionOpened={() => {}}
        onCancel={vi.fn(async () => {})}
        onSubmit={onSubmit}
      />,
    );

    await user.click(
      screen.getByRole("button", { name: "translated:common.execute" }),
    );
    const previewConfirmation = await screen.findByRole("dialog");
    await user.click(
      within(previewConfirmation).getByRole("button", {
        name: "translated:common.execute",
      }),
    );
    const preview = await screen.findByTestId("invitation-preview");
    expect(preview).toHaveTextContent("translated:invitations.can_accept");
    expect(preview).not.toHaveTextContent("translated:invitations.denied");
    expect(preview).toHaveTextContent("home");
    expect(preview).toHaveTextContent("translated:join_request.role.member");
    expect(preview).toHaveTextContent("File access is limited to shared folders");
    expect(preview).toHaveTextContent(
      "translated:invitations.reauthentication_required",
    );
    expect(preview).toHaveTextContent("translated:invitations.ordinary_membership");
    expect(preview).toHaveTextContent("translated:invitations.not_permanent");
    expect(document.body).not.toHaveTextContent(invitationCode);

    const checkbox = within(preview).getByRole("checkbox", {
      name: "translated:invitations.terms_checkbox",
    });
    const accept = within(preview).getByRole("button", {
      name: "translated:invitations.accept",
    });
    expect(accept).toBeDisabled();
    await user.click(checkbox);
    expect(accept).toBeEnabled();
    await user.click(accept);

    await waitFor(() => expect(onSubmit).toHaveBeenCalledTimes(2));
    expect(onSubmit.mock.calls[1]?.[0]).toBe(redeemInvitation);
    expect(onSubmit.mock.calls[1]?.[1]).toEqual({
      network_id: invitationNetworkId,
      coordinator_url: "https://coordinator.example.test",
      code: invitationCode,
      invitation_id: invitationId,
      expected_invitation_revision: "3",
      preview_digest: invitationPreview.preview_digest,
      terms_hash: invitationPreview.terms_hash,
      acknowledged_at_unix_ms: "1700000000123",
    });
    expect(
      await screen.findByText("translated:invitations.redeemed_notice"),
    ).toBeInTheDocument();
    expect(screen.queryByTestId("invitation-preview")).not.toBeInTheDocument();
    expect(document.body).not.toHaveTextContent(invitationCode);
    now.mockRestore();
  });

  it("requires a distinct Exit consent and blocks Required invitations when local Exit is unsupported", async () => {
    const requiredPreview: InvitationPreview = {
      ...invitationPreview,
      terms: {
        ...invitationPreview.terms,
        exit_contribution: "required",
        exit_contribution_scope: "all_members",
      },
    };
    const now = vi.spyOn(Date, "now").mockReturnValue(1_700_000_000_123);

    const renderRequiredPreview = async (
      preview: InvitationPreview,
      renderSnapshot: NodeSnapshot,
    ) => {
      const onSubmit = vi
        .fn<
          (
            action: NodeUiAction,
            values: ActionValues,
            onOperation: (operation: NodeOperationSnapshot) => void,
          ) => Promise<ActionSubmission>
        >()
        .mockResolvedValueOnce({
          outcome: {
            status: "invitation_preview",
            data: preview,
          } as NodeCommandOutcome,
        })
        .mockResolvedValueOnce({
          outcome: {
            status: "invitation_redeemed",
            data: {
              invitation_id: invitationId,
              network_id: invitationNetworkId,
              node_id: "node-current",
              role: "member",
              capabilities: ["file"],
              network_revision: 8,
              invitation_revision: 3,
              joined_at_unix_ms: 1_700_000_000_123,
              already_redeemed: false,
              grants_permanent_membership: false,
            },
          } as NodeCommandOutcome,
        });

      const view = render(
        <ActionPanel
          actions={[previewInvitation, redeemInvitation]}
          snapshot={renderSnapshot}
          selectedNetwork="home"
          locale="zh-CN"
          pendingAction={null}
          interactionBlocked={null}
          requestedAction={{
            actionId: "preview_invitation",
            values: {
              network_id: invitationNetworkId,
              coordinator_url: "https://coordinator.example.test",
              code: invitationCode,
            },
          }}
          operation={null}
          onRequestedActionOpened={() => {}}
          onCancel={vi.fn(async () => {})}
          onSubmit={onSubmit}
        />,
      );
      const user = userEvent.setup();
      await user.click(
        screen.getByRole("button", { name: "translated:common.execute" }),
      );
      const confirmation = await screen.findByRole("dialog");
      await user.click(
        within(confirmation).getByRole("button", {
          name: "translated:common.execute",
        }),
      );
      return { user, view, onSubmit, previewCard: await screen.findByTestId("invitation-preview") };
    };

    const supported = await renderRequiredPreview(requiredPreview, snapshot);
    const supportedCard = supported.previewCard;
    expect(supportedCard).toHaveTextContent("translated:join.exit.local_supported");
    expect(supportedCard).toHaveTextContent("translated:join.exit.local_enabled");
    expect(supportedCard).toHaveTextContent("translated:join.exit.cannot_join_without_local_exit");
    const terms = within(supportedCard).getByRole("checkbox", {
      name: "translated:invitations.terms_checkbox",
    });
    const exitConsent = within(supportedCard).getByRole("checkbox", {
      name: "translated:join.exit.consent",
    });
    const accept = within(supportedCard).getByRole("button", {
      name: "translated:invitations.accept",
    });
    expect(accept).toBeDisabled();
    await supported.user.click(terms);
    expect(accept).toBeDisabled();
    await supported.user.click(exitConsent);
    expect(accept).toBeEnabled();
    await supported.user.click(accept);
    await waitFor(() => expect(supported.onSubmit).toHaveBeenCalledTimes(2));
    expect(supported.onSubmit.mock.calls[1]?.[1]).toEqual({
      network_id: invitationNetworkId,
      coordinator_url: "https://coordinator.example.test",
      code: invitationCode,
      invitation_id: invitationId,
      expected_invitation_revision: "3",
      preview_digest: requiredPreview.preview_digest,
      terms_hash: requiredPreview.terms_hash,
      acknowledged_at_unix_ms: "1700000000123",
      exit_contribution_acknowledged: "true",
    });
    supported.view.unmount();

    const unsupportedSnapshot: NodeSnapshot = {
      ...snapshot,
      exit_node_capabilities: {
        ...snapshot.exit_node_capabilities,
        schema_version: 1,
        platform: "windows",
        supported: false,
        ipv4_forwarding: false,
        ipv6_forwarding: false,
        nat: false,
        cleanup: false,
        unsupported_reasons: ["platform_not_linux"],
      },
    };
    const unsupported = await renderRequiredPreview(
      requiredPreview,
      unsupportedSnapshot,
    );
    expect(unsupported.previewCard).toHaveTextContent(
      "translated:join.exit.local_unsupported",
    );
    expect(unsupported.previewCard).toHaveTextContent(
      "translated:join.exit.cannot_join_without_local_exit",
    );
    expect(
      within(unsupported.previewCard).getByRole("button", {
        name: "translated:invitations.accept",
      }),
    ).toBeDisabled();
    expect(
      within(unsupported.previewCard).getByRole("checkbox", {
        name: "translated:invitations.terms_checkbox",
      }),
    ).toBeDisabled();
    expect(unsupported.onSubmit).toHaveBeenCalledTimes(1);
    now.mockRestore();
  });

  const unacceptableInvitationPreviews: Array<{
    name: string;
    preview: InvitationPreview;
  }> = [
    {
      name: "expired",
      preview: {
        ...invitationPreview,
        state: "expired",
        denial_key: "invitations.state.expired",
      },
    },
    {
      name: "revoked",
      preview: {
        ...invitationPreview,
        state: "revoked",
        denial_key: "invitations.state.revoked",
      },
    },
    {
      name: "exhausted",
      preview: {
        ...invitationPreview,
        state: "exhausted",
        remaining_uses: 0,
        denial_key: "invitations.state.exhausted",
      },
    },
    {
      name: "permanent membership",
      preview: {
        ...invitationPreview,
        creates_permanent_membership: true,
      },
    },
  ];

  it.each(unacceptableInvitationPreviews)(
    "blocks confirmation for $name invitations even if the preview claims acceptance",
    async ({ preview: rejectedPreview }) => {
      const user = userEvent.setup();
      const now = vi.spyOn(Date, "now").mockReturnValue(1_700_000_000_123);
      const onSubmit = vi.fn<
        (
          action: NodeUiAction,
          values: ActionValues,
          onOperation: (operation: NodeOperationSnapshot) => void,
        ) => Promise<ActionSubmission>
      >(async () => ({
        outcome: {
          status: "invitation_preview",
          data: rejectedPreview,
        } as NodeCommandOutcome,
      }));
      render(
        <ActionPanel
          actions={[previewInvitation, redeemInvitation]}
          snapshot={snapshot}
          selectedNetwork="home"
          locale="zh-CN"
          pendingAction={null}
          interactionBlocked={null}
          requestedAction={{
            actionId: "preview_invitation",
            values: {
              network_id: invitationNetworkId,
              coordinator_url: "https://coordinator.example.test",
              code: invitationCode,
            },
          }}
          operation={null}
          onRequestedActionOpened={() => {}}
          onCancel={vi.fn(async () => {})}
          onSubmit={onSubmit}
        />,
      );

      await user.click(
        screen.getByRole("button", { name: "translated:common.execute" }),
      );
      const confirmation = await screen.findByRole("dialog");
      await user.click(
        within(confirmation).getByRole("button", {
          name: "translated:common.execute",
        }),
      );
      const preview = await screen.findByTestId("invitation-preview");
      expect(preview).toHaveTextContent("translated:invitations.cannot_accept");
      expect(
        within(preview).getByRole("checkbox", {
          name: "translated:invitations.terms_checkbox",
        }),
      ).toBeDisabled();
      expect(
        within(preview).getByRole("button", {
          name: "translated:invitations.accept",
        }),
      ).toBeDisabled();
      expect(onSubmit).toHaveBeenCalledOnce();

      await user.click(
        within(preview).getByRole("button", {
          name: "translated:invitations.preview_again",
        }),
      );
      await waitFor(() => expect(onSubmit).toHaveBeenCalledTimes(2));
      expect(onSubmit.mock.calls[1]?.[0]).toBe(previewInvitation);
      expect(onSubmit.mock.calls[1]?.[1]).toEqual({
        network_id: invitationNetworkId,
        coordinator_url: "https://coordinator.example.test",
        code: invitationCode,
      });

      await user.click(
        within(preview).getByRole("button", {
          name: "translated:invitations.back_to_edit",
        }),
      );
      expect(screen.queryByTestId("invitation-preview")).not.toBeInTheDocument();
      expect(
        screen.getByDisplayValue("https://coordinator.example.test"),
      ).toBeInTheDocument();
      expect(screen.getByDisplayValue(invitationCode)).toBeInTheDocument();
      now.mockRestore();
    },
  );
});

describe("action panel join request review list", () => {
  it("explains reason visibility and enforces its character limit", async () => {
    const user = userEvent.setup();
    render(
      <ActionPanel
        actions={[submitJoinRequest]}
        snapshot={snapshot}
        selectedNetwork="home"
        locale="zh-CN"
        pendingAction={null}
        interactionBlocked={null}
        requestedAction={null}
        operation={null}
        onRequestedActionOpened={() => {}}
        onCancel={vi.fn(async () => {})}
        onSubmit={vi.fn(async () => ({}))}
      />,
    );

    await user.click(
      screen.getByRole("button", {
        name: "translated:action.submit_join_request",
      }),
    );
    const reason = screen.getByPlaceholderText(
      "translated:placeholder.join_request_reason",
    ) as HTMLInputElement;
    expect(reason).toHaveAttribute("maxlength", "1000");
    const help = document.getElementById(
      reason.getAttribute("aria-describedby") ?? "",
    );
    expect(help).toHaveTextContent(
      "translated:join_requests.reason_input_help",
    );
    expect(help).toHaveTextContent(
      "translated:join_requests.reason_input_counter",
    );
    await user.type(reason, "家庭网络");
    expect(help).toHaveAttribute("data-character-count", "4");
  });

  it("shows applicant details, reason visibility and approval risks", async () => {
    const user = userEvent.setup();
    const onSubmit = vi.fn(async (): Promise<ActionSubmission> => ({
      outcome: joinRequestsOutcome as NodeCommandOutcome,
    }));
    const view = render(
      <ActionPanel
        actions={[listJoinRequests]}
        snapshot={snapshot}
        selectedNetwork="home"
        locale="zh-CN"
        pendingAction={null}
        interactionBlocked={null}
        requestedAction={null}
        operation={null}
        onRequestedActionOpened={() => {}}
        onCancel={vi.fn(async () => {})}
        onSubmit={onSubmit}
      />,
    );

    await user.click(
      screen.getByRole("button", {
        name: "translated:action.list_join_requests",
      }),
    );
    const network = screen.getByLabelText("translated:field.network_id");
    await user.clear(network);
    await user.type(network, "net_stable_home");
    await user.click(
      screen.getByRole("button", { name: "translated:common.execute" }),
    );

    const list = await screen.findByTestId("join-request-list");
    expect(list).toHaveTextContent("Branch office router");
    expect(list).toHaveTextContent(joinRequest.reason!);
    expect(list).toHaveTextContent("translated:join_requests.node_id");
    expect(list).toHaveTextContent(joinRequest.applicant.node_id);
    expect(list).toHaveTextContent(joinRequest.applicant.public_key_fingerprint);
    expect(list).toHaveTextContent(
      "translated:join_requests.reason_visibility.owners_and_administrators",
    );
    expect(within(list).getByTestId("join-request-risk")).toHaveTextContent(
      "translated:join_requests.exit.required",
    );
    expect(within(list).getByTestId("join-request-risk")).toHaveTextContent(
      "translated:join_requests.file_access.granted",
    );
    expect(within(list).getByTestId("join-request-risk")).toHaveTextContent(
      "translated:join_requests.data_plane.granted",
    );
    expect(within(list).getByTestId("join-request-risk")).toHaveTextContent(
      "translated:join_requests.expiring_soon",
    );

    view.rerender(
      <ActionPanel
        actions={[listJoinRequests]}
        snapshot={snapshot}
        selectedNetwork="studio"
        locale="zh-CN"
        pendingAction={null}
        interactionBlocked={null}
        requestedAction={null}
        operation={null}
        onRequestedActionOpened={() => {}}
        onCancel={vi.fn(async () => {})}
        onSubmit={onSubmit}
      />,
    );
    await waitFor(() => {
      expect(screen.queryByTestId("join-request-list")).not.toBeInTheDocument();
    });
  });

  it("renders an explicit empty review list", async () => {
    const user = userEvent.setup();
    render(
      <ActionPanel
        actions={[listJoinRequests]}
        snapshot={snapshot}
        selectedNetwork="home"
        locale="zh-CN"
        pendingAction={null}
        interactionBlocked={null}
        requestedAction={null}
        operation={null}
        onRequestedActionOpened={() => {}}
        onCancel={vi.fn(async () => {})}
        onSubmit={vi.fn(async (): Promise<ActionSubmission> => ({
          outcome: {
            status: "join_requests",
            data: { network_id: "net_stable_home", requests: [] },
          } as NodeCommandOutcome,
        }))}
      />,
    );

    await user.click(
      screen.getByRole("button", {
        name: "translated:action.list_join_requests",
      }),
    );
    const network = screen.getByLabelText("translated:field.network_id");
    await user.clear(network);
    await user.type(network, "net_stable_home");
    await user.click(
      screen.getByRole("button", { name: "translated:common.execute" }),
    );

    expect(await screen.findByTestId("join-request-empty")).toHaveTextContent(
      "translated:join_requests.empty",
    );
  });

  it("reviews one request with the pinned revision and inline approval controls", async () => {
    const user = userEvent.setup();
    const onSubmit = vi.fn(
      async (
        action: NodeUiAction,
        values: ActionValues,
      ): Promise<ActionSubmission> => {
        void values;
        if (action.id === "list_join_requests") {
          return { outcome: joinRequestsOutcome as NodeCommandOutcome };
        }
        return {
          outcome: {
            status: "join_requests_reviewed",
            data: {
              network_id: "net_stable_home",
              reviews: [
                {
                  request_id: joinRequest.request_id,
                  revision: joinRequest.revision + 1,
                  state: "approved",
                },
              ],
            },
          } as NodeCommandOutcome,
        };
      },
    );
    render(
      <ActionPanel
        actions={joinRequestReviewActions}
        snapshot={snapshot}
        selectedNetwork="home"
        locale="zh-CN"
        pendingAction={null}
        interactionBlocked={null}
        requestedAction={null}
        operation={null}
        onRequestedActionOpened={() => {}}
        onCancel={vi.fn(async () => {})}
        onSubmit={onSubmit}
      />,
    );

    await user.click(
      screen.getByRole("button", {
        name: "translated:action.list_join_requests",
      }),
    );
    const network = screen.getByLabelText("translated:field.network_id");
    await user.clear(network);
    await user.type(network, "net_stable_home");
    await user.click(
      screen.getByRole("button", { name: "translated:common.execute" }),
    );
    const list = await screen.findByTestId("join-request-list");
    expect(
      screen.queryByRole("button", {
        name: "translated:action.review_join_request",
      }),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByRole("button", {
        name: "translated:action.batch_review_join_requests",
      }),
    ).not.toBeInTheDocument();

    await user.click(
      within(list).getByRole("button", {
        name: "translated:choice.join_request_review.approve",
      }),
    );
    const dialog = await screen.findByRole("dialog");
    expect(dialog).toHaveTextContent(
      "translated:join_requests.review_dialog_title",
    );
    expect(dialog).toHaveTextContent("Branch office router");
    expect(dialog).toHaveTextContent("translated:join_requests.revision_pinned");
    const grantImpactSection = within(dialog).getByRole("region", {
      name: "translated:join_requests.grant_impact",
    });
    expect(grantImpactSection).toHaveTextContent(
      "translated:join_requests.grant_impact",
    );
    const grantImpact = within(grantImpactSection).getByTestId(
      "join-request-review-grant",
    );
    expect(grantImpact).toHaveTextContent("translated:join_requests.role");
    expect(grantImpact).toHaveTextContent("translated:join_request.role.member");
    expect(grantImpact).toHaveTextContent("translated:capability.relay");
    expect(grantImpact).toHaveTextContent("translated:capability.file");
    expect(grantImpact).toHaveTextContent("translated:join_requests.exit.required");
    expect(grantImpact).toHaveTextContent(
      "translated:join_requests.file_access.granted",
    );
    await user.click(
      within(dialog).getByRole("button", {
        name: "translated:join_requests.confirm_review",
      }),
    );

    await waitFor(() => expect(onSubmit).toHaveBeenCalledTimes(2));
    expect(onSubmit.mock.calls[1][0].id).toBe("review_join_request");
    expect(onSubmit.mock.calls[1][1]).toMatchObject({
      network_id: "net_stable_home",
      request_id: joinRequest.request_id,
      expected_revision: String(joinRequest.revision),
      action: "approve",
    });
    expect(onSubmit.mock.calls[1][1]).not.toHaveProperty("note");
    await waitFor(() => {
      expect(
        within(screen.getByTestId("join-request-list")).getByText(
          "translated:join_request.state.approved",
        ),
      ).toBeInTheDocument();
    });
  });

  it("requires a note before requesting more information", async () => {
    const user = userEvent.setup();
    const onSubmit = vi.fn(
      async (
        action: NodeUiAction,
        values: ActionValues,
      ): Promise<ActionSubmission> => {
        void values;
        if (action.id === "list_join_requests") {
          return { outcome: joinRequestsOutcome as NodeCommandOutcome };
        }
        return {
          outcome: {
            status: "join_requests_reviewed",
            data: {
              network_id: "net_stable_home",
              reviews: [
                {
                  request_id: joinRequest.request_id,
                  revision: joinRequest.revision + 1,
                  state: "needs_information",
                },
              ],
            },
          } as NodeCommandOutcome,
        };
      },
    );
    render(
      <ActionPanel
        actions={joinRequestReviewActions}
        snapshot={snapshot}
        selectedNetwork="home"
        locale="zh-CN"
        pendingAction={null}
        interactionBlocked={null}
        requestedAction={null}
        operation={null}
        onRequestedActionOpened={() => {}}
        onCancel={vi.fn(async () => {})}
        onSubmit={onSubmit}
      />,
    );

    await user.click(
      screen.getByRole("button", {
        name: "translated:action.list_join_requests",
      }),
    );
    const network = screen.getByLabelText("translated:field.network_id");
    await user.clear(network);
    await user.type(network, "net_stable_home");
    await user.click(
      screen.getByRole("button", { name: "translated:common.execute" }),
    );
    const list = await screen.findByTestId("join-request-list");
    await user.click(
      within(list).getByRole("button", {
        name: "translated:choice.join_request_review.request_information",
      }),
    );
    const dialog = await screen.findByRole("dialog");
    await user.click(
      within(dialog).getByRole("button", {
        name: "translated:join_requests.confirm_review",
      }),
    );
    expect(
      await within(dialog).findByText(
        "translated:join_request.error.information_required",
      ),
    ).toBeInTheDocument();
    expect(onSubmit).toHaveBeenCalledOnce();

    await user.type(
      within(dialog).getByRole("textbox", {
        name: "translated:field.join_request_note",
      }),
      "Please list the devices and file-sharing scope.",
    );
    await user.click(
      within(dialog).getByRole("button", {
        name: "translated:join_requests.confirm_review",
      }),
    );
    await waitFor(() => expect(onSubmit).toHaveBeenCalledTimes(2));
    expect(onSubmit.mock.calls[1][1]).toMatchObject({
      action: "request_information",
      note: "Please list the devices and file-sharing scope.",
    });
  });

  it("batch approves selected requests and updates their pinned revisions", async () => {
    const user = userEvent.setup();
    const secondRequest: JoinRequest = {
      ...joinRequest,
      request_id: `join_${"C".repeat(43)}`,
      applicant: {
        ...joinRequest.applicant,
        node_id: `node_${"D".repeat(43)}`,
        display_name: "Warehouse gateway",
      },
      revision: 5,
    };
    const batchListOutcome = {
      status: "join_requests" as const,
      data: {
        network_id: "net_stable_home",
        requests: [joinRequest, secondRequest],
      },
    };
    const onSubmit = vi.fn(
      async (
        action: NodeUiAction,
        values: ActionValues,
      ): Promise<ActionSubmission> => {
        if (action.id === "list_join_requests") {
          return { outcome: batchListOutcome as NodeCommandOutcome };
        }
        expect(action.id).toBe("batch_review_join_requests");
        const reviews = JSON.parse(String(values.reviews)) as Array<{
          request_id: string;
          revision?: number;
          expected_revision?: number;
          action?: string;
        }>;
        return {
          outcome: {
            status: "join_requests_reviewed",
            data: {
              network_id: "net_stable_home",
              reviews: reviews.map((review) => ({
                request_id: review.request_id,
                revision: (review.expected_revision ?? 0) + 1,
                state: "approved",
              })),
            },
          } as NodeCommandOutcome,
        };
      },
    );
    render(
      <ActionPanel
        actions={joinRequestReviewActions}
        snapshot={snapshot}
        selectedNetwork="home"
        locale="zh-CN"
        pendingAction={null}
        interactionBlocked={null}
        requestedAction={null}
        operation={null}
        onRequestedActionOpened={() => {}}
        onCancel={vi.fn(async () => {})}
        onSubmit={onSubmit}
      />,
    );

    await user.click(
      screen.getByRole("button", {
        name: "translated:action.list_join_requests",
      }),
    );
    const network = screen.getByLabelText("translated:field.network_id");
    await user.clear(network);
    await user.type(network, "net_stable_home");
    await user.click(
      screen.getByRole("button", { name: "translated:common.execute" }),
    );
    const list = await screen.findByTestId("join-request-list");
    await user.click(
      screen.getByRole("checkbox", {
        name: "translated:join_requests.select_all",
      }),
    );
    expect(screen.getByTestId("join-request-selection-bar")).toHaveTextContent(
      "translated:join_requests.selected_count",
    );
    await user.click(
      screen.getByRole("button", {
        name: "translated:join_requests.batch.approve",
      }),
    );
    const dialog = await screen.findByRole("dialog");
    expect(dialog).toHaveTextContent(
      "translated:join_requests.batch_dialog_title",
    );
    expect(dialog).toHaveTextContent("Warehouse gateway");
    expect(
      within(dialog).getAllByTestId("join-request-review-grant"),
    ).toHaveLength(2);
    await user.click(
      within(dialog).getByRole("button", {
        name: "translated:join_requests.confirm_review",
      }),
    );

    await waitFor(() => expect(onSubmit).toHaveBeenCalledTimes(2));
    expect(onSubmit.mock.calls[1][0].id).toBe("batch_review_join_requests");
    expect(JSON.parse(String(onSubmit.mock.calls[1][1].reviews))).toEqual([
      {
        request_id: joinRequest.request_id,
        expected_revision: joinRequest.revision,
        action: "approve",
      },
      {
        request_id: secondRequest.request_id,
        expected_revision: secondRequest.revision,
        action: "approve",
      },
    ]);
    await waitFor(() => {
      expect(
        within(list).getAllByText("translated:join_request.state.approved"),
      ).toHaveLength(2);
    });
    expect(
      screen.queryByTestId("join-request-selection-bar"),
    ).not.toBeInTheDocument();
  });
});


describe("action panel network member directory", () => {
  it("keeps the action expanded and renders the authoritative member directory", async () => {
    const user = userEvent.setup();
    const onSubmit = vi.fn(async (): Promise<ActionSubmission> => ({
      outcome: {
        status: "network_members",
        data: memberDirectory,
      },
    }));
    render(
      <ActionPanel
        actions={[listNetworkMembers]}
        snapshot={snapshot}
        selectedNetwork="home"
        locale="zh-CN"
        pendingAction={null}
        interactionBlocked={null}
        requestedAction={null}
        operation={null}
        onRequestedActionOpened={() => {}}
        onCancel={vi.fn(async () => {})}
        onSubmit={onSubmit}
      />,
    );

    const actionHead = screen.getByRole("button", {
      name: "translated:action.list_network_members",
    });
    await user.click(actionHead);
    await user.type(
      screen.getByLabelText("translated:field.network_id"),
      "net_stable_home",
    );
    await user.click(
      screen.getByRole("button", { name: "translated:common.execute" }),
    );

    expect(
      await screen.findByText("translated:member_directory.title"),
    ).toBeInTheDocument();
    expect(actionHead).toHaveAttribute("aria-expanded", "true");
    expect(
      screen.getAllByText("translated:member_directory.privacy_boundary"),
    ).toHaveLength(2);
  });

  it("shows the explicit permission boundary and no member data when rejected", async () => {
    const user = userEvent.setup();
    render(
      <ActionPanel
        actions={[listNetworkMembers]}
        snapshot={snapshot}
        selectedNetwork="home"
        locale="zh-CN"
        pendingAction={null}
        interactionBlocked={null}
        requestedAction={null}
        operation={null}
        onRequestedActionOpened={() => {}}
        onCancel={vi.fn(async () => {})}
        onSubmit={vi.fn(async (): Promise<ActionSubmission> => ({
          outcome: {
            status: "rejected",
            data: {
              category: "authorization",
              code: "permission_denied",
              message: "permission denied",
              message_key: "member_directory.error.permission_denied",
              retryable: false,
            },
          } as NodeCommandOutcome,
        }))}
      />,
    );

    await user.click(
      screen.getByRole("button", {
        name: "translated:action.list_network_members",
      }),
    );
    await user.type(
      screen.getByLabelText("translated:field.network_id"),
      "net_stable_home",
    );
    await user.click(
      screen.getByRole("button", { name: "translated:common.execute" }),
    );

    expect(await screen.findByRole("alert")).toHaveTextContent(
      "translated:member_directory.error.permission_denied",
    );
    expect(
      screen.queryByText("translated:member_directory.title"),
    ).not.toBeInTheDocument();
    expect(screen.queryByTestId("member-list")).not.toBeInTheDocument();
  });

  it("clears a previously loaded directory when the network changes", async () => {
    const user = userEvent.setup();
    const onSubmit = vi.fn(async (): Promise<ActionSubmission> => ({
      outcome: {
        status: "network_members",
        data: memberDirectory,
      },
    }));
    const view = render(
      <ActionPanel
        actions={[listNetworkMembers]}
        snapshot={snapshot}
        selectedNetwork="home"
        locale="zh-CN"
        pendingAction={null}
        interactionBlocked={null}
        requestedAction={null}
        operation={null}
        onRequestedActionOpened={() => {}}
        onCancel={vi.fn(async () => {})}
        onSubmit={onSubmit}
      />,
    );

    await user.click(
      screen.getByRole("button", {
        name: "translated:action.list_network_members",
      }),
    );
    await user.type(
      screen.getByLabelText("translated:field.network_id"),
      "net_stable_home",
    );
    await user.click(
      screen.getByRole("button", { name: "translated:common.execute" }),
    );
    expect(
      await screen.findByText("translated:member_directory.title"),
    ).toBeInTheDocument();

    view.rerender(
      <ActionPanel
        actions={[listNetworkMembers]}
        snapshot={snapshot}
        selectedNetwork="studio"
        locale="zh-CN"
        pendingAction={null}
        interactionBlocked={null}
        requestedAction={null}
        operation={null}
        onRequestedActionOpened={() => {}}
        onCancel={vi.fn(async () => {})}
        onSubmit={onSubmit}
      />,
    );

    await waitFor(() => {
      expect(
        screen.queryByText("translated:member_directory.title"),
      ).not.toBeInTheDocument();
    });
  });
});


describe("action panel member governance", () => {
  const originalClientWidth = Object.getOwnPropertyDescriptor(
    Element.prototype,
    "clientWidth",
  );
  const originalClientHeight = Object.getOwnPropertyDescriptor(
    Element.prototype,
    "clientHeight",
  );

  afterEach(() => {
    if (originalClientWidth) {
      Object.defineProperty(
        Element.prototype,
        "clientWidth",
        originalClientWidth,
      );
    }
    if (originalClientHeight) {
      Object.defineProperty(
        Element.prototype,
        "clientHeight",
        originalClientHeight,
      );
    }
  });

  const governedNodeId = `node_${"A".repeat(43)}`;
  const previewMemberRemoval = manifest.pages
    .flatMap((page) => page.actions)
    .find((action) => action.id === "preview_member_removal") as NodeUiAction;
  const changeMemberLifecycle = manifest.pages
    .flatMap((page) => page.actions)
    .find((action) => action.id === "change_member_lifecycle") as NodeUiAction;

  function governedDirectory(
    lifecycle: "active" | "removed" = "active",
  ): NetworkMemberDirectory {
    return {
      network_id: "net_stable_home",
      generated_at_unix_ms: 1_700_000_100_000,
      viewer_role: "owner",
      can_manage_members: true,
      members: [
        {
          node_id: governedNodeId,
          display_name: "Governed member",
          identity_fingerprint: currentFingerprint,
          role: "member",
          groups: ["member"],
          capabilities: ["file"],
          status: lifecycle === "active" ? "active" : "removed",
          lifecycle_state: lifecycle,
          revision: lifecycle === "active" ? 3 : 4,
          access_epoch: lifecycle === "active" ? 2 : 3,
          available_actions: lifecycle === "active" ? ["remove", "suspend"] : [],
          presence: {
            state: "online",
            source: "live_session",
            observed_at_unix_ms: 1_700_000_095_000,
          },
          joined_at_unix_ms: 1_700_000_000_000,
        },
      ],
    };
  }

  const removalPreview: MemberRemovalPreview = {
    network_id: "net_stable_home",
    target_node_id: governedNodeId,
    role: "member",
    status: "active",
    lifecycle_state: "active",
    revision: 3,
    access_epoch: 2,
    presence: {
      state: "online",
      source: "live_session",
      observed_at_unix_ms: 1_700_000_095_000,
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
  };

  const lifecycleChange: MemberLifecycleChange = {
    network_id: "net_stable_home",
    target_node_id: governedNodeId,
    action: "remove",
    status: "removed",
    revision: 4,
    access_epoch: 3,
    access_revoked: true,
    requires_reapproval: true,
    requires_reauthentication: true,
    idempotent: false,
    operation_id: "op_lifecycle",
    audit_id: "audit_lifecycle",
  };

  it("governs members from the authoritative directory and refreshes it after a change", async () => {
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
    const user = userEvent.setup();
    let directoryLoads = 0;
    const onSubmit = vi.fn(
      async (
        submitted: NodeUiAction,
        values: ActionValues,
      ): Promise<ActionSubmission> => {
        void values;
        if (submitted.id === "list_network_members") {
          directoryLoads += 1;
          return {
            outcome: {
              status: "network_members",
              data: governedDirectory(
                directoryLoads === 1 ? "active" : "removed",
              ),
            },
          };
        }
        if (submitted.id === "preview_member_removal") {
          return {
            outcome: { status: "member_removal_preview", data: removalPreview },
          };
        }
        return {
          outcome: { status: "member_lifecycle_changed", data: lifecycleChange },
        };
      },
    );
    render(
      <ActionPanel
        actions={[listNetworkMembers, previewMemberRemoval, changeMemberLifecycle]}
        snapshot={snapshot}
        selectedNetwork="home"
        locale="en"
        pendingAction={null}
        interactionBlocked={null}
        requestedAction={null}
        operation={null}
        onRequestedActionOpened={() => {}}
        onCancel={vi.fn(async () => {})}
        onSubmit={onSubmit}
      />,
    );

    const actionHead = screen.getByRole("button", {
      name: "translated:action.list_network_members",
    });
    await user.click(actionHead);
    await user.type(
      screen.getByLabelText("translated:field.network_id"),
      "net_stable_home",
    );
    await user.click(
      screen.getByRole("button", { name: "translated:common.execute" }),
    );

    await user.click(
      await screen.findByRole("option", { name: /Governed member/ }),
    );
    await user.click(screen.getByTestId("member-governance-action-remove"));
    const impact = await screen.findByTestId("member-removal-impact");
    expect(impact).toHaveTextContent("translated:member_removal.impact.credentials");
    await user.click(screen.getByTestId("member-governance-confirm"));

    await waitFor(() => expect(onSubmit).toHaveBeenCalledTimes(4));
    const lifecycleCall = onSubmit.mock.calls[2];
    expect(lifecycleCall[0].id).toBe("change_member_lifecycle");
    expect(lifecycleCall[1]).toMatchObject({
      network_id: "net_stable_home",
      target_node_id: governedNodeId,
      action: "remove",
      role: "member",
      expected_revision: "3",
    });
    const refreshCall = onSubmit.mock.calls[3];
    expect(refreshCall[0].id).toBe("list_network_members");
    expect(refreshCall[1]).toEqual({ network_id: "net_stable_home" });

    expect(await screen.findByTestId("member-detail")).toHaveTextContent(
      "translated:member_directory.lifecycle_state.removed",
    );
    expect(screen.getByTestId("member-detail")).toHaveTextContent("4");
    expect(screen.getByText("translated:outcome.network_members_loaded")).toBeVisible();
    expect(actionHead).toHaveAttribute("aria-expanded", "true");
  });

  it("keeps a standalone removal preview open with the full impact summary", async () => {
    const user = userEvent.setup();
    const onSubmit = vi.fn(
      async (
        submitted: NodeUiAction,
        values: ActionValues,
      ): Promise<ActionSubmission> => {
        void submitted;
        void values;
        return {
          outcome: { status: "member_removal_preview", data: removalPreview },
        };
      },
    );
    render(
      <ActionPanel
        actions={[previewMemberRemoval]}
        snapshot={snapshot}
        selectedNetwork="home"
        locale="en"
        pendingAction={null}
        interactionBlocked={null}
        requestedAction={null}
        operation={null}
        onRequestedActionOpened={() => {}}
        onCancel={vi.fn(async () => {})}
        onSubmit={onSubmit}
      />,
    );

    const actionHead = screen.getByRole("button", {
      name: "translated:action.preview_member_removal",
    });
    await user.click(actionHead);
    await user.type(
      screen.getByLabelText("translated:field.network_id"),
      "net_stable_home",
    );
    await user.type(
      screen.getByLabelText("translated:field.target_node_id"),
      governedNodeId,
    );
    await user.click(
      screen.getByRole("button", { name: "translated:common.execute" }),
    );

    expect(
      await screen.findByText("translated:outcome.member_removal_previewed"),
    ).toBeInTheDocument();
    expect(actionHead).toHaveAttribute("aria-expanded", "true");
    expect(onSubmit.mock.calls[0][1]).toEqual({
      network_id: "net_stable_home",
      target_node_id: governedNodeId,
    });
  });

  it("keeps the lifecycle card open after a confirmed change", async () => {
    const user = userEvent.setup();
    const onSubmit = vi.fn(
      async (
        submitted: NodeUiAction,
        values: ActionValues,
      ): Promise<ActionSubmission> => {
        void submitted;
        void values;
        return {
          outcome: {
            status: "member_lifecycle_changed",
            data: { ...lifecycleChange, action: "suspend", status: "suspended" },
          },
        };
      },
    );
    render(
      <ActionPanel
        actions={[changeMemberLifecycle]}
        snapshot={snapshot}
        selectedNetwork="home"
        locale="en"
        pendingAction={null}
        interactionBlocked={null}
        requestedAction={null}
        operation={null}
        onRequestedActionOpened={() => {}}
        onCancel={vi.fn(async () => {})}
        onSubmit={onSubmit}
      />,
    );

    const actionHead = screen.getByRole("button", {
      name: "translated:action.change_member_lifecycle",
    });
    await user.click(actionHead);
    await user.type(
      screen.getByLabelText("translated:field.network_id"),
      "net_stable_home",
    );
    await user.type(
      screen.getByLabelText("translated:field.target_node_id"),
      governedNodeId,
    );
    await user.selectOptions(
      screen.getByLabelText("translated:field.member_lifecycle_action"),
      "suspend",
    );
    await user.selectOptions(
      screen.getByLabelText("translated:field.membership_role"),
      "member",
    );
    await user.type(
      screen.getByLabelText("translated:field.expected_revision"),
      "3",
    );
    await user.click(
      screen.getByRole("button", { name: "translated:common.execute" }),
    );
    const dialog = await screen.findByRole("dialog");
    await user.click(
      within(dialog).getByRole("button", {
        name: "translated:common.execute",
      }),
    );

    expect(
      await screen.findByText("translated:outcome.member_lifecycle_changed"),
    ).toBeInTheDocument();
    expect(actionHead).toHaveAttribute("aria-expanded", "true");
    expect(onSubmit).toHaveBeenCalledTimes(1);
    expect(onSubmit.mock.calls[0][1]).toMatchObject({
      action: "suspend",
      expected_revision: "3",
      role: "member",
    });
  });
});
