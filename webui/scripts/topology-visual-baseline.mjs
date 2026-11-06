#!/usr/bin/env node

import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { mkdtemp, mkdir, readdir, readFile, rm, writeFile } from "node:fs/promises";
import { createServer } from "node:http";
import { tmpdir } from "node:os";
import { dirname, extname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const WEBUI_ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const REPO_ROOT = resolve(WEBUI_ROOT, "..");
const DIST_ROOT = join(REPO_ROOT, "crates/client/src/webui/dist");
const FIXTURE_ROOT = join(REPO_ROOT, "contracts/typescript/fixtures");
const SCENARIO_FIXTURE = join(
  FIXTURE_ROOT,
  "topology-presentation-scenarios.json",
);
const BASE_FIXTURE = join(FIXTURE_ROOT, "product-visual-snapshot.json");
const OUTPUT_ROOT = resolve(
  process.env.TOPOLOGY_BASELINE_OUTPUT_DIR ??
    join(REPO_ROOT, "target/topology-visual-baseline"),
);
const VIEWPORT = { width: 1440, height: 900, deviceScaleFactor: 2 };
const FIXED_SERVER_TIME = "2026-01-01T00:00:05Z";
const WEBSOCKET_GUID = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";
const HEALTH_VALUES = ["healthy", "unknown", "degraded", "unreachable"];
const CAPABILITY_ORDER = ["coordinator", "relay", "exit", "file"];
const INTERACTION_SCENARIO_ID = "multi-network";
const INTERACTION_NODE_ID = "node-multi-studio";
const TOPOLOGY_LAYOUT_STORAGE_KEY = "fuckipv6.topology.layout.v1";

function sha256(value) {
  return createHash("sha256").update(value).digest("hex");
}

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

function jsonResponse(response, value, status = 200) {
  response.writeHead(status, {
    "content-type": "application/json; charset=utf-8",
    "cache-control": "no-store",
  });
  response.end(JSON.stringify(value));
}

function fileResponse(response, body, contentType) {
  response.writeHead(200, {
    "content-type": contentType,
    "cache-control": "no-store",
  });
  response.end(body);
}

function contentType(pathname) {
  switch (extname(pathname)) {
    case ".css":
      return "text/css; charset=utf-8";
    case ".js":
      return "text/javascript; charset=utf-8";
    case ".html":
      return "text/html; charset=utf-8";
    default:
      return "application/octet-stream";
  }
}

function expectedConnectionStates(scenario) {
  return scenario.topology.connections.map((connection) => connection.state);
}

function validateScenarioFixture(document) {
  assert.equal(document.schema_version, 1, "scenario fixture schema version");
  assert.equal(document.locale, "en", "scenario fixture locale");
  assert.ok(Array.isArray(document.scenarios), "scenarios must be an array");
  assert.ok(document.scenarios.length >= 6, "expected at least six scenarios");
  const ids = new Set();
  for (const scenario of document.scenarios) {
    assert.match(scenario.id, /^[a-z0-9-]+$/, "scenario id must be a slug");
    assert.ok(!ids.has(scenario.id), `duplicate scenario id: ${scenario.id}`);
    ids.add(scenario.id);
    const topology = scenario.topology;
    assert.ok(topology, `${scenario.id} must include topology`);
    const boundaryIds = topology.boundaries.map((boundary) => boundary.network_id);
    const nodeIds = topology.nodes.map((node) => node.node_id);
    assert.equal(new Set(boundaryIds).size, boundaryIds.length);
    assert.equal(new Set(nodeIds).size, nodeIds.length);
    assert.ok(nodeIds.includes(topology.viewer), `${scenario.id} viewer must be visible`);
    assert.deepEqual(scenario.expect.boundary_ids, boundaryIds);
    assert.deepEqual(scenario.expect.node_ids, nodeIds);
    assert.deepEqual(
      scenario.expect.connection_states,
      expectedConnectionStates(scenario),
    );

    const sharedNodeIds = topology.nodes
      .filter((node) => node.networks.length > 1)
      .map((node) => node.node_id);
    assert.deepEqual(scenario.expect.shared_node_ids, sharedNodeIds);
    assert.deepEqual(
      scenario.expect.health_by_node,
      Object.fromEntries(topology.nodes.map((node) => [node.node_id, node.health])),
    );
    assert.deepEqual(
      scenario.expect.enabled_capabilities_by_node,
      Object.fromEntries(
        topology.nodes.map((node) => [node.node_id, node.capabilities ?? []]),
      ),
    );

    for (const node of topology.nodes) {
      for (const membership of node.networks) {
        assert.ok(
          boundaryIds.includes(membership.network_id),
          `${scenario.id}/${node.node_id} references another network`,
        );
        assert.ok(
          HEALTH_VALUES.includes(membership.health),
          `${scenario.id}/${node.node_id} has invalid membership health`,
        );
      }
    }
    for (const connection of topology.connections) {
      assert.ok(nodeIds.includes(connection.from), `${scenario.id} connection from is missing`);
      assert.ok(nodeIds.includes(connection.to), `${scenario.id} connection to is missing`);
      for (const networkId of connection.networks ?? []) {
        assert.ok(
          boundaryIds.includes(networkId),
          `${scenario.id} connection references another network`,
        );
      }
    }
    assert.ok(
      nodeIds.includes(scenario.expect.detail_node_id),
      `${scenario.id} detail node is missing`,
    );
  }
}

function scenarioSnapshot(baseFixture, scenario) {
  const snapshot = structuredClone(baseFixture.snapshot);
  snapshot.revision = scenario.topology.revision;
  snapshot.topology = structuredClone(scenario.topology);
  snapshot.lifecycle = {
    ...snapshot.lifecycle,
    active_networks: scenario.topology.boundaries.length,
    configured_networks: scenario.topology.boundaries.length,
    failed_networks: [],
  };
  const sourceNetworks = snapshot.networks ?? [];
  snapshot.networks = scenario.topology.boundaries.map((boundary, index) => {
    const source =
      sourceNetworks.find((network) => network.name === boundary.network_id) ??
      sourceNetworks[index % Math.max(sourceNetworks.length, 1)];
    if (!source) {
      throw new Error(`base fixture has no network template for ${boundary.network_id}`);
    }
    return {
      ...source,
      name: boundary.network_id,
      status: boundary.capabilities?.length ? source.status : "degraded",
      peer_count: Math.max(0, boundary.visible_member_count - 1),
    };
  });
  snapshot.peers = (snapshot.peers ?? []).filter((peer) =>
    scenario.topology.boundaries.some(
      (boundary) => boundary.network_id === peer.network_name,
    ),
  );
  return snapshot;
}

function memberDirectory(topology, networkId) {
  const boundary = topology.boundaries.find(
    (candidate) => candidate.network_id === networkId,
  );
  const viewerRole = boundary?.viewer_role ?? "member";
  return {
    network_id: networkId,
    generated_at_unix_ms: topology.generated_at_unix_ms + 1000,
    viewer_role: viewerRole,
    can_manage_members: viewerRole !== "member",
    members: topology.nodes.flatMap((node) => {
      const membership = node.networks.find(
        (candidate) => candidate.network_id === networkId,
      );
      if (!membership) return [];
      return [
        {
          node_id: node.node_id,
          display_name: node.display_name,
          identity_fingerprint: `fingerprint-${node.node_id}`,
          role: membership.role,
          groups: [membership.role],
          capabilities: membership.capabilities ?? node.capabilities ?? [],
          status: membership.status,
          lifecycle_state: membership.status,
          revision: topology.revision,
          access_epoch: 1,
          available_actions: [],
          presence: membership.presence,
        },
      ];
    }),
  };
}

async function startFixtureServer(manifest, getSnapshot, sharedAssets) {
  const appHtml = await readFile(join(DIST_ROOT, "index.html"));
  const webuiCss = await readFile(join(sharedAssets, "webui.css"));
  const i18nJs = await readFile(join(sharedAssets, "i18n.js"));
  const distAssets = new Map(
    await Promise.all(
      (await readdir(join(DIST_ROOT, "assets"))).map(async (name) => [
        `/assets/${name}`,
        await readFile(join(DIST_ROOT, "assets", name)),
      ]),
    ),
  );
  const sockets = new Set();

  const server = createServer((request, response) => {
    const url = new URL(request.url ?? "/", "http://127.0.0.1");
    if (url.pathname === "/api/v1/commands" && request.method === "POST") {
      let body = "";
      request.setEncoding("utf8");
      request.on("data", (chunk) => {
        body += chunk;
      });
      request.on("end", () => {
        try {
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
          const snapshot = getSnapshot();
          const networkId = String(
            envelope?.request?.payload?.network_id ??
              snapshot.topology?.boundaries[0]?.network_id ??
              "home",
          );
          jsonResponse(response, {
            schema_version: 1,
            result: {
              request_id: envelope.request.request_id,
              outcome: {
                status: "network_members",
                data: memberDirectory(snapshot.topology, networkId),
              },
            },
          });
        } catch {
          jsonResponse(response, { error: "invalid command envelope" }, 400);
        }
      });
      return;
    }

    switch (url.pathname) {
      case "/":
        fileResponse(response, appHtml, "text/html; charset=utf-8");
        return;
      case "/assets/webui.css":
        fileResponse(response, webuiCss, "text/css; charset=utf-8");
        return;
      case "/assets/i18n.js":
        fileResponse(response, i18nJs, "text/javascript; charset=utf-8");
        return;
      case "/api/v1/auth/session":
        jsonResponse(response, { authenticated: true });
        return;
      case "/api/v1/settings/language":
        jsonResponse(response, { preference: getSnapshot().language_preference });
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
        jsonResponse(response, {
          schema_version: 1,
          request_id: 1,
          server_time: FIXED_SERVER_TIME,
          snapshot: getSnapshot().snapshot,
        });
        return;
      default: {
        const asset = distAssets.get(url.pathname);
        if (asset) {
          fileResponse(response, asset, contentType(url.pathname));
          return;
        }
        response.writeHead(404, { "content-type": "text/plain; charset=utf-8" });
        response.end("not found");
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
  });

  await new Promise((resolvePromise, reject) => {
    server.once("error", reject);
    server.listen(0, "127.0.0.1", resolvePromise);
  });
  const address = server.address();
  if (!address || typeof address === "string") {
    throw new Error("fixture server did not bind TCP");
  }
  return {
    origin: `http://127.0.0.1:${address.port}`,
    close: async () => {
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
    socket.addEventListener("message", (event) => {
      const message = JSON.parse(String(event.data));
      if (!message.id) return;
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
    throw new Error(
      result.exceptionDetails.exception?.description ?? result.exceptionDetails.text,
    );
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
    await sleep(50);
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
    "WebUI product shell",
    timeoutMs,
  );
  await evaluate(session, "document.fonts.ready.then(() => true)");
  await sleep(250);
}

async function nodeCenter(session, nodeId) {
  return evaluate(
    session,
    `(() => {
      const node = document.querySelector('g.topology-node[data-node-id=${JSON.stringify(nodeId)}]');
      if (!node) throw new Error("interaction target is missing from the graph");
      const rect = node.getBoundingClientRect();
      const point = { x: rect.x + rect.width / 2, y: rect.y + rect.height / 2 };
      const hit = document.elementFromPoint(point.x, point.y)?.closest("g.topology-node");
      if (hit?.getAttribute("data-node-id") !== ${JSON.stringify(nodeId)}) {
        throw new Error("interaction target center is occluded by another node");
      }
      return point;
    })()`,
  );
}

async function dispatchMouse(session, type, x, y, options = {}) {
  await session.send("Input.dispatchMouseEvent", {
    type,
    x,
    y,
    ...options,
  });
}

async function movePointerAway(session) {
  await dispatchMouse(session, "mouseMoved", VIEWPORT.width / 2, 16, {
    button: "none",
    buttons: 0,
  });
  await waitFor(
    session,
    `!document.querySelector('[data-testid="topology-node-hover"]')`,
    "topology hover to clear",
    5_000,
  );
}

async function shiftClick(session, point) {
  const modifiers = 8;
  await dispatchMouse(session, "mouseMoved", point.x, point.y, {
    button: "none",
    buttons: 0,
    modifiers,
  });
  await dispatchMouse(session, "mousePressed", point.x, point.y, {
    button: "left",
    buttons: 1,
    clickCount: 1,
    modifiers,
  });
  await dispatchMouse(session, "mouseReleased", point.x, point.y, {
    button: "left",
    buttons: 0,
    clickCount: 1,
    modifiers,
  });
}

async function rightClick(session, point) {
  await dispatchMouse(session, "mouseMoved", point.x, point.y, {
    button: "none",
    buttons: 0,
  });
  await dispatchMouse(session, "mousePressed", point.x, point.y, {
    button: "right",
    buttons: 2,
    clickCount: 1,
  });
  await dispatchMouse(session, "mouseReleased", point.x, point.y, {
    button: "right",
    buttons: 0,
    clickCount: 1,
  });
}

function transformTranslation(transform) {
  const match = /^translate\(([^ ]+) ([^)]+)\)$/.exec(transform ?? "");
  assert.ok(match, `invalid topology node transform: ${transform}`);
  return { x: Number(match[1]), y: Number(match[2]) };
}

async function layoutPersistenceEvidence(session, nodeId) {
  return evaluate(
    session,
    `(() => {
      const view = document.querySelector(".topology-view");
      const node = document.querySelector('g.topology-node[data-node-id=${JSON.stringify(nodeId)}]');
      const raw = localStorage.getItem(${JSON.stringify(TOPOLOGY_LAYOUT_STORAGE_KEY)});
      let stored = null;
      let parseError = false;
      if (raw !== null) {
        try {
          stored = JSON.parse(raw);
        } catch {
          parseError = true;
        }
      }
      return {
        fixed_count: Number(view?.getAttribute("data-layout-fixed-count")),
        pinned: view?.getAttribute("data-layout-pinned"),
        storage_status: view?.getAttribute("data-layout-storage-status"),
        transform: node?.getAttribute("transform") ?? null,
        data_layout_pinned: node?.getAttribute("data-layout-pinned") ?? null,
        dragging: node?.getAttribute("data-dragging") ?? null,
        storage_raw: raw,
        storage: stored,
        storage_parse_error: parseError,
      };
    })()`,
  );
}

async function dragNodeBy(session, nodeId, dx, dy) {
  const start = await nodeCenter(session, nodeId);
  const target = { x: start.x + dx, y: start.y + dy };
  await dispatchMouse(session, "mouseMoved", start.x, start.y, {
    button: "none",
    buttons: 0,
  });
  await dispatchMouse(session, "mousePressed", start.x, start.y, {
    button: "left",
    buttons: 1,
    clickCount: 1,
  });
  for (let step = 1; step <= 6; step += 1) {
    await dispatchMouse(
      session,
      "mouseMoved",
      start.x + ((target.x - start.x) * step) / 6,
      start.y + ((target.y - start.y) * step) / 6,
      { button: "left", buttons: 1 },
    );
    await sleep(16);
  }
  await dispatchMouse(session, "mouseReleased", target.x, target.y, {
    button: "left",
    buttons: 0,
    clickCount: 1,
  });
  await waitFor(
    session,
    `(() => {
      const view = document.querySelector(".topology-view");
      const node = document.querySelector('g.topology-node[data-node-id=${JSON.stringify(nodeId)}]');
      return view?.getAttribute("data-layout-fixed-count") === "1" &&
        view?.getAttribute("data-layout-storage-status") === "loaded" &&
        node?.getAttribute("data-layout-pinned") === "true" &&
        node?.getAttribute("data-dragging") === "false";
    })()`,
    "committed pinned topology layout",
    5_000,
  );
}

async function clickTopologyControl(session, ariaLabel) {
  const point = await evaluate(
    session,
    `(() => {
      const control = [...document.querySelectorAll("button")].find(
        (element) => element.getAttribute("aria-label") === ${JSON.stringify(ariaLabel)},
      );
      if (!control) throw new Error("topology control is missing");
      const rect = control.getBoundingClientRect();
      const point = { x: rect.x + rect.width / 2, y: rect.y + rect.height / 2 };
      const hit = document.elementFromPoint(point.x, point.y)?.closest("button");
      if (hit !== control) throw new Error("topology control is occluded");
      return point;
    })()`,
  );
  await dispatchMouse(session, "mouseMoved", point.x, point.y, {
    button: "none",
    buttons: 0,
  });
  await dispatchMouse(session, "mousePressed", point.x, point.y, {
    button: "left",
    buttons: 1,
    clickCount: 1,
  });
  await dispatchMouse(session, "mouseReleased", point.x, point.y, {
    button: "left",
    buttons: 0,
    clickCount: 1,
  });
}

async function reloadTopology(session, nodeCount) {
  await session.send("Page.reload", { ignoreCache: true });
  await waitForReady(session);
  await navigateToTopology(session, nodeCount);
}

async function captureViewport(session) {
  const screenshot = await session.send("Page.captureScreenshot", {
    format: "png",
    fromSurface: true,
    captureBeyondViewport: false,
  });
  const image = Buffer.from(screenshot.data, "base64");
  const dimensions = pngDimensions(image);
  assert.equal(dimensions.width, VIEWPORT.width * VIEWPORT.deviceScaleFactor);
  assert.equal(dimensions.height, VIEWPORT.height * VIEWPORT.deviceScaleFactor);
  return { image, dimensions, sha256: sha256(image) };
}

async function navigateToTopology(session, nodeCount) {
  await waitFor(
    session,
    `[...document.querySelectorAll(".module-button")].some((element) => element.textContent?.trim() === "Networks")`,
    "Networks module",
  );
  await evaluate(
    session,
    `(() => {
      const button = [...document.querySelectorAll(".module-button")].find(
        (element) => element.textContent?.trim() === "Networks",
      );
      button?.click();
      return Boolean(button);
    })()`,
  );
  await waitFor(
    session,
    `[...document.querySelectorAll('[role="tab"]')].some((element) => element.textContent?.trim() === "Topology")`,
    "Topology tab",
  );
  await evaluate(
    session,
    `(() => {
      const tab = [...document.querySelectorAll('[role="tab"]')].find(
        (element) => element.textContent?.trim() === "Topology",
      );
      tab?.click();
      return Boolean(tab);
    })()`,
  );
  await waitFor(
    session,
    `Boolean(
      document.querySelector(".topology-view[data-view='graph']") &&
      document.querySelector(".topology-canvas[data-layout-pending='false']") &&
      Number(document.querySelector(".topology-canvas")?.dataset.renderedNodes) === ${nodeCount}
    )`,
    "committed topology graph",
  );
  await evaluate(session, "document.fonts.ready.then(() => true)");
  await sleep(300);
}

function pngDimensions(buffer) {
  if (buffer.toString("ascii", 1, 4) !== "PNG") throw new Error("capture is not PNG");
  return { width: buffer.readUInt32BE(16), height: buffer.readUInt32BE(20) };
}

async function topologySemantics(session) {
  return evaluate(
    session,
    `(() => {
      const text = (element) => element?.textContent?.trim() ?? "";
      const rect = (element) => {
        if (!element) return null;
        const bounds = element.getBoundingClientRect();
        return {
          x: Number(bounds.x.toFixed(4)),
          y: Number(bounds.y.toFixed(4)),
          width: Number(bounds.width.toFixed(4)),
          height: Number(bounds.height.toFixed(4)),
        };
      };
      const canvas = document.querySelector(".topology-canvas");
      const view = document.querySelector(".topology-view");
      return {
        title: document.title,
        locale: document.documentElement.lang,
        font_family: getComputedStyle(document.body).fontFamily
          .split(",")[0]
          .trim()
          .replace(/^["']|["']$/g, ""),
        viewport: { width: innerWidth, height: innerHeight, devicePixelRatio },
        view: {
          mode: view?.getAttribute("data-view"),
          presentation_state: view?.getAttribute("data-topology-state"),
          rect: rect(view),
        },
        canvas: {
          layout_pending: canvas?.getAttribute("data-layout-pending"),
          rendered_nodes: Number(canvas?.getAttribute("data-rendered-nodes")),
          rendered_connections: Number(canvas?.getAttribute("data-rendered-connections")),
          rect: rect(canvas),
          svg_zoom: document.querySelector(".topology-canvas svg")?.getAttribute("data-zoom"),
          svg_color_mode: document.querySelector(".topology-canvas svg")?.getAttribute("data-color-mode"),
        },
        boundaries: [...document.querySelectorAll("g.topology-boundary")].map((element) => ({
          label: text(element.querySelector(".topology-boundary-label")),
          rect: rect(element),
          stroke: element.querySelector("ellipse")?.getAttribute("stroke"),
          dash: element.querySelector("ellipse")?.getAttribute("stroke-dasharray"),
        })),
        nodes: [...document.querySelectorAll("g.topology-node[role='option']")].map((element) => ({
          node_id: element.getAttribute("data-node-id"),
          health: ${JSON.stringify(HEALTH_VALUES)}.find((value) => element.classList.contains(value)) ?? null,
          selected: element.getAttribute("data-selected"),
          shared: Boolean(element.querySelector(".topology-node-shared")),
          shared_count: text(element.querySelector(".topology-node-shared text")),
          enabled_capabilities: element.querySelectorAll(".capability-enabled").length,
          disabled_capabilities: element.querySelectorAll(".capability-disabled").length,
          label: text(element.querySelector(".topology-node-label")),
          aria_label: element.getAttribute("aria-label"),
          rect: rect(element),
        })),
        connections: [...document.querySelectorAll("g.topology-connection")].map((element) => ({
          state: [...element.classList].find((value) =>
            ${JSON.stringify(["active", "idle", "stale"])}.includes(value),
          ) ?? null,
          title: text(element.querySelector("title")),
          dash: element.querySelector("line")?.getAttribute("stroke-dasharray"),
        })),
        state_notices: [...document.querySelectorAll(".state-notice[data-topology-state]")].map((element) => ({
          state: element.getAttribute("data-topology-state"),
          text: text(element),
        })),
        filter: {
          search: document.querySelector(".topology-filters input[type='search']")?.value ?? "",
          count: text(document.querySelector(".topology-filter-count")),
        },
      };
    })()`,
  );
}

function detailEvidence(session, nodeId) {
  return evaluate(
    session,
    `(async () => {
      const node = document.querySelector('g.topology-node[data-node-id=${JSON.stringify(nodeId)}]');
      if (!node) throw new Error("detail target is missing from the graph");
      node.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true }));
      const deadline = Date.now() + 5000;
      while (Date.now() < deadline) {
        const detail = document.querySelector('[data-testid="topology-node-detail"]');
        if (detail?.querySelector("h2")?.textContent?.trim()) {
          return {
            heading: detail.querySelector("h2")?.textContent?.trim() ?? "",
            text: detail.textContent?.replace(/\\s+/g, " ").trim() ?? "",
            health: detail.querySelector(".status-pill")?.textContent?.trim() ?? "",
          };
        }
        await new Promise((resolve) => setTimeout(resolve, 50));
      }
      throw new Error("timed out waiting for node detail");
    })()`,
  );
}

function interactionSemantics(session) {
  return evaluate(
    session,
    `(() => {
      const text = (element) => element?.textContent?.replace(/\\s+/g, " ").trim() ?? "";
      const rect = (element) => {
        if (!element) return null;
        const bounds = element.getBoundingClientRect();
        return {
          x: Number(bounds.x.toFixed(4)),
          y: Number(bounds.y.toFixed(4)),
          width: Number(bounds.width.toFixed(4)),
          height: Number(bounds.height.toFixed(4)),
        };
      };
      const hoverElement = document.querySelector('[data-testid="topology-node-hover"]');
      const menuElement = document.querySelector('[data-testid="topology-node-menu"]');
      const selectionBar = document.querySelector(".topology-selection-bar");
      return {
        hover: hoverElement ? {
          heading: text(hoverElement.querySelector(".topology-node-hover-heading strong")),
          health: text(hoverElement.querySelector(".topology-node-hover-heading .status-pill")),
          fields: [...hoverElement.querySelectorAll("dl > div")].map((field) => ({
            label: text(field.querySelector("dt")),
            value: text(field.querySelector("dd")),
          })),
          hint: text(hoverElement.querySelector("p")),
          rect: rect(hoverElement),
        } : null,
        menu: menuElement ? {
          label: menuElement.getAttribute("aria-label"),
          heading: text(menuElement.querySelector(".topology-node-menu-heading strong")),
          node_id: text(menuElement.querySelector(".topology-node-menu-heading code")),
          items: [...menuElement.querySelectorAll('[role="menuitem"]')].map((item) => ({
            text: text(item),
            disabled: item.getAttribute("aria-disabled") === "true",
          })),
          rect: rect(menuElement),
        } : null,
        selection: {
          selected_ids: [...document.querySelectorAll('g.topology-node[data-selected="true"]')]
            .map((node) => node.getAttribute("data-node-id")),
          bar: selectionBar ? {
            text: text(selectionBar),
            rect: rect(selectionBar),
          } : null,
          notice: text(document.querySelector(".topology-selection-notice")),
        },
        viewport: { width: innerWidth, height: innerHeight, devicePixelRatio },
      };
    })()`,
  );
}

function assertInteractionSemantics(state, semantics) {
  assert.equal(semantics.viewport.width, VIEWPORT.width, `${state} viewport width`);
  assert.equal(semantics.viewport.height, VIEWPORT.height, `${state} viewport height`);
  assert.equal(
    semantics.viewport.devicePixelRatio,
    VIEWPORT.deviceScaleFactor,
    `${state} device pixel ratio`,
  );
  if (state === "hover") {
    assert.ok(semantics.hover, "hover state must render the hover card");
    assert.equal(semantics.hover.heading, "Studio Peer");
    assert.equal(semantics.hover.health, "Healthy");
    assert.deepEqual(
      semantics.hover.fields,
      [
        { label: "Role", value: "Member" },
        { label: "Capabilities", value: "Coordinator" },
        { label: "Health", value: "Healthy · Online" },
      ],
    );
    assert.match(semantics.hover.hint, /Shift\+F10/);
    assert.equal(semantics.menu, null);
    assert.deepEqual(semantics.selection.selected_ids, []);
    return;
  }
  if (state === "selected") {
    assert.equal(semantics.hover, null);
    assert.equal(semantics.menu, null);
    assert.deepEqual(semantics.selection.selected_ids, [INTERACTION_NODE_ID]);
    assert.match(semantics.selection.bar?.text ?? "", /1 nodes selected/);
    assert.match(semantics.selection.bar?.text ?? "", /Shift-click to add more nodes/);
    assert.equal(semantics.selection.notice, "");
    return;
  }
  if (state === "menu") {
    assert.equal(semantics.hover, null);
    assert.ok(semantics.menu, "menu state must render the object menu");
    assert.match(semantics.menu.label ?? "", /Studio Peer/);
    assert.equal(semantics.menu.heading, "Studio Peer");
    assert.equal(semantics.menu.node_id, INTERACTION_NODE_ID);
    assert.deepEqual(semantics.menu.items, [
      { text: "Open node details", disabled: false },
      { text: "Add to batch selection", disabled: false },
      { text: "Pin current layout", disabled: false },
      { text: "The server exposes no actions for this object", disabled: true },
    ]);
    assert.deepEqual(semantics.selection.selected_ids, []);
    return;
  }
  throw new Error(`unexpected interaction state: ${state}`);
}

function assertScenarioSemantics(scenario, semantics) {
  const topology = scenario.topology;
  const expectedBoundaryNames = topology.boundaries.map((boundary) => boundary.name);
  assert.equal(semantics.view.mode, "graph", `${scenario.id} must render graph mode`);
  assert.equal(semantics.view.presentation_state, "ready", `${scenario.id} presentation state`);
  assert.equal(semantics.locale, "en", `${scenario.id} locale`);
  assert.match(semantics.font_family, /Inter/, `${scenario.id} must use Inter`);
  assert.deepEqual(
    semantics.boundaries.map((boundary) => boundary.label),
    expectedBoundaryNames,
    `${scenario.id} boundary labels`,
  );
  assert.deepEqual(
    semantics.nodes.map((node) => node.node_id),
    topology.nodes.map((node) => node.node_id),
    `${scenario.id} node order`,
  );
  assert.deepEqual(
    semantics.connections.map((connection) => connection.state),
    topology.connections.map((connection) => connection.state),
    `${scenario.id} connection states`,
  );
  assert.equal(
    semantics.canvas.rendered_nodes,
    topology.nodes.length,
    `${scenario.id} rendered node count`,
  );
  assert.equal(
    semantics.canvas.rendered_connections,
    topology.connections.length,
    `${scenario.id} rendered connection count`,
  );
  assert.equal(semantics.canvas.layout_pending, "false");
  assert.deepEqual(semantics.state_notices, [], `${scenario.id} must be a ready state`);
  assert.equal(semantics.viewport.devicePixelRatio, VIEWPORT.deviceScaleFactor);

  const boundariesById = new Map(
    topology.boundaries.map((boundary) => [boundary.network_id, boundary]),
  );
  for (const [index, node] of topology.nodes.entries()) {
    const rendered = semantics.nodes[index];
    assert.equal(rendered.health, node.health, `${scenario.id}/${node.node_id} health`);
    assert.equal(
      rendered.enabled_capabilities,
      node.capabilities?.length ?? 0,
      `${scenario.id}/${node.node_id} enabled capability count`,
    );
    assert.equal(
      rendered.disabled_capabilities,
      CAPABILITY_ORDER.length - (node.capabilities?.length ?? 0),
      `${scenario.id}/${node.node_id} disabled capability count`,
    );
    assert.equal(
      rendered.shared,
      node.networks.length > 1,
      `${scenario.id}/${node.node_id} shared marker`,
    );
    assert.equal(
      rendered.shared_count,
      node.networks.length > 1 ? `×${node.networks.length}` : "",
    );
    assert.match(rendered.aria_label, new RegExp(node.display_name.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")));
    for (const membership of node.networks) {
      const boundary = boundariesById.get(membership.network_id);
      assert.ok(
        rendered.aria_label.includes(boundary.name),
        `${scenario.id}/${node.node_id} aria label must include ${boundary.name}`,
      );
    }
  }

  const connectionsByTitle = semantics.connections.map((connection) => connection.title);
  for (const connection of topology.connections) {
    const from = topology.nodes.find((node) => node.node_id === connection.from);
    const to = topology.nodes.find((node) => node.node_id === connection.to);
    assert.ok(
      connectionsByTitle.some(
        (title) => title.includes(from.display_name) && title.includes(to.display_name),
      ),
      `${scenario.id} connection title must name both endpoints`,
    );
  }
}

async function main() {
  if (!process.argv.includes("--skip-build")) {
    await run("npm", ["run", "build"], { cwd: WEBUI_ROOT });
  }
  await mkdir(OUTPUT_ROOT, { recursive: true });
  const scenarioText = await readFile(SCENARIO_FIXTURE, "utf8");
  const scenarioDocument = JSON.parse(scenarioText);
  validateScenarioFixture(scenarioDocument);
  const baseText = await readFile(BASE_FIXTURE, "utf8");
  const baseFixture = JSON.parse(baseText);
  const manifest = JSON.parse(
    await readFile(join(FIXTURE_ROOT, "node-manifest.json"), "utf8"),
  );
  const report = {
    schema_version: 1,
    kind: "topology-visual-baseline",
    fixture: {
      path: "contracts/typescript/fixtures/topology-presentation-scenarios.json",
      sha256: sha256(scenarioText),
      revision: scenarioDocument.fixture_revision,
      scenario_count: scenarioDocument.scenarios.length,
    },
    environment: {
      viewport: VIEWPORT,
      locale: "en-US",
      timezone: "UTC",
      reduced_motion: "reduce",
      font_family: "Inter",
    },
    scenarios: [],
    interactions: [],
    layout_persistence: null,
  };

  const workspace = await mkdtemp(join(tmpdir(), "fuckipv6-topology-baseline-"));
  const chromeProfile = join(workspace, "chrome-profile");
  const sharedAssets = join(workspace, "shared-assets");
  let fixtureServer;
  let chrome;
  let cdp;
  let activeFixture;
  let chromeError = "";
  try {
    const firstScenario = scenarioDocument.scenarios[0];
    activeFixture = {
      language_preference: scenarioDocument.language_preference,
      snapshot: scenarioSnapshot(baseFixture, firstScenario),
    };
    await prepareSharedAssets(sharedAssets);
    fixtureServer = await startFixtureServer(
      manifest,
      () => activeFixture,
      sharedAssets,
    );

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
        "--disable-gpu",
        "--disable-lcd-text",
        "--disable-sync",
        "--metrics-recording-only",
        "--mute-audio",
        "--no-sandbox",
        "--force-color-profile=srgb",
        "--font-render-hinting=none",
        "--hide-scrollbars",
        "about:blank",
      ],
      { stdio: ["ignore", "ignore", "pipe"] },
    );
    chrome.stderr.on("data", (chunk) => {
      chromeError = `${chromeError}${chunk}`.slice(-8_000);
    });
    const devtoolsPort = await waitForFile(join(chromeProfile, "DevToolsActivePort"));
    const port = Number(devtoolsPort.split("\n")[0]);
    if (!Number.isInteger(port) || port <= 0) {
      throw new Error("invalid Chrome DevTools port");
    }
    const targets = await fetch(`http://127.0.0.1:${port}/json/list`).then(
      (response) => response.json(),
    );
    const target = targets.find((candidate) => candidate.type === "page");
    if (!target) throw new Error("Chrome has no page target");
    cdp = await CdpSession.connect(target.webSocketDebuggerUrl);

    await cdp.send("Page.enable");
    await cdp.send("Runtime.enable");
    await cdp.send("Emulation.setDeviceMetricsOverride", {
      ...VIEWPORT,
      screenWidth: VIEWPORT.width,
      screenHeight: VIEWPORT.height,
      mobile: false,
    });
    await cdp.send("Emulation.setLocaleOverride", { locale: "en-US" });
    await cdp.send("Emulation.setTimezoneOverride", { timezoneId: "UTC" });
    await cdp.send("Emulation.setEmulatedMedia", {
      features: [{ name: "prefers-reduced-motion", value: "reduce" }],
    });
    const browser = await cdp.send("Browser.getVersion");
    report.environment.chrome = browser.product;
    report.environment.chrome_revision = browser.revision;
    report.environment.user_agent = browser.userAgent;
    report.environment.platform = process.platform;
    report.environment.node = process.version;

    const loadScenario = async (scenario) => {
      activeFixture = {
        language_preference: scenarioDocument.language_preference,
        snapshot: scenarioSnapshot(baseFixture, scenario),
      };
      await cdp.send("Page.navigate", { url: `${fixtureServer.origin}/` });
      await waitForReady(cdp);
      await navigateToTopology(cdp, scenario.topology.nodes.length);
    };

    // The first local-font layout can select a different rasterization path than
    // subsequent navigations in the same browser process. Warm it without output.
    await loadScenario(scenarioDocument.scenarios[0]);

    for (const scenario of scenarioDocument.scenarios) {
      await loadScenario(scenario);
      const semantics = await topologySemantics(cdp);
      assertScenarioSemantics(scenario, semantics);
      const screenshot = await cdp.send("Page.captureScreenshot", {
        format: "png",
        fromSurface: true,
        captureBeyondViewport: false,
      });
      const image = Buffer.from(screenshot.data, "base64");
      const dimensions = pngDimensions(image);
      assert.equal(dimensions.width, VIEWPORT.width * VIEWPORT.deviceScaleFactor);
      assert.equal(dimensions.height, VIEWPORT.height * VIEWPORT.deviceScaleFactor);
      const detail = await detailEvidence(cdp, scenario.expect.detail_node_id);
      for (const expectedText of scenario.expect.detail_contains) {
        assert.ok(
          detail.text.includes(expectedText),
          `${scenario.id} detail must include ${JSON.stringify(expectedText)}; received ${JSON.stringify(detail.text)}`,
        );
      }
      assert.equal(detail.heading, (
        scenario.topology.nodes.find(
          (node) => node.node_id === scenario.expect.detail_node_id,
        )
      ).display_name);

      const imageName = `${scenario.id}.png`;
      const metadataName = `${scenario.id}.json`;
      const imagePath = join(OUTPUT_ROOT, imageName);
      const metadataPath = join(OUTPUT_ROOT, metadataName);
      const imageSha256 = sha256(image);
      const scenarioReport = {
        id: scenario.id,
        title: scenario.title,
        topology_revision: scenario.topology.revision,
        topology_sha256: sha256(JSON.stringify(scenario.topology)),
        image: {
          path: imageName,
          dimensions,
          sha256: imageSha256,
        },
        semantics,
        detail,
        assertions: {
          boundary_ids: scenario.expect.boundary_ids,
          node_ids: scenario.expect.node_ids,
          shared_node_ids: scenario.expect.shared_node_ids,
          connection_states: scenario.expect.connection_states,
          detail_contains: scenario.expect.detail_contains,
        },
      };
      await writeFile(imagePath, image);
      await writeFile(metadataPath, `${JSON.stringify(scenarioReport, null, 2)}\n`);
      report.scenarios.push(scenarioReport);
      process.stdout.write(
        `${scenario.id}: ${dimensions.width}x${dimensions.height} ${imageSha256}\n`,
      );
    }

    const interactionScenario = scenarioDocument.scenarios.find(
      (scenario) => scenario.id === INTERACTION_SCENARIO_ID,
    );
    assert.ok(interactionScenario, "interaction scenario must exist");
    for (const state of ["hover", "selected", "menu"]) {
      await loadScenario(interactionScenario);
      const point = await nodeCenter(cdp, INTERACTION_NODE_ID);
      if (state === "hover") {
        await dispatchMouse(cdp, "mouseMoved", point.x, point.y, {
          button: "none",
          buttons: 0,
        });
        await waitFor(
          cdp,
          `Boolean(document.querySelector('[data-testid="topology-node-hover"]'))`,
          "topology hover card",
          5_000,
        );
      } else if (state === "selected") {
        await shiftClick(cdp, point);
        await waitFor(
          cdp,
          `document.querySelector('g.topology-node[data-node-id=${JSON.stringify(INTERACTION_NODE_ID)}]')?.getAttribute("data-selected") === "true"`,
          "topology node selection",
          5_000,
        );
        await movePointerAway(cdp);
      } else {
        await rightClick(cdp, point);
        await waitFor(
          cdp,
          `Boolean(document.querySelector('[data-testid="topology-node-menu"]'))`,
          "topology object menu",
          5_000,
        );
      }
      await sleep(150);
      const semantics = await interactionSemantics(cdp);
      assertInteractionSemantics(state, semantics);
      const screenshot = await cdp.send("Page.captureScreenshot", {
        format: "png",
        fromSurface: true,
        captureBeyondViewport: false,
      });
      const image = Buffer.from(screenshot.data, "base64");
      const dimensions = pngDimensions(image);
      assert.equal(dimensions.width, VIEWPORT.width * VIEWPORT.deviceScaleFactor);
      assert.equal(dimensions.height, VIEWPORT.height * VIEWPORT.deviceScaleFactor);
      const name = `${INTERACTION_SCENARIO_ID}-${state}`;
      const imageName = `${name}.png`;
      const metadataName = `${name}.json`;
      const imageSha256 = sha256(image);
      const interactionReport = {
        scenario_id: INTERACTION_SCENARIO_ID,
        node_id: INTERACTION_NODE_ID,
        state,
        topology_revision: interactionScenario.topology.revision,
        topology_sha256: sha256(JSON.stringify(interactionScenario.topology)),
        image: {
          path: imageName,
          dimensions,
          sha256: imageSha256,
        },
        semantics,
      };
      await writeFile(join(OUTPUT_ROOT, imageName), image);
      await writeFile(
        join(OUTPUT_ROOT, metadataName),
        `${JSON.stringify(interactionReport, null, 2)}\n`,
      );
      report.interactions.push(interactionReport);
      process.stdout.write(
        `${name}: ${dimensions.width}x${dimensions.height} ${imageSha256}\n`,
      );
    }

    await evaluate(
      cdp,
      `localStorage.removeItem(${JSON.stringify(TOPOLOGY_LAYOUT_STORAGE_KEY)}); true`,
    );
    await loadScenario(interactionScenario);
    const initialLayout = await layoutPersistenceEvidence(cdp, INTERACTION_NODE_ID);
    assert.equal(initialLayout.fixed_count, 0, "initial fixed position count");
    assert.equal(initialLayout.pinned, "false", "initial pin state");
    assert.equal(initialLayout.storage_status, "empty", "initial storage status");
    assert.equal(initialLayout.storage_raw, null, "initial layout storage");
    const automaticPosition = transformTranslation(initialLayout.transform);

    await dragNodeBy(cdp, INTERACTION_NODE_ID, 86, 58);
    await movePointerAway(cdp);
    const draggedLayout = await layoutPersistenceEvidence(cdp, INTERACTION_NODE_ID);
    const draggedPosition = transformTranslation(draggedLayout.transform);
    assert.notDeepEqual(draggedPosition, automaticPosition, "drag must move the node");
    assert.equal(draggedLayout.fixed_count, 1, "dragged fixed position count");
    assert.equal(draggedLayout.pinned, "true", "dragged pin state");
    assert.equal(draggedLayout.storage_status, "loaded", "dragged storage status");
    assert.equal(draggedLayout.storage_parse_error, false, "layout storage must parse");
    assert.deepEqual(Object.keys(draggedLayout.storage ?? {}).sort(), [
      "positions",
      "version",
    ]);
    assert.equal(draggedLayout.storage.version, 1);
    assert.deepEqual(Object.keys(draggedLayout.storage.positions), [
      INTERACTION_NODE_ID,
    ]);
    assert.deepEqual(
      Object.keys(draggedLayout.storage.positions[INTERACTION_NODE_ID]).sort(),
      ["x", "y"],
    );
    for (const sensitiveValue of [
      "Studio Peer",
      "Alpha",
      "Beta",
      "Healthy",
      "Coordinator",
      "Member",
    ]) {
      assert.equal(
        draggedLayout.storage_raw.includes(sensitiveValue),
        false,
        `layout storage must not contain ${sensitiveValue}`,
      );
    }
    await sleep(150);
    const pinnedCapture = await captureViewport(cdp);

    await reloadTopology(cdp, interactionScenario.topology.nodes.length);
    const restoredLayout = await layoutPersistenceEvidence(cdp, INTERACTION_NODE_ID);
    assert.deepEqual(
      transformTranslation(restoredLayout.transform),
      draggedPosition,
      "reload must restore the pinned node position",
    );
    assert.equal(restoredLayout.fixed_count, 1, "restored fixed position count");
    assert.equal(restoredLayout.pinned, "true", "restored pin state");
    assert.equal(restoredLayout.storage_status, "loaded", "restored storage status");

    await clickTopologyControl(cdp, "Reset pinned layout");
    await waitFor(
      cdp,
      `(() => {
        const view = document.querySelector(".topology-view");
        const node = document.querySelector('g.topology-node[data-node-id=${JSON.stringify(INTERACTION_NODE_ID)}]');
        return view?.getAttribute("data-layout-fixed-count") === "0" &&
          view?.getAttribute("data-layout-storage-status") === "empty" &&
          node?.getAttribute("data-layout-pinned") === "false";
      })()`,
      "reset topology layout",
      5_000,
    );
    await movePointerAway(cdp);
    const resetLayout = await layoutPersistenceEvidence(cdp, INTERACTION_NODE_ID);
    assert.deepEqual(
      transformTranslation(resetLayout.transform),
      automaticPosition,
      "reset must restore the automatic position",
    );
    assert.equal(resetLayout.storage_raw, null, "reset must clear layout storage");

    await reloadTopology(cdp, interactionScenario.topology.nodes.length);
    const resetReloadLayout = await layoutPersistenceEvidence(cdp, INTERACTION_NODE_ID);
    assert.deepEqual(
      transformTranslation(resetReloadLayout.transform),
      automaticPosition,
      "reload after reset must keep the automatic position",
    );
    assert.equal(resetReloadLayout.fixed_count, 0, "reset reload fixed position count");
    assert.equal(resetReloadLayout.storage_raw, null, "reset reload layout storage");
    await movePointerAway(cdp);
    await sleep(150);
    const resetCapture = await captureViewport(cdp);

    const pinnedImageName = `${INTERACTION_SCENARIO_ID}-layout-pinned.png`;
    const resetImageName = `${INTERACTION_SCENARIO_ID}-layout-reset.png`;
    await writeFile(join(OUTPUT_ROOT, pinnedImageName), pinnedCapture.image);
    await writeFile(join(OUTPUT_ROOT, resetImageName), resetCapture.image);
    report.layout_persistence = {
      scenario_id: INTERACTION_SCENARIO_ID,
      node_id: INTERACTION_NODE_ID,
      storage_key: TOPOLOGY_LAYOUT_STORAGE_KEY,
      storage_schema: {
        version: 1,
        persisted_fields: ["positions.<stable-node-id>.x", "positions.<stable-node-id>.y"],
        excluded: ["display_name", "network", "role", "address", "health", "connections"],
      },
      initial: initialLayout,
      dragged: draggedLayout,
      restored: restoredLayout,
      reset: resetLayout,
      reset_reloaded: resetReloadLayout,
      screenshots: [
        {
          state: "pinned",
          path: pinnedImageName,
          dimensions: pinnedCapture.dimensions,
          sha256: pinnedCapture.sha256,
        },
        {
          state: "reset",
          path: resetImageName,
          dimensions: resetCapture.dimensions,
          sha256: resetCapture.sha256,
        },
      ],
    };
    await writeFile(
      join(OUTPUT_ROOT, `${INTERACTION_SCENARIO_ID}-layout.json`),
      `${JSON.stringify(report.layout_persistence, null, 2)}\n`,
    );
    process.stdout.write(
      `${INTERACTION_SCENARIO_ID}-layout: drag, reload restore, reset, reload automatic ` +
        `${pinnedCapture.sha256} ${resetCapture.sha256}\n`,
    );

    await writeFile(
      join(OUTPUT_ROOT, "manifest.json"),
      `${JSON.stringify(report, null, 2)}\n`,
    );
    process.stdout.write(
      `topology visual baseline passed (${report.scenarios.length} scenarios, ` +
        `${report.interactions.length} interaction states, 1 layout persistence check)\n` +
        `${join(OUTPUT_ROOT, "manifest.json")}\n`,
    );
  } catch (error) {
    if (chrome && chrome.exitCode === null) {
      const chromeStderr = chrome.stderr.read();
      if (chromeStderr) process.stderr.write(`Chrome stderr:\n${chromeStderr}\n`);
      else if (chromeError) process.stderr.write(`Chrome stderr:\n${chromeError}\n`);
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
