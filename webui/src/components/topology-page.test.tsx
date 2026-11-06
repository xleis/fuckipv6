import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import manifestFixture from "../../../contracts/typescript/fixtures/node-manifest.json";
import type {
  MemberLifecycleAction,
  NetworkMember,
  NetworkMemberDirectory,
  NodeUiAction,
  NodeUiManifest,
  TopologySnapshot,
} from "../../../contracts/typescript/node-contract-v1";
import type { TopologyActionSubmit } from "../topology-actions";
import { installTestI18nRuntime } from "../test/runtime";
import { TopologyPage } from "./topology-page";

const translated = (key: string) => `translated:${key}`;
const BOB_ID = `node_${"B".repeat(43)}`;
const CAROL_ID = `node_${"C".repeat(43)}`;
const manifest = manifestFixture as NodeUiManifest;
const governanceActions = manifest.pages
  .flatMap((page) => page.actions)
  .filter((action) =>
    new Set([
      "list_network_members",
      "preview_member_removal",
      "change_member_lifecycle",
    ]).has(action.id),
  ) as NodeUiAction[];

function member(
  nodeId: string,
  displayName: string,
  availableActions: MemberLifecycleAction[],
  revision: number,
): NetworkMember {
  return {
    node_id: nodeId,
    display_name: displayName,
    identity_fingerprint: `fingerprint-${nodeId}`,
    role: "member",
    groups: ["member"],
    capabilities: nodeId === BOB_ID ? ["exit"] : ["relay"],
    status: nodeId === BOB_ID ? "degraded" : "suspended",
    lifecycle_state: nodeId === BOB_ID ? "degraded" : "suspended",
    revision,
    access_epoch: 4,
    available_actions: availableActions,
    presence:
      nodeId === BOB_ID
        ? {
            state: "offline",
            source: "disconnect_observed",
            observed_at_unix_ms: 1_700_000_000_000,
          }
        : {
            state: "unknown",
            source: "not_observed",
            observed_at_unix_ms: null,
          },
  };
}

function directory(
  canManageMembers = true,
  viewerRole: NetworkMemberDirectory["viewer_role"] = "owner",
): NetworkMemberDirectory {
  return {
    network_id: "net-alpha",
    generated_at_unix_ms: 1_700_000_100_000,
    viewer_role: viewerRole,
    can_manage_members: canManageMembers,
    members: [
      member(BOB_ID, "Bob", ["suspend", "remove"], 7),
      member(CAROL_ID, "Carol", ["suspend", "remove"], 9),
    ],
  };
}

function snapshot(revision = 3, includeBob = true): TopologySnapshot {
  const nodes = [
    ...(includeBob
      ? [
          {
            node_id: BOB_ID,
            display_name: "Bob",
            health: "degraded" as const,
            is_self: false,
            capabilities: ["exit" as const],
            networks: [
              {
                network_id: "net-alpha",
                health: "degraded" as const,
                presence: {
                  state: "offline" as const,
                  source: "disconnect_observed" as const,
                },
                role: "member" as const,
                status: "degraded" as const,
              },
            ],
          },
        ]
      : []),
    {
      node_id: CAROL_ID,
      display_name: "Carol",
      health: "unreachable" as const,
      is_self: false,
      capabilities: ["relay" as const],
      networks: [
        {
          network_id: "net-alpha",
          health: "unreachable" as const,
          presence: {
            state: "unknown" as const,
            source: "not_observed" as const,
          },
          role: "member" as const,
          status: "suspended" as const,
        },
      ],
    },
  ];
  return {
    revision,
    viewer: "node-self",
    generated_at_unix_ms: 1_700_000_000_000 + revision,
    truncated: false,
    boundaries: [
      {
        network_id: "net-alpha",
        name: "Alpha",
        member_count: nodes.length,
        visible_member_count: nodes.length,
        viewer_role: "owner",
      },
    ],
    nodes,
    connections: [],
  };
}

function directorySubmit(
  data: NetworkMemberDirectory,
  lifecycle?: (
    values: Record<string, unknown>,
  ) => Awaited<ReturnType<TopologyActionSubmit>>,
): TopologyActionSubmit {
  return async (action, values) => {
    if (action.id === "list_network_members") {
      return {
        outcome: { status: "network_members", data },
      };
    }
    if (lifecycle && action.id === "change_member_lifecycle") {
      return lifecycle(values);
    }
    return {};
  };
}

async function selectRows(user: ReturnType<typeof userEvent.setup>, names: string[]) {
  await user.click(
    screen.getByRole("button", { name: translated("topology.view.list") }),
  );
  const table = screen.getByRole("table");
  for (const name of names) {
    const row = [...table.querySelectorAll<HTMLTableRowElement>("tbody tr")].find(
      (candidate) => candidate.textContent?.includes(name),
    );
    expect(row).toBeDefined();
    await user.click(within(row as HTMLElement).getByRole("checkbox"));
  }
}

beforeEach(() => {
  installTestI18nRuntime();
  Object.defineProperty(Element.prototype, "scrollIntoView", {
    configurable: true,
    value: vi.fn(),
  });
  vi.spyOn(Element.prototype, "getBoundingClientRect").mockReturnValue({
    x: 0,
    y: 0,
    top: 0,
    left: 0,
    right: 120,
    bottom: 120,
    width: 120,
    height: 120,
    toJSON: () => ({}),
  });
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

describe("TopologyPage interactions and governance", () => {
  it("loads the member directory once across equivalent callback and manifest refreshes", async () => {
    const firstSubmit = vi.fn(directorySubmit(directory()));
    const { rerender } = render(
      <TopologyPage
        topology={snapshot()}
        locale="zh-CN"
        actions={governanceActions}
        onSubmitAction={firstSubmit}
      />,
    );

    await waitFor(() => expect(firstSubmit).toHaveBeenCalledTimes(1));
    rerender(
      <TopologyPage
        topology={snapshot()}
        locale="zh-CN"
        actions={[]}
        onSubmitAction={firstSubmit}
      />,
    );
    const secondSubmit = vi.fn(directorySubmit(directory()));
    rerender(
      <TopologyPage
        topology={snapshot()}
        locale="zh-CN"
        actions={governanceActions.map((action) => ({ ...action }))}
        onSubmitAction={secondSubmit}
      />,
    );
    await new Promise((resolve) => setTimeout(resolve, 50));

    expect(firstSubmit).toHaveBeenCalledTimes(1);
    expect(secondSubmit).not.toHaveBeenCalled();
  });

  it("shows role, Exit capability, degraded health and presence on hover", async () => {
    const user = userEvent.setup();
    const onSubmit = vi.fn(directorySubmit(directory()));
    render(
      <TopologyPage
        topology={snapshot()}
        locale="zh-CN"
        actions={governanceActions}
        onSubmitAction={onSubmit}
      />,
    );

    await user.hover(screen.getByRole("option", { name: /Bob/ }));
    const tooltip = await screen.findByTestId("topology-node-hover");
    expect(tooltip).toHaveTextContent("Bob");
    expect(tooltip).toHaveTextContent(translated("join_request.role.member"));
    expect(tooltip).toHaveTextContent(translated("capability.exit"));
    expect(tooltip).toHaveTextContent(translated("topology.health.degraded"));
    expect(tooltip).toHaveTextContent(
      translated("member_directory.presence.offline"),
    );
  });

  it("opens the same action menu from right click and Shift+F10", async () => {
    const user = userEvent.setup();
    const onSubmit = vi.fn(directorySubmit(directory()));
    render(
      <TopologyPage
        topology={snapshot()}
        locale="zh-CN"
        actions={governanceActions}
        onSubmitAction={onSubmit}
      />,
    );
    const bob = screen.getByRole("option", { name: /Bob/ });

    fireEvent.contextMenu(bob, { clientX: 40, clientY: 50 });
    const menu = await screen.findByRole("menu");
    expect(menu).toHaveTextContent("Bob");
    const suspend = await within(menu).findByRole("menuitem", {
      name: translated("choice.member_lifecycle.suspend"),
    });
    await user.click(suspend);
    expect(screen.getByText(translated("topology.selection.count"))).toBeInTheDocument();

    fireEvent.keyDown(bob, { key: "F10", shiftKey: true });
    expect(await screen.findByRole("menu")).toBeInTheDocument();
  });

  it("clears selection with an explicit notice when filters change", async () => {
    const user = userEvent.setup();
    const onSubmit = vi.fn(directorySubmit(directory()));
    render(
      <TopologyPage
        topology={snapshot()}
        locale="zh-CN"
        actions={governanceActions}
        onSubmitAction={onSubmit}
      />,
    );
    await selectRows(user, ["Bob"]);
    expect(screen.getByText(translated("topology.selection.count"))).toBeInTheDocument();

    await user.type(
      screen.getByLabelText(translated("topology.filters.search")),
      "zzz",
    );
    expect(
      await screen.findByText(translated("topology.selection.filter_changed")),
    ).toBeInTheDocument();
    expect(
      screen.queryByText(translated("topology.selection.count")),
    ).not.toBeInTheDocument();
  });

  it("retains surviving selections and permanently drops objects after refresh", async () => {
    const onSubmit = vi.fn(directorySubmit(directory()));
    const user = userEvent.setup();
    const { container, rerender } = render(
      <TopologyPage
        topology={snapshot(3)}
        locale="zh-CN"
        actions={governanceActions}
        onSubmitAction={onSubmit}
      />,
    );
    await selectRows(user, ["Bob"]);
    expect(container.querySelector(`[data-node-id="${BOB_ID}"]`)).toHaveAttribute(
      "data-selected",
      "true",
    );

    rerender(
      <TopologyPage
        topology={snapshot(4)}
        locale="zh-CN"
        actions={governanceActions}
        onSubmitAction={onSubmit}
      />,
    );
    await waitFor(() =>
      expect(container.querySelector(`[data-node-id="${BOB_ID}"]`)).toHaveAttribute(
        "data-selected",
        "true",
      ),
    );

    rerender(
      <TopologyPage
        topology={snapshot(5, false)}
        locale="zh-CN"
        actions={governanceActions}
        onSubmitAction={onSubmit}
      />,
    );
    await waitFor(() =>
      expect(container.querySelector(`[data-node-id="${BOB_ID}"]`)).not.toBeInTheDocument(),
    );

    rerender(
      <TopologyPage
        topology={snapshot(6)}
        locale="zh-CN"
        actions={governanceActions}
        onSubmitAction={onSubmit}
      />,
    );
    await waitFor(() =>
      expect(container.querySelector(`[data-node-id="${BOB_ID}"]`)).toHaveAttribute(
        "data-selected",
        "false",
      ),
    );
  });

  it("exposes no lifecycle action when the directory denies management", async () => {
    const user = userEvent.setup();
    let lifecycleCalls = 0;
    const onSubmit = vi.fn(directorySubmit(directory(false)));
    const wrappedSubmit: TopologyActionSubmit = async (...args) => {
      if (args[0].id === "change_member_lifecycle") lifecycleCalls += 1;
      return onSubmit(...args);
    };
    render(
      <TopologyPage
        topology={snapshot()}
        locale="zh-CN"
        actions={governanceActions}
        onSubmitAction={wrappedSubmit}
      />,
    );

    fireEvent.contextMenu(screen.getByRole("option", { name: /Bob/ }));
    const menu = await screen.findByRole("menu");
    expect(
      await within(menu).findByRole("menuitem", {
        name: translated("topology.menu.no_actions"),
      }),
    ).toHaveAttribute("aria-disabled", "true");

    fireEvent.keyDown(document, { key: "Escape" });
    await selectRows(user, ["Bob"]);
    const actionSelect = await screen.findByLabelText(
      translated("topology.batch.action"),
    );
    expect(actionSelect).toBeDisabled();
    expect(
      screen.getByRole("button", { name: translated("topology.batch.preview") }),
    ).toBeDisabled();
    expect(lifecycleCalls).toBe(0);
  });

  it("treats a server removal denial as a per-item skip and never submits lifecycle", async () => {
    const user = userEvent.setup();
    const onSubmit = vi.fn(
      async (action: NodeUiAction, values: Record<string, unknown>) => {
        if (action.id === "list_network_members") {
          return { outcome: { status: "network_members" as const, data: directory() } };
        }
        if (action.id === "preview_member_removal") {
          return {
            outcome: {
              status: "member_removal_preview" as const,
              data: {
                network_id: "net-alpha",
                target_node_id: values.target_node_id as string,
                role: "member" as const,
                status: "degraded" as const,
                lifecycle_state: "degraded" as const,
                revision: 7,
                access_epoch: 4,
                presence: {
                  state: "offline" as const,
                  source: "disconnect_observed" as const,
                },
                can_remove: false,
                denial_key: "member_lifecycle.error.owner_protected",
                network_access_revoked: false,
                file_access_revoked: false,
                incomplete_transfers_resume_blocked: false,
                credentials_revoked: false,
                local_files_deleted: false,
                old_access_epoch_invalidated: false,
                safe_undo_requires_reapproval: true,
                safe_undo_restores_old_credentials: false,
                impact_keys: ["member_removal.impact.network_access"],
              },
            },
          };
        }
        return {};
      },
    );
    render(
      <TopologyPage
        topology={snapshot()}
        locale="zh-CN"
        actions={governanceActions}
        onSubmitAction={onSubmit}
      />,
    );
    await selectRows(user, ["Bob", "Carol"]);
    await waitFor(() =>
      expect(
        screen.getByRole("button", { name: translated("topology.batch.preview") }),
      ).toBeEnabled(),
    );
    await user.selectOptions(
      screen.getByLabelText(translated("topology.batch.action")),
      "remove",
    );
    await user.click(
      screen.getByRole("button", { name: translated("topology.batch.preview") }),
    );
    await user.click(
      within(await screen.findByTestId("topology-batch-dialog")).getByRole(
        "button",
        { name: translated("topology.batch.execute") },
      ),
    );

    const results = await screen.findByTestId("topology-batch-results");
    expect(within(results).getAllByText(/topology\.batch\.remove_blocked/)).toHaveLength(2);
    expect(
      onSubmit.mock.calls.filter(([action]) => action.id === "change_member_lifecycle"),
    ).toHaveLength(0);
  });

  it("reports mixed success and rejection item by item with the real audit ID", async () => {
    const user = userEvent.setup();
    const onSubmit = vi.fn(
      async (action: NodeUiAction, values: Record<string, unknown>) => {
        if (action.id === "list_network_members") {
          return { outcome: { status: "network_members" as const, data: directory() } };
        }
        if (values.target_node_id === BOB_ID) {
          return {
            outcome: {
              status: "member_lifecycle_changed" as const,
              data: {
                network_id: "net-alpha",
                target_node_id: BOB_ID,
                action: "suspend" as const,
                status: "suspended" as const,
                revision: 8,
                access_epoch: 5,
                access_revoked: true,
                requires_reapproval: true,
                requires_reauthentication: true,
                idempotent: false,
                operation_id: "op-bob",
                audit_id: "audit-bob-42",
              },
            },
          };
        }
        return {
          outcome: {
            status: "rejected" as const,
            data: {
              category: "authorization" as const,
              code: "permission_denied" as const,
              message: "denied",
              message_key: "member_lifecycle.error.permission_denied",
              retryable: false,
            },
          },
        };
      },
    );
    render(
      <TopologyPage
        topology={snapshot()}
        locale="zh-CN"
        actions={governanceActions}
        onSubmitAction={onSubmit}
      />,
    );
    await selectRows(user, ["Bob", "Carol"]);
    await waitFor(() =>
      expect(
        screen.getByRole("button", { name: translated("topology.batch.preview") }),
      ).toBeEnabled(),
    );
    await user.click(
      screen.getByRole("button", { name: translated("topology.batch.preview") }),
    );
    const dialog = await screen.findByTestId("topology-batch-dialog");
    expect(dialog).toHaveTextContent(translated("topology.batch.reauthentication"));
    expect(dialog).toHaveTextContent(translated("confirm.member_lifecycle.suspend"));
    await user.click(
      within(dialog).getByRole("button", {
        name: translated("topology.batch.execute"),
      }),
    );

    const results = await screen.findByTestId("topology-batch-results");
    const bob = within(results).getByTestId(`topology-batch-result-${BOB_ID}`);
    const carol = within(results).getByTestId(`topology-batch-result-${CAROL_ID}`);
    expect(bob).toHaveTextContent(translated("topology.batch.result.success"));
    expect(bob).toHaveTextContent("audit-bob-42");
    expect(carol).toHaveTextContent(translated("topology.batch.result.failed"));
    expect(carol).not.toHaveTextContent("audit-bob-42");
    expect(results).toHaveTextContent(translated("topology.batch.result.summary"));

    const lifecycleCalls = onSubmit.mock.calls.filter(
      ([action]) => action.id === "change_member_lifecycle",
    );
    expect(lifecycleCalls).toHaveLength(2);
    expect(lifecycleCalls[0][1]).toMatchObject({
      network_id: "net-alpha",
      target_node_id: BOB_ID,
      action: "suspend",
      expected_revision: "7",
    });
    expect(lifecycleCalls[1][1]).toMatchObject({
      network_id: "net-alpha",
      target_node_id: CAROL_ID,
      action: "suspend",
      expected_revision: "9",
    });
  });
});
