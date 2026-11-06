#!/usr/bin/env node

import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { mkdtemp, mkdir, readFile, rm, writeFile } from "node:fs/promises";
import { createServer } from "node:http";
import { tmpdir } from "node:os";
import { dirname, extname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const WEBUI_ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const REPO_ROOT = resolve(WEBUI_ROOT, "..");
const DIST_ROOT = join(REPO_ROOT, "crates/client/src/webui/dist");
const FIXTURE_ROOT = join(REPO_ROOT, "contracts/typescript/fixtures");
const REPORT_ROOT = resolve(
  process.env.TOPOLOGY_CAPACITY_E2E_OUTPUT_DIR ??
    join(REPO_ROOT, "target/topology-capacity-e2e"),
);
const FIXED_SERVER_TIME = "2026-01-01T00:00:05Z";
const WEBSOCKET_GUID = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";
const NODE_COUNT = 1_000;
const CONNECTION_COUNT = 5_000;
const EVENT_BURST_COUNT = Number(
  process.env.TOPOLOGY_CAPACITY_EVENT_COUNT ?? 500,
);
const DESKTOP_VIEWPORT = {
  width: 960,
  height: 640,
  deviceScaleFactor: 1,
};
const BUDGETS = {
  topology_first_interactive_ms: 2_000,
  filter_ms: 500,
  selection_ms: 500,
  incremental_refresh_ms: 500,
  list_keyboard_navigation_ms: 500,
  long_task_ms: 50,
  max_graph_nodes: 80,
  max_graph_connections: 200,
  max_list_dom_rows: 40,
  max_snapshot_requests_for_500_events: 2,
  max_js_heap_mb: 256,
};

function commandAvailable(command) {
  return spawnSync(command, ["--version"], { stdio: "ignore" }).status === 0;
}

function findChrome() {
  const candidates = [
    process.env.CHROME_BIN,
    "chrome",
    "google-chrome-stable",
    "google-chrome",
    "chromium",
    "chromium-browser",
  ].filter(Boolean);
  const chrome = candidates.find(commandAvailable);
  if (!chrome) throw new Error("Chrome/Chromium not found; set CHROME_BIN");
  return chrome;
}

function run(command, args, options) {
  return new Promise((resolvePromise, reject) => {
    const child = spawn(command, args, { ...options, stdio: "inherit" });
    child.once("error", reject);
    child.once("exit", (code, signal) => {
      if (code === 0) resolvePromise();
      else reject(new Error(`${command} exited with ${signal ?? code}`));
    });
  });
}

function gitValue(args) {
  const result = spawnSync("git", args, {
    cwd: REPO_ROOT,
    encoding: "utf8",
  });
  if (result.status !== 0) throw new Error(`git ${args.join(" ")} failed`);
  return result.stdout.trim();
}

function detectAndroidDevices() {
  const result = spawnSync("adb", ["devices", "-l"], { encoding: "utf8" });
  if (result.error?.code === "ENOENT" || result.status !== 0) {
    return {
      adb_available: false,
      connected_devices: [],
      reason: "adb is unavailable",
    };
  }
  const connectedDevices = result.stdout
    .split("\n")
    .slice(1)
    .map((line) => line.trim())
    .filter((line) => line && !line.startsWith("*") && !line.endsWith("offline"));
  return {
    adb_available: true,
    connected_devices: connectedDevices,
    reason:
      connectedDevices.length === 0
        ? "no connected Android device"
        : "Android device detected, but this Chrome-only harness does not claim a device run",
  };
}

async function prepareSharedAssets(destination) {
  await run(
    "cargo",
    [
      "run",
      "--quiet",
      "-p",
      "fuckipv6-common",
      "--example",
      "export-browser-assets",
      "--",
      destination,
    ],
    { cwd: REPO_ROOT },
  );
}

function capacityTopologyFixture() {
  const presence = {
    state: "online",
    source: "live_observation",
    observed_at_unix_ms: 1_700_000_000_000,
  };
  const nodes = Array.from({ length: NODE_COUNT }, (_, index) => {
    const nodeId = `node-${String(index).padStart(4, "0")}`;
    const health =
      index % 17 === 0
        ? "degraded"
        : index % 29 === 0
          ? "unknown"
          : "healthy";
    const capability =
      index % 5 === 0 ? "relay" : index % 7 === 0 ? "exit" : "coordinator";
    const networks =
      index % 3 === 0
        ? ["capacity-a", "capacity-b"]
        : index % 3 === 1
          ? ["capacity-a"]
          : ["capacity-b"];
    return {
      node_id: nodeId,
      display_name: `Capacity Node ${String(index).padStart(4, "0")}`,
      health,
      is_self: index === 0,
      capabilities: [capability],
      networks: networks.map((networkId) => ({
        network_id: networkId,
        health,
        presence,
        role: index === 0 ? "owner" : index % 11 === 0 ? "administrator" : "member",
        status: health === "healthy" ? "active" : "degraded",
        capabilities: [capability],
      })),
    };
  });
  const connections = Array.from({ length: CONNECTION_COUNT }, (_, index) => {
    const from = index % NODE_COUNT;
    const to = (index * 7 + 13) % NODE_COUNT;
    return {
      connection_id: `capacity-connection-${String(index).padStart(4, "0")}`,
      from: `node-${String(from).padStart(4, "0")}`,
      to: `node-${String(to === from ? (to + 1) % NODE_COUNT : to).padStart(4, "0")}`,
      networks: [
        from % 3 === 2 || to % 3 === 2 ? "capacity-b" : "capacity-a",
      ],
      observed_at_unix_ms: 1_700_000_000_000 + index,
      state: ["active", "idle", "stale"][index % 3],
    };
  });

  return {
    revision: 9_000,
    viewer: "node-0000",
    generated_at_unix_ms: 1_700_000_005_000,
    truncated: false,
    boundaries: [
      {
        network_id: "capacity-a",
        name: "Capacity A",
        member_count: 667,
        visible_member_count: 617,
        viewer_role: "owner",
        capabilities: ["relay"],
      },
      {
        network_id: "capacity-b",
        name: "Capacity B",
        member_count: 667,
        visible_member_count: 617,
        viewer_role: "administrator",
        capabilities: ["exit"],
      },
    ],
    clusters: [
      {
        network_id: "capacity-a",
        hidden_member_count: 50,
        expandable: false,
        sample: [],
      },
      {
        network_id: "capacity-b",
        hidden_member_count: 50,
        expandable: false,
        sample: [],
      },
    ],
    nodes,
    connections,
  };
}

function memberDirectory(networkId) {
  return {
    network_id: networkId,
    generated_at_unix_ms: 1_700_000_006_000,
    viewer_role: "owner",
    can_manage_members: true,
    members: [
      {
        node_id: "node-0000",
        display_name: "Capacity Node 0000",
        identity_fingerprint: "capacity-self",
        role: "owner",
        groups: ["owner"],
        capabilities: ["relay"],
        status: "active",
        lifecycle_state: "active",
        revision: 1,
        access_epoch: 1,
        available_actions: [],
        presence: {
          state: "online",
          source: "live_observation",
          observed_at_unix_ms: 1_700_000_000_000,
        },
      },
    ],
  };
}

function websocketTextFrame(payload) {
  const body = Buffer.from(payload);
  if (body.length < 126) {
    return Buffer.concat([Buffer.from([0x81, body.length]), body]);
  }
  if (body.length < 65_536) {
    const header = Buffer.alloc(4);
    header[0] = 0x81;
    header[1] = 126;
    header.writeUInt16BE(body.length, 2);
    return Buffer.concat([header, body]);
  }
  const header = Buffer.alloc(10);
  header[0] = 0x81;
  header[1] = 127;
  header.writeBigUInt64BE(BigInt(body.length), 2);
  return Buffer.concat([header, body]);
}

function contentType(pathname) {
  switch (extname(pathname)) {
    case ".css":
      return "text/css; charset=utf-8";
    case ".js":
      return "text/javascript; charset=utf-8";
    case ".html":
      return "text/html; charset=utf-8";
    case ".json":
      return "application/json; charset=utf-8";
    default:
      return "application/octet-stream";
  }
}

function jsonResponse(response, value, status = 200) {
  response.writeHead(status, {
    "content-type": "application/json; charset=utf-8",
    "cache-control": "no-store",
  });
  response.end(JSON.stringify(value));
}

async function startFixtureServer(manifest, fixture, sharedAssets) {
  const clients = new Set();
  const sockets = new Set();
  let snapshotRequests = 0;
  const snapshotRequestTimes = [];
  const server = createServer(async (request, response) => {
    try {
      const url = new URL(request.url ?? "/", "http://127.0.0.1");
      if (url.pathname === "/api/v1/commands" && request.method === "POST") {
        let body = "";
        for await (const chunk of request) body += chunk;
        const envelope = JSON.parse(body);
        const command = envelope?.request?.command;
        if (command !== "list_network_members") {
          jsonResponse(
            response,
            {
              schema_version: 1,
              error: {
                code: "unsupported_command",
                message: `fixture does not implement ${String(command)}`,
              },
            },
            400,
          );
          return;
        }
        const networkId = String(envelope?.request?.payload?.network_id ?? "capacity-a");
        jsonResponse(response, {
          schema_version: 1,
          result: {
            request_id: envelope.request.request_id,
            outcome: {
              status: "network_members",
              data: memberDirectory(networkId),
            },
          },
        });
        return;
      }

      switch (url.pathname) {
        case "/":
          response.writeHead(200, { "content-type": contentType("index.html") });
          response.end(await readFile(join(DIST_ROOT, "index.html")));
          return;
        case "/assets/webui.css":
          response.writeHead(200, { "content-type": contentType("webui.css") });
          response.end(await readFile(join(sharedAssets, "webui.css")));
          return;
        case "/assets/i18n.js":
          response.writeHead(200, { "content-type": contentType("i18n.js") });
          response.end(await readFile(join(sharedAssets, "i18n.js")));
          return;
        case "/api/v1/auth/session":
          jsonResponse(response, { authenticated: true });
          return;
        case "/api/v1/settings/language":
          jsonResponse(response, { preference: fixture.language_preference });
          return;
        case "/api/v1/manifest":
          jsonResponse(response, {
            schema_version: 1,
            request_id: 1,
            server_time: FIXED_SERVER_TIME,
            manifest,
          });
          return;
        case "/api/v1/snapshot":
          snapshotRequests += 1;
          snapshotRequestTimes.push(Date.now());
          jsonResponse(response, {
            schema_version: 1,
            request_id: snapshotRequests,
            server_time: FIXED_SERVER_TIME,
            snapshot: fixture.snapshot,
          });
          return;
        default:
          break;
      }

      if (url.pathname.startsWith("/assets/")) {
        const relativeAsset = decodeURIComponent(url.pathname.slice("/assets/".length));
        const assetPath = resolve(DIST_ROOT, "assets", relativeAsset);
        if (!assetPath.startsWith(resolve(DIST_ROOT, "assets") + "/")) {
          response.writeHead(403);
          response.end("forbidden");
          return;
        }
        response.writeHead(200, { "content-type": contentType(assetPath) });
        response.end(await readFile(assetPath));
        return;
      }

      response.writeHead(404, { "content-type": "text/plain; charset=utf-8" });
      response.end("not found");
    } catch (error) {
      if (!response.headersSent) {
        jsonResponse(response, { error: String(error) }, 500);
      } else {
        response.destroy();
      }
    }
  });

  server.on("connection", (socket) => {
    sockets.add(socket);
    socket.once("close", () => sockets.delete(socket));
  });
  server.on("upgrade", (request, socket) => {
    const url = new URL(request.url ?? "/", "http://127.0.0.1");
    const key = request.headers["sec-websocket-key"];
    if (url.pathname !== "/api/node/events" || typeof key !== "string") {
      socket.destroy();
      return;
    }
    const accept = createHash("sha1").update(key + WEBSOCKET_GUID).digest("base64");
    socket.write(
      "HTTP/1.1 101 Switching Protocols\r\n" +
        "Upgrade: websocket\r\n" +
        "Connection: Upgrade\r\n" +
        `Sec-WebSocket-Accept: ${accept}\r\n\r\n`,
    );
    clients.add(socket);
    socket.on("error", () => {});
    socket.once("close", () => clients.delete(socket));
  });

  await new Promise((resolvePromise, reject) => {
    server.once("error", reject);
    server.listen(0, "127.0.0.1", resolvePromise);
  });
  const address = server.address();
  if (!address || typeof address === "string") throw new Error("fixture server did not bind TCP");

  return {
    origin: `http://127.0.0.1:${address.port}`,
    resetSnapshotRequests() {
      snapshotRequests = 0;
      snapshotRequestTimes.length = 0;
    },
    snapshotRequestCount() {
      return snapshotRequests;
    },
    snapshotRequestTimestamps() {
      return snapshotRequestTimes;
    },
    emitEventBurst(count = EVENT_BURST_COUNT) {
      fixture.snapshot.revision += 1;
      fixture.snapshot.topology.revision += 1;
      fixture.snapshot.topology.generated_at_unix_ms += 1;
      fixture.snapshot.topology.nodes[0].display_name = "Capacity Node 0000 refreshed";
      const payload = JSON.stringify({
        schema_version: 1,
        type: "event",
        event: { kind: "capacity_fixture_refresh" },
      });
      const frame = websocketTextFrame(payload);
      for (let index = 0; index < count; index += 1) {
        for (const client of clients) client.write(frame);
      }
      return { clients: clients.size, frames: count };
    },
    close: async () => {
      for (const client of clients) client.destroy();
      for (const socket of sockets) socket.destroy();
      await new Promise((resolvePromise) => server.close(resolvePromise));
    },
  };
}

async function stopChild(child) {
  if (!child || child.exitCode !== null) return;
  const exited = new Promise((resolvePromise) => child.once("exit", resolvePromise));
  child.kill("SIGTERM");
  await Promise.race([
    exited,
    new Promise((resolvePromise) => setTimeout(resolvePromise, 2_000)),
  ]);
  if (child.exitCode === null) {
    child.kill("SIGKILL");
    await exited;
  }
}

async function waitForFile(path, timeoutMs = 20_000) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    try {
      return await readFile(path, "utf8");
    } catch {
      await new Promise((resolvePromise) => setTimeout(resolvePromise, 50));
    }
  }
  throw new Error(`timed out waiting for ${path}`);
}

function sleep(milliseconds) {
  return new Promise((resolvePromise) => setTimeout(resolvePromise, milliseconds));
}

class CdpSession {
  constructor(socket) {
    this.socket = socket;
    this.nextId = 1;
    this.pending = new Map();
    this.eventWaiters = new Map();
    socket.addEventListener("message", (event) => {
      const message = JSON.parse(String(event.data));
      if (!message.id) {
        const waiters = this.eventWaiters.get(message.method);
        const waiter = waiters?.shift();
        if (waiter) {
          clearTimeout(waiter.timer);
          waiter.resolve(message.params);
        }
        return;
      }
      const pending = this.pending.get(message.id);
      if (!pending) return;
      this.pending.delete(message.id);
      if (message.error) pending.reject(new Error(`${pending.method}: ${message.error.message}`));
      else pending.resolve(message.result);
    });
    socket.addEventListener("close", () => {
      for (const pending of this.pending.values()) {
        pending.reject(new Error(`${pending.method}: CDP socket closed`));
      }
      this.pending.clear();
    });
  }

  static async connect(url) {
    const socket = new WebSocket(url);
    await new Promise((resolvePromise, reject) => {
      socket.addEventListener("open", resolvePromise, { once: true });
      socket.addEventListener("error", reject, { once: true });
    });
    return new CdpSession(socket);
  }

  send(method, params = {}) {
    const id = this.nextId++;
    this.socket.send(JSON.stringify({ id, method, params }));
    return new Promise((resolvePromise, reject) => {
      this.pending.set(id, { method, resolve: resolvePromise, reject });
    });
  }

  waitForEvent(method, timeoutMs = 10_000) {
    return new Promise((resolvePromise, reject) => {
      const waiters = this.eventWaiters.get(method) ?? [];
      const waiter = {
        resolve: resolvePromise,
        reject,
        timer: setTimeout(() => {
          const current = this.eventWaiters.get(method) ?? [];
          this.eventWaiters.set(
            method,
            current.filter((candidate) => candidate !== waiter),
          );
          reject(new Error(`timed out waiting for CDP event ${method}`));
        }, timeoutMs),
      };
      waiters.push(waiter);
      this.eventWaiters.set(method, waiters);
    });
  }

  close() {
    this.socket.close();
  }
}

async function evaluate(session, expression) {
  const result = await session.send("Runtime.evaluate", {
    expression,
    awaitPromise: true,
    returnByValue: true,
    userGesture: true,
  });
  if (result.exceptionDetails) {
    throw new Error(result.exceptionDetails.exception?.description ?? result.exceptionDetails.text);
  }
  return result.result.value;
}

async function waitFor(session, expression, description, timeoutMs = 20_000) {
  const deadline = Date.now() + timeoutMs;
  let lastError;
  while (Date.now() < deadline) {
    try {
      if (await evaluate(session, expression)) return;
    } catch (error) {
      lastError = error;
    }
    await sleep(20);
  }
  throw new Error(
    `timed out waiting for ${description}${lastError ? `: ${lastError.message}` : ""}`,
  );
}

async function waitForReady(session, timeoutMs = 20_000) {
  await waitFor(
    session,
    `Boolean(
      document.querySelector(".app-shell") &&
      document.querySelector(".nav-footer")?.textContent?.includes("Live")
    )`,
    "WebUI product shell and live event connection",
    timeoutMs,
  );
  await evaluate(session, "document.fonts.ready.then(() => true)");
  await sleep(100);
}

async function pressKey(session, key) {
  const definitions = {
    " ": { code: "Space", virtualKeyCode: 32, text: " " },
    End: { code: "End", virtualKeyCode: 35 },
  };
  const definition = definitions[key];
  if (!definition) throw new Error(`unsupported key ${key}`);
  const common = {
    key,
    code: definition.code,
    windowsVirtualKeyCode: definition.virtualKeyCode,
    nativeVirtualKeyCode: definition.virtualKeyCode,
    ...(definition.text
      ? { text: definition.text, unmodifiedText: definition.text }
      : {}),
  };
  await session.send("Input.dispatchKeyEvent", { type: "keyDown", ...common });
  await session.send("Input.dispatchKeyEvent", { type: "keyUp", ...common });
}

async function navigateToTopology(session) {
  const clickedModule = await evaluate(
    session,
    `(() => {
      const button = [...document.querySelectorAll(".module-button")].find(
        (element) => element.textContent?.trim() === "Networks",
      );
      button?.click();
      return Boolean(button);
    })()`,
  );
  assert.equal(clickedModule, true);
  await waitFor(
    session,
    `[...document.querySelectorAll('[role="tab"]')].some(
      (element) => element.textContent?.trim() === "Topology",
    )`,
    "Topology page tab",
  );
  const clickedTab = await evaluate(
    session,
    `(() => {
      const tab = [...document.querySelectorAll('[role="tab"]')].find(
        (element) => element.textContent?.trim() === "Topology",
      );
      tab?.click();
      return Boolean(tab);
    })()`,
  );
  assert.equal(clickedTab, true);
}

async function setSearchValue(session, value) {
  const changed = await evaluate(
    session,
    `(() => {
      const input = document.querySelector('.topology-filters input[type="search"]');
      if (!(input instanceof HTMLInputElement)) return false;
      const previous = input.value;
      const setter = Object.getOwnPropertyDescriptor(
        HTMLInputElement.prototype,
        "value",
      )?.set;
      setter?.call(input, ${JSON.stringify(value)});
      input._valueTracker?.setValue(previous);
      input.dispatchEvent(new Event("input", { bubbles: true }));
      return true;
    })()`,
  );
  assert.equal(changed, true);
}

async function installLongTaskObserver(session) {
  return evaluate(
    session,
    `(() => {
      const supported = PerformanceObserver.supportedEntryTypes?.includes("longtask") ?? false;
      window.__topologyCapacityLongTasks = [];
      if (!supported) return false;
      window.__topologyCapacityLongTaskObserver = new PerformanceObserver((list) => {
        for (const entry of list.getEntries()) {
          window.__topologyCapacityLongTasks.push({
            duration: entry.duration,
            startTime: entry.startTime,
            name: entry.name,
          });
        }
      });
      window.__topologyCapacityLongTaskObserver.observe({ type: "longtask" });
      return true;
    })()`,
  );
}

async function readLongTasks(session) {
  return evaluate(
    session,
    `(() => {
      window.__topologyCapacityLongTaskObserver?.disconnect();
      return {
        entries: window.__topologyCapacityLongTasks ?? [],
        marks: performance.getEntriesByType("mark")
          .filter((entry) => entry.name.startsWith("capacity:"))
          .map((entry) => ({ name: entry.name, startTime: entry.startTime })),
      };
    })()`,
  );
}

async function mark(session, name) {
  await evaluate(session, `performance.mark(${JSON.stringify(`capacity:${name}`)})`);
}

async function jsHeapBytes(session) {
  const result = await session.send("Performance.getMetrics");
  const metric = result.metrics.find((candidate) => candidate.name === "JSHeapUsedSize");
  return metric?.value ?? 0;
}

async function main() {
  if (!process.argv.includes("--skip-build")) {
    await run("npm", ["run", "build"], { cwd: WEBUI_ROOT });
  }
  const profileRefresh = process.argv.includes("--profile");
  const traceRefresh = process.argv.includes("--trace");
  await mkdir(REPORT_ROOT, { recursive: true });
  const workspace = await mkdtemp(join(tmpdir(), "fuckipv6-topology-capacity-e2e-"));
  const chromeProfile = join(workspace, "chrome-profile");
  const sharedAssets = join(workspace, "shared-assets");
  const checks = [];
  let fixtureServer;
  let chrome;
  let cdp;
  try {
    const manifest = JSON.parse(await readFile(join(FIXTURE_ROOT, "node-manifest.json"), "utf8"));
    const fixture = JSON.parse(
      await readFile(join(FIXTURE_ROOT, "product-visual-snapshot.json"), "utf8"),
    );
    const topology = capacityTopologyFixture();
    fixture.snapshot.topology = topology;
    const datasetHash = createHash("sha256")
      .update(JSON.stringify(topology))
      .digest("hex");
    await prepareSharedAssets(sharedAssets);
    fixtureServer = await startFixtureServer(manifest, fixture, sharedAssets);

    chrome = spawn(
      findChrome(),
      [
        "--headless=new",
        "--remote-debugging-port=0",
        `--user-data-dir=${chromeProfile}`,
        "--no-first-run",
        "--no-default-browser-check",
        "--disable-background-networking",
        "--disable-component-update",
        "--disable-default-apps",
        "--disable-extensions",
        "--disable-sync",
        "--metrics-recording-only",
        "--mute-audio",
        "--no-sandbox",
        "--hide-scrollbars",
        "about:blank",
      ],
      { stdio: ["ignore", "ignore", "pipe"] },
    );
    let chromeError = "";
    chrome.stderr.on("data", (chunk) => {
      chromeError = `${chromeError}${chunk}`.slice(-8_000);
    });
    const devtoolsPort = await waitForFile(join(chromeProfile, "DevToolsActivePort"));
    const port = Number(devtoolsPort.split("\n")[0]);
    if (!Number.isInteger(port) || port <= 0) throw new Error("invalid Chrome DevTools port");
    const targets = await fetch(`http://127.0.0.1:${port}/json/list`).then((response) =>
      response.json(),
    );
    const target = targets.find((candidate) => candidate.type === "page");
    if (!target) throw new Error("Chrome has no page target");
    cdp = await CdpSession.connect(target.webSocketDebuggerUrl);

    await cdp.send("Page.enable");
    await cdp.send("Runtime.enable");
    await cdp.send("Performance.enable");
    await cdp.send("Emulation.setDeviceMetricsOverride", {
      ...DESKTOP_VIEWPORT,
      screenWidth: DESKTOP_VIEWPORT.width,
      screenHeight: DESKTOP_VIEWPORT.height,
      mobile: false,
    });
    await cdp.send("Emulation.setLocaleOverride", { locale: "en-US" });
    await cdp.send("Emulation.setTimezoneOverride", { timezoneId: "UTC" });
    await cdp.send("Emulation.setEmulatedMedia", {
      features: [{ name: "prefers-reduced-motion", value: "reduce" }],
    });

    const pageStartedAt = Date.now();
    await cdp.send("Page.navigate", { url: `${fixtureServer.origin}/` });
    await waitForReady(cdp);
    const pageReadyMs = Date.now() - pageStartedAt;

    const topologyStartedAt = Date.now();
    await navigateToTopology(cdp);
    await waitFor(
      cdp,
      `document.querySelector(".topology-view[data-view='graph']") &&
       document.querySelector(".topology-canvas[data-layout-pending='false']") &&
       Number(document.querySelector(".topology-canvas")?.dataset.renderedNodes) > 0`,
      "interactive capacity graph",
      5_000,
    );
    const topologyFirstInteractiveMs = Date.now() - topologyStartedAt;
    assert.ok(
      topologyFirstInteractiveMs <= BUDGETS.topology_first_interactive_ms,
      `topology first interactive ${topologyFirstInteractiveMs}ms exceeds budget`,
    );
    checks.push("1,000-node topology becomes interactive within 2s");

    const graphCounts = await evaluate(
      cdp,
      `(() => {
        const canvas = document.querySelector(".topology-canvas");
        return {
          nodes: Number(canvas?.dataset.renderedNodes),
          connections: Number(canvas?.dataset.renderedConnections),
          nodeDom: document.querySelectorAll("g.topology-node[role='option']").length,
          connectionDom: document.querySelectorAll("g.topology-connection").length,
          filterStatus: document.querySelector(".topology-filter-count")?.textContent?.trim(),
          compactNotice: document.querySelector(".topology-graph-budget")?.textContent?.trim(),
        };
      })()`,
    );
    assert.ok(graphCounts.nodes <= BUDGETS.max_graph_nodes);
    assert.ok(graphCounts.connections <= BUDGETS.max_graph_connections);
    assert.equal(graphCounts.nodeDom, graphCounts.nodes);
    assert.equal(graphCounts.connectionDom, graphCounts.connections);
    assert.ok(graphCounts.compactNotice?.replaceAll(",", "").includes("1000"));
    checks.push("graph DOM is capped at 80 nodes and 200 connections");

    const longTasksSupported = await installLongTaskObserver(cdp);
    assert.equal(longTasksSupported, true, "Chrome must expose longtask observations");
    await mark(cdp, "filter-start");

    const filterStartedAt = Date.now();
    await setSearchValue(cdp, "Capacity Node 09");
    await waitFor(
      cdp,
      `document.querySelector(".topology-canvas[data-layout-pending='false']") &&
       Number(document.querySelector(".topology-canvas")?.dataset.renderedNodes) === 80`,
      "capped filtered graph",
      5_000,
    );
    const filterMs = Date.now() - filterStartedAt;
    await mark(cdp, "filter-end");
    assert.ok(filterMs <= BUDGETS.filter_ms, `filter ${filterMs}ms exceeds budget`);
    checks.push("search/filter over 1,000 nodes completes within 500ms");

    await mark(cdp, "selection-start");
    await evaluate(
      cdp,
      `(() => {
        const node = document.querySelector('g.topology-node[data-node-id="node-0900"]');
        node?.focus();
        return Boolean(node);
      })()`,
    );
    const selectionStartedAt = Date.now();
    await pressKey(cdp, " ");
    await waitFor(
      cdp,
      `document.querySelector('g.topology-node[data-node-id="node-0900"]')?.getAttribute("data-selected") === "true"`,
      "filtered node selection",
      5_000,
    );
    const selectionMs = Date.now() - selectionStartedAt;
    await mark(cdp, "selection-end");
    assert.ok(selectionMs <= BUDGETS.selection_ms, `selection ${selectionMs}ms exceeds budget`);
    checks.push("selection in a filtered capacity graph completes within 500ms");

    await mark(cdp, "filter-clear-start");
    await setSearchValue(cdp, "");
    await waitFor(
      cdp,
      `document.querySelector(".topology-canvas[data-layout-pending='false']") &&
       Number(document.querySelector(".topology-canvas")?.dataset.renderedNodes) === 80`,
      "cleared capacity filter",
      5_000,
    );
    await mark(cdp, "filter-clear-end");

    if (profileRefresh) {
      await cdp.send("Profiler.enable");
      await cdp.send("Profiler.start");
    }
    if (traceRefresh) {
      await cdp.send("Tracing.start", {
        categories:
          "devtools.timeline,disabled-by-default-devtools.timeline,blink.user_timing,v8.execute,toplevel,latencyInfo",
        transferMode: "ReturnAsStream",
      });
    }
    await mark(cdp, "refresh-start");
    fixtureServer.resetSnapshotRequests();
    const refreshStartedAt = Date.now();
    const burst = fixtureServer.emitEventBurst(EVENT_BURST_COUNT);
    assert.ok(burst.clients >= 1, "WebSocket fixture client is not connected");
    await waitFor(
      cdp,
      `document.querySelector('g.topology-node[data-node-id="node-0000"]')?.getAttribute("aria-label")?.includes("Capacity Node 0000 refreshed")`,
      "incremental capacity refresh",
      5_000,
    );
    const incrementalRefreshMs = Date.now() - refreshStartedAt;
    await mark(cdp, "refresh-visible");
    await sleep(500);
    await mark(cdp, "refresh-end");
    const snapshotRequestCount = fixtureServer.snapshotRequestCount();
    assert.ok(
      incrementalRefreshMs <= BUDGETS.incremental_refresh_ms,
      `incremental refresh ${incrementalRefreshMs}ms exceeds budget`,
    );
    assert.ok(
      snapshotRequestCount <= BUDGETS.max_snapshot_requests_for_500_events,
      `500 events produced ${snapshotRequestCount} snapshot requests`,
    );
    checks.push("500-event burst coalesces to at most two snapshot requests");
    checks.push("incremental refresh remains visible within 500ms");

    await mark(cdp, "list-switch-start");
    await evaluate(
      cdp,
      `(() => {
        const button = [...document.querySelectorAll(".topology-view-switch button")].find(
          (element) => element.textContent?.trim() === "List",
        );
        button?.click();
        return Boolean(button);
      })()`,
    );
    await waitFor(
      cdp,
      `document.querySelector(".topology-view[data-view='list']") &&
       document.querySelector(".topology-list-table table")?.getAttribute("aria-rowcount") === "1001" &&
       Number(document.querySelector(".topology-list-table")?.dataset.renderedRows) > 0`,
      "virtualized 1,000-row list",
      5_000,
    );
    await mark(cdp, "list-switch-end");
    const listCounts = await evaluate(
      cdp,
      `(() => ({
        ariaRowCount: Number(document.querySelector(".topology-list-table table")?.getAttribute("aria-rowcount")),
        renderedRows: Number(document.querySelector(".topology-list-table")?.dataset.renderedRows),
        rowDom: document.querySelectorAll("tbody tr[data-node-id]").length,
      }))()`,
    );
    assert.equal(listCounts.ariaRowCount, NODE_COUNT + 1);
    assert.ok(listCounts.renderedRows <= BUDGETS.max_list_dom_rows);
    assert.equal(listCounts.rowDom, listCounts.renderedRows);
    assert.ok(listCounts.rowDom < NODE_COUNT);
    checks.push("virtualized list exposes all 1,000 nodes with bounded row DOM");

    await mark(cdp, "list-end-start");
    await evaluate(
      cdp,
      `(() => {
        const row = document.querySelector("tbody tr[data-node-id]");
        row?.focus();
        return Boolean(row);
      })()`,
    );
    await waitFor(
      cdp,
      `document.activeElement?.matches("tbody tr[data-node-id]")`,
      "first virtualized row focus",
      5_000,
    );
    const listNavigationStartedAt = Date.now();
    await pressKey(cdp, "End");
    try {
      await waitFor(
        cdp,
        `document.activeElement?.matches("tbody tr[data-node-id='node-0999']")`,
        "last virtualized node after End",
        5_000,
      );
    } catch (error) {
      const navigationState = await evaluate(
        cdp,
        `(() => ({
          activeNodeId: document.activeElement?.getAttribute("data-node-id"),
          activeTag: document.activeElement?.tagName,
          activeRole: document.activeElement?.getAttribute("role"),
          scrollTop: document.querySelector(".topology-list-table")?.scrollTop,
          renderedRows: document.querySelector(".topology-list-table")?.dataset.renderedRows,
          visibleNodes: [...document.querySelectorAll("tbody tr[data-node-id]")].map(
            (row) => row.getAttribute("data-node-id"),
          ),
        }))()`,
      );
      throw new Error(`${error.message}: ${JSON.stringify(navigationState)}`);
    }
    const listKeyboardNavigationMs = Date.now() - listNavigationStartedAt;
    await mark(cdp, "list-end-focus");
    const finalRenderedRows = await evaluate(
      cdp,
      `Number(document.querySelector(".topology-list-table")?.dataset.renderedRows)`,
    );
    assert.ok(
      listKeyboardNavigationMs <= BUDGETS.list_keyboard_navigation_ms,
      `list keyboard navigation ${listKeyboardNavigationMs}ms exceeds budget`,
    );
    assert.ok(finalRenderedRows <= BUDGETS.max_list_dom_rows);
    await mark(cdp, "list-end-end");
    checks.push("End key reaches node 1,000 without expanding the virtual list");

    await sleep(250);
    if (profileRefresh) {
      const { profile } = await cdp.send("Profiler.stop");
      await writeFile(
        join(REPORT_ROOT, "refresh-cpu-profile.json"),
        `${JSON.stringify(profile, null, 2)}\n`,
      );
    }
    if (traceRefresh) {
      const completed = cdp.waitForEvent("Tracing.tracingComplete");
      await cdp.send("Tracing.end");
      const { stream } = await completed;
      let trace = "";
      while (true) {
        const chunk = await cdp.send("IO.read", { handle: stream });
        trace += chunk.base64Encoded
          ? Buffer.from(chunk.data, "base64").toString("utf8")
          : chunk.data;
        if (chunk.eof) break;
      }
      await cdp.send("IO.close", { handle: stream });
      await writeFile(
        join(REPORT_ROOT, "refresh-trace.json"),
        trace,
      );
    }
    const longTaskObservation = await readLongTasks(cdp);
    const longTasks = longTaskObservation.entries.map((entry) => entry.duration);
    const maxLongTaskMs = longTasks.length === 0 ? 0 : Math.max(...longTasks);
    if (maxLongTaskMs > BUDGETS.long_task_ms) {
      throw new Error(
        `observed ${maxLongTaskMs}ms long task: ${JSON.stringify({ ...longTaskObservation, snapshotRequestTimes: fixtureServer.snapshotRequestTimestamps() })}`,
      );
    }
    checks.push("filter, selection, incremental update and list navigation produce no >50ms long task");

    const heapBytes = await jsHeapBytes(cdp);
    const heapMb = heapBytes / (1024 * 1024);
    assert.ok(heapMb <= BUDGETS.max_js_heap_mb, `JS heap ${heapMb}MB exceeds budget`);
    checks.push("JS heap remains below the 256MB capacity budget");

    const browserVersion = await cdp.send("Browser.getVersion");
    const userAgent = await evaluate(cdp, "navigator.userAgent");
    const android = detectAndroidDevices();
    if (android.connected_devices.length > 0) {
      checks.push("Android device connectivity detected; device performance remains a separate gate");
    }
    const uname = spawnSync("uname", ["-srmo"], { encoding: "utf8" }).stdout.trim();
    const status = spawnSync("git", ["status", "--porcelain"], {
      cwd: REPO_ROOT,
      encoding: "utf8",
    }).stdout.trim();
    const report = {
      schema_version: 1,
      kind: "topology-capacity-e2e",
      generated_at: new Date().toISOString(),
      commit: {
        sha: gitValue(["rev-parse", "HEAD"]),
        branch: gitValue(["branch", "--show-current"]),
        working_tree_clean: status === "",
      },
      dataset: {
        generator: "topology-capacity-e2e-v1",
        nodes: NODE_COUNT,
        connections: CONNECTION_COUNT,
        sha256: datasetHash,
      },
      environment: {
        chrome: browserVersion.product,
        user_agent: userAgent,
        js_version: browserVersion.jsVersion,
        platform: process.platform,
        arch: process.arch,
        uname,
        node: process.version,
        viewport: DESKTOP_VIEWPORT,
        reduced_motion: "reduce",
      },
      budgets: BUDGETS,
      measurements: {
        page_ready_ms: pageReadyMs,
        topology_first_interactive_ms: topologyFirstInteractiveMs,
        filter_ms: filterMs,
        selection_ms: selectionMs,
        incremental_refresh_ms: incrementalRefreshMs,
        list_keyboard_navigation_ms: listKeyboardNavigationMs,
        event_frames: EVENT_BURST_COUNT,
        snapshot_requests_after_event_burst: snapshotRequestCount,
        rendered_graph_nodes: graphCounts.nodes,
        rendered_graph_connections: graphCounts.connections,
        rendered_list_rows: listCounts.renderedRows,
        final_rendered_list_rows: finalRenderedRows,
        list_aria_rowcount: listCounts.ariaRowCount,
        long_tasks_ms: longTasks,
        max_long_task_ms: maxLongTaskMs,
        js_heap_mb: Number(heapMb.toFixed(2)),
      },
      android_real_device: {
        ...android,
        tested: false,
      },
      checks,
    };

    await writeFile(join(REPORT_ROOT, "report.json"), `${JSON.stringify(report, null, 2)}\n`);
    process.stdout.write(`topology capacity E2E passed (${checks.length} checks)\n`);
    for (const check of checks) process.stdout.write(`- ${check}\n`);
    if (android.connected_devices.length === 0) {
      process.stdout.write(`Android real-device gate unresolved: ${android.reason}\n`);
    }
    process.stdout.write(`${join(REPORT_ROOT, "report.json")}\n`);
  } catch (error) {
    if (chrome?.exitCode === null) {
      const chromeStderr = chrome.stderr.read();
      if (chromeStderr) process.stderr.write(`Chrome stderr:\n${chromeStderr}\n`);
    }
    throw error;
  } finally {
    cdp?.close();
    await stopChild(chrome);
    await fixtureServer?.close();
    await rm(workspace, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 });
  }
}

main().catch((error) => {
  process.stderr.write(`${error.stack ?? error}\n`);
  process.exitCode = 1;
});
