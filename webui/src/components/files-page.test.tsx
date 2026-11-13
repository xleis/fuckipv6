import {
  cleanup,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import productFixture from "../../../contracts/typescript/fixtures/product-visual-snapshot.json";
import type { NodeSnapshot } from "../../../contracts/typescript/node-contract-v1";
import { installTestI18nRuntime, translated } from "../test/runtime";
import { FilesPage } from "./files-page";

const snapshot = productFixture.snapshot as NodeSnapshot;
const target = {
  peer: "node_file_peer",
  network_id: "net_home",
  network_name: "home",
  share_id: "share_home",
};

type ApiCall = {
  url: URL;
  init: RequestInit;
};

type ApiHandler = (call: ApiCall) => Response | Promise<Response>;

function json(payload: unknown, status = 200): Response {
  return new Response(JSON.stringify(payload), {
    status,
    headers: { "Content-Type": "application/json" },
  });
}

function stubFetch(handler: ApiHandler) {
  const mock = vi.fn(
    async (input: RequestInfo | URL, init: RequestInit = {}) => {
      const url = new URL(String(input), "http://localhost");
      return handler({ url, init });
    },
  );
  vi.stubGlobal("fetch", mock);
  return mock;
}

function callsTo(mock: ReturnType<typeof stubFetch>, path: string): ApiCall[] {
  return mock.mock.calls
    .map(([input, init = {}]) => ({
      url: new URL(String(input), "http://localhost"),
      init,
    }))
    .filter((call) => call.url.pathname === path);
}

function standardHandler({
  targets = [target],
  directory = { entries: [], next_cursor: null, truncated: false },
  transfers = [],
}: {
  targets?: unknown[];
  directory?: unknown;
  transfers?: unknown[];
} = {}): ApiHandler {
  return ({ url }) => {
    if (url.pathname === "/api/v1/file/targets") return json(targets);
    if (url.pathname === "/api/v1/file/directory") return json(directory);
    if (url.pathname === "/api/v1/file/transfers") return json(transfers);
    throw new Error(`unexpected File API request: ${url.pathname}`);
  };
}

function renderPage() {
  return render(
    <FilesPage snapshot={snapshot} locale="zh-CN" selectedNetwork="home" />,
  );
}

beforeEach(() => {
  installTestI18nRuntime();
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

describe("FilesPage", () => {
  it("shows explicit empty states when no File target exists", async () => {
    stubFetch(standardHandler({ targets: [] }));
    renderPage();

    expect(await screen.findByTestId("file-no-targets")).toHaveTextContent(
      translated("file.page.no_targets.title"),
    );
    expect(await screen.findByTestId("file-transfers-empty")).toHaveTextContent(
      translated("file.page.transfers.empty"),
    );
    expect(screen.getByTestId("file-no-targets")).toHaveTextContent(
      translated("file.page.no_targets.detail"),
    );
  });

  it("prefers the selected network, loads a directory, and navigates into folders", async () => {
    const fetchMock = stubFetch(({ url }) => {
      if (url.pathname === "/api/v1/file/targets") {
        return json([
          {
            ...target,
            peer: "node_office",
            network_id: "net_office",
            network_name: "office",
          },
          target,
        ]);
      }
      if (url.pathname === "/api/v1/file/directory") {
        const path = url.searchParams.get("path");
        return json(
          path === "docs"
            ? {
                entries: [
                  {
                    path: "docs/readme.md",
                    is_directory: false,
                    size: 42,
                    modified_at_unix_ms: 1_700_000_000_000,
                  },
                ],
                next_cursor: null,
                truncated: false,
              }
            : {
                entries: [
                  {
                    path: "docs",
                    is_directory: true,
                    size: 0,
                    modified_at_unix_ms: null,
                  },
                ],
                next_cursor: null,
                truncated: false,
              },
        );
      }
      if (url.pathname === "/api/v1/file/transfers") return json([]);
      throw new Error(`unexpected request ${url.pathname}`);
    });

    const user = userEvent.setup();
    renderPage();

    const folder = await screen.findByRole("button", { name: "docs" });
    expect(screen.getByRole("combobox")).toHaveValue(
      "net_home\u0000node_file_peer",
    );
    await user.click(folder);

    expect(
      await screen.findByRole("button", { name: "readme.md" }),
    ).toBeVisible();
    const directoryCalls = callsTo(fetchMock, "/api/v1/file/directory");
    expect(directoryCalls).toHaveLength(2);
    expect(directoryCalls[0].url.searchParams.get("network_id")).toBe(
      "net_home",
    );
    expect(directoryCalls[0].url.searchParams.get("path")).toBe("");
    expect(directoryCalls[1].url.searchParams.get("path")).toBe("docs");
    expect(
      screen.getByRole("navigation", {
        name: translated("file.page.breadcrumb"),
      }),
    ).toHaveTextContent("docs");
  });

  it("uploads the selected file as an octet stream with the logical target path", async () => {
    const fetchMock = stubFetch(({ url }) => {
      if (url.pathname === "/api/v1/file/uploads") {
        return json(
          {
            transfer_id: "transfer-upload",
            operation_id: "operation-upload",
            size_bytes: 5,
          },
          201,
        );
      }
      if (url.pathname === "/api/v1/file/directory") {
        return json({ entries: [], next_cursor: null, truncated: false });
      }
      if (url.pathname === "/api/v1/file/targets") return json([target]);
      if (url.pathname === "/api/v1/file/transfers") return json([]);
      throw new Error(`unexpected request ${url.pathname}`);
    });
    const user = userEvent.setup();
    const { container } = renderPage();

    await screen.findByText(translated("file.page.directory.empty"));
    const file = new File(["hello"], "hello.txt", { type: "text/plain" });
    const fileInput = container.querySelector(
      'input[type="file"]',
    ) as HTMLInputElement;
    await user.upload(fileInput, file);
    await user.click(
      screen.getByRole("button", {
        name: translated("file.page.upload"),
      }),
    );

    await waitFor(() => {
      expect(callsTo(fetchMock, "/api/v1/file/uploads")).toHaveLength(1);
    });
    const [upload] = callsTo(fetchMock, "/api/v1/file/uploads");
    expect(upload.url.searchParams.get("peer")).toBe(target.peer);
    expect(upload.url.searchParams.get("network_id")).toBe(target.network_id);
    expect(upload.url.searchParams.get("share_id")).toBe(target.share_id);
    expect(upload.url.searchParams.get("path")).toBe("hello.txt");
    expect(new Headers(upload.init.headers).get("Content-Type")).toBe(
      "application/octet-stream",
    );
    expect(upload.init.body).toBe(file);
    expect(
      await screen.findByText(translated("file.page.upload_started")),
    ).toBeVisible();
    expect(screen.getByText(translated("file.page.choose_file"))).toBeVisible();
  });

  it("starts a download by POST and clicks a hidden token anchor", async () => {
    const fetchMock = stubFetch(({ url, init }) => {
      if (url.pathname === "/api/v1/file/targets") return json([target]);
      if (url.pathname === "/api/v1/file/transfers") return json([]);
      if (url.pathname === "/api/v1/file/directory") {
        return json({
          entries: [
            {
              path: "reports/final.txt",
              is_directory: false,
              size: 12,
              modified_at_unix_ms: 1_700_000_000_000,
            },
          ],
          next_cursor: null,
          truncated: false,
        });
      }
      if (url.pathname === "/api/v1/file/downloads" && init.method === "POST") {
        return json({
          token: "download token",
          transfer_id: "transfer-download",
          operation_id: "operation-download",
          expires_at_unix_ms: 1_700_100_000_000,
        });
      }
      throw new Error(`unexpected request ${init.method} ${url.pathname}`);
    });
    const clickedAnchors: HTMLAnchorElement[] = [];
    const click = vi
      .spyOn(HTMLAnchorElement.prototype, "click")
      .mockImplementation(function (this: HTMLAnchorElement) {
        clickedAnchors.push(this);
      });
    const user = userEvent.setup();
    renderPage();

    await user.click(
      await screen.findByRole("button", {
        name: translated("file.page.download_aria"),
      }),
    );

    await waitFor(() => expect(click).toHaveBeenCalledTimes(1));
    const [download] = callsTo(fetchMock, "/api/v1/file/downloads");
    expect(download.init.method).toBe("POST");
    expect(JSON.parse(String(download.init.body))).toEqual({
      peer: target.peer,
      network_id: target.network_id,
      share_id: target.share_id,
      path: "reports/final.txt",
    });
    if (!clickedAnchors[0]) throw new Error("download anchor was not clicked");
    expect(clickedAnchors[0].download).toBe("final.txt");
    expect(clickedAnchors[0].href).toContain(
      "/api/v1/file/downloads/download%20token",
    );
  });

  it("pauses, resumes, and cancels transfers through the node API", async () => {
    const transfers = [
      {
        transfer_id: "transfer-active",
        operation_id: "operation-active",
        direction: "upload",
        state: "active",
        transferred_bytes: 2,
        total_bytes: 10,
        received_chunks: 1,
        total_chunks: 5,
        updated_at_unix_ms: 1_700_000_000_000,
        error_code: null,
        failure_kind: null,
      },
      {
        transfer_id: "transfer-paused",
        operation_id: "operation-paused",
        direction: "download",
        state: "paused",
        transferred_bytes: 4,
        total_bytes: 10,
        received_chunks: 2,
        total_chunks: 5,
        updated_at_unix_ms: 1_700_000_000_000,
        error_code: null,
        failure_kind: null,
      },
    ];
    const fetchMock = stubFetch(({ url, init }) => {
      if (url.pathname === "/api/v1/file/targets") return json([target]);
      if (url.pathname === "/api/v1/file/directory") {
        return json({ entries: [], next_cursor: null, truncated: false });
      }
      if (url.pathname === "/api/v1/file/transfers") return json(transfers);
      if (url.pathname.startsWith("/api/v1/file/transfers/")) {
        return json({ status: "ok" });
      }
      throw new Error(`unexpected request ${init.method} ${url.pathname}`);
    });
    const user = userEvent.setup();
    const { container } = renderPage();

    await screen.findByText("transfer-active");
    const activeCard = screen.getByText("transfer-active").closest("article");
    const pausedCard = screen.getByText("transfer-paused").closest("article");
    if (!activeCard || !pausedCard) throw new Error("transfer cards missing");

    await user.click(
      within(activeCard).getByRole("button", {
        name: translated("file.page.pause"),
      }),
    );
    await waitFor(() =>
      expect(
        callsTo(
          fetchMock,
          "/api/v1/file/transfers/transfer-active/operation-active/pause",
        ),
      ).toHaveLength(1),
    );
    await user.click(
      within(pausedCard).getByRole("button", {
        name: translated("file.page.resume"),
      }),
    );
    await waitFor(() =>
      expect(
        callsTo(
          fetchMock,
          "/api/v1/file/transfers/transfer-paused/operation-paused/resume",
        ),
      ).toHaveLength(1),
    );
    await user.click(
      within(pausedCard).getByRole("button", {
        name: translated("file.page.cancel"),
      }),
    );
    await waitFor(() =>
      expect(
        callsTo(
          fetchMock,
          "/api/v1/file/transfers/transfer-paused/operation-paused",
        ),
      ).toHaveLength(1),
    );

    const [pause] = callsTo(
      fetchMock,
      "/api/v1/file/transfers/transfer-active/operation-active/pause",
    );
    const [resume] = callsTo(
      fetchMock,
      "/api/v1/file/transfers/transfer-paused/operation-paused/resume",
    );
    const [cancel] = callsTo(
      fetchMock,
      "/api/v1/file/transfers/transfer-paused/operation-paused",
    );
    expect(pause.init.method).toBe("POST");
    expect(resume.init.method).toBe("POST");
    expect(cancel.init.method).toBe("DELETE");
    expect(container.querySelectorAll(".file-transfer-card")).toHaveLength(2);
  });

  it.each([
    [403, "file.page.error.permission"],
    [422, "file.page.error.unsupported"],
    [503, "file.page.error.unavailable"],
    [429, "file.page.error.rate_limited"],
  ])("classifies HTTP %s File target errors", async (status, titleKey) => {
    stubFetch(({ url }) => {
      if (url.pathname === "/api/v1/file/targets") {
        return json({ error: "rejected" }, status);
      }
      if (url.pathname === "/api/v1/file/transfers") return json([]);
      throw new Error(`unexpected request ${url.pathname}`);
    });
    renderPage();

    expect(await screen.findByText(translated(titleKey))).toBeVisible();
    expect(screen.getByRole("alert")).toHaveTextContent(translated(titleKey));
  });

  it("appends cursor pages without replacing the first directory page", async () => {
    const fetchMock = stubFetch(({ url }) => {
      if (url.pathname === "/api/v1/file/targets") return json([target]);
      if (url.pathname === "/api/v1/file/transfers") return json([]);
      if (url.pathname === "/api/v1/file/directory") {
        if (url.searchParams.get("cursor") === "cursor-2") {
          return json({
            entries: [
              {
                path: "second.txt",
                is_directory: false,
                size: 2,
                modified_at_unix_ms: null,
              },
            ],
            next_cursor: null,
            truncated: false,
          });
        }
        return json({
          entries: [
            {
              path: "first.txt",
              is_directory: false,
              size: 1,
              modified_at_unix_ms: null,
            },
          ],
          next_cursor: "cursor-2",
          truncated: true,
        });
      }
      throw new Error(`unexpected request ${url.pathname}`);
    });
    const user = userEvent.setup();
    renderPage();

    await screen.findByRole("button", { name: "first.txt" });
    await user.click(
      screen.getByRole("button", {
        name: translated("file.page.load_more"),
      }),
    );

    expect(
      await screen.findByRole("button", { name: "second.txt" }),
    ).toBeVisible();
    expect(screen.getByRole("button", { name: "first.txt" })).toBeVisible();
    expect(
      screen.queryByRole("button", {
        name: translated("file.page.load_more"),
      }),
    ).not.toBeInTheDocument();
    const directoryCalls = callsTo(fetchMock, "/api/v1/file/directory");
    expect(directoryCalls[1].url.searchParams.get("cursor")).toBe("cursor-2");
  });

  it("shows the authoritative empty directory state", async () => {
    stubFetch(standardHandler());
    renderPage();

    expect(await screen.findByTestId("file-directory-empty")).toHaveTextContent(
      translated("file.page.directory.empty"),
    );
    expect(
      screen.getByText(translated("file.page.directory.empty_detail")),
    ).toBeVisible();
  });
});
