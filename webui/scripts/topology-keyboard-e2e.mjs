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
const REPORT_ROOT = resolve(
  process.env.TOPOLOGY_E2E_OUTPUT_DIR ??
    join(REPO_ROOT, "target/topology-keyboard-e2e"),
);
const DESKTOP_VIEWPORT = {
  width: 1440,
  height: 900,
  deviceScaleFactor: 1,
};
const MOBILE_VIEWPORT = {
  width: 480,
  height: 900,
  deviceScaleFactor: 1,
};
const FIXED_SERVER_TIME = "2026-01-01T00:00:05Z";
const WEBSOCKET_GUID = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";
const SELF_ID = "node-self";
const BOB_ID = "node-bob";
const CAROL_ID = "node-carol";

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

function topologyFixture() {
  const presence = {
    state: "online",
    source: "live_observation",
    observed_at_unix_ms: 1_700_000_000_000,
  };
  return {
    revision: 4242,
    viewer: SELF_ID,
    generated_at_unix_ms: 1_700_000_005_000,
    truncated: false,
    boundaries: [
      {
        network_id: "home",
        name: "Home",
        member_count: 2,
        visible_member_count: 2,
        viewer_role: "owner",
        capabilities: ["relay"],
      },
      {
        network_id: "studio",
        name: "Studio",
        member_count: 1,
        visible_member_count: 1,
        viewer_role: "administrator",
        capabilities: ["exit"],
      },
    ],
    clusters: [],
    nodes: [
      {
        node_id: SELF_ID,
        display_name: "Local node",
        health: "healthy",
        is_self: true,
        capabilities: ["relay"],
        networks: [
          {
            network_id: "home",
            health: "healthy",
            presence,
            role: "owner",
            status: "active",
            capabilities: ["relay"],
          },
        ],
      },
      {
        node_id: BOB_ID,
        display_name: "Bob",
        health: "degraded",
        is_self: false,
        capabilities: ["exit"],
        networks: [
          {
            network_id: "home",
            health: "degraded",
            presence,
            role: "member",
            status: "degraded",
            capabilities: ["exit"],
          },
          {
            network_id: "studio",
            health: "healthy",
            presence,
            role: "member",
            status: "active",
            capabilities: ["exit"],
          },
        ],
      },
      {
        node_id: CAROL_ID,
        display_name: "Carol",
        health: "unreachable",
        is_self: false,
        capabilities: ["coordinator"],
        networks: [
          {
            network_id: "studio",
            health: "unreachable",
            presence: {
              state: "unknown",
              source: "not_observed",
              observed_at_unix_ms: null,
            },
            role: "administrator",
            status: "suspended",
            capabilities: ["coordinator"],
          },
        ],
      },
    ],
    connections: [
      {
        connection_id: "connection-self-bob",
        from: SELF_ID,
        to: BOB_ID,
        networks: ["home", "studio"],
        observed_at_unix_ms: 1_700_000_004_000,
        state: "active",
      },
      {
        connection_id: "connection-bob-carol",
        from: BOB_ID,
        to: CAROL_ID,
        networks: ["studio"],
        observed_at_unix_ms: 1_700_000_003_000,
        state: "idle",
      },
    ],
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
        node_id: SELF_ID,
        display_name: "Local node",
        identity_fingerprint: "fingerprint-self",
        role: "owner",
        groups: ["owner"],
        capabilities: ["relay"],
        status: "active",
        lifecycle_state: "active",
        revision: 11,
        access_epoch: 4,
        available_actions: [],
        presence: {
          state: "online",
          source: "live_observation",
          observed_at_unix_ms: 1_700_000_000_000,
        },
      },
      {
        node_id: BOB_ID,
        display_name: "Bob",
        identity_fingerprint: "fingerprint-bob",
        role: "member",
        groups: ["member"],
        capabilities: ["exit"],
        status: "degraded",
        lifecycle_state: "degraded",
        revision: 12,
        access_epoch: 4,
        available_actions: ["suspend", "remove"],
        presence: {
          state: "online",
          source: "live_observation",
          observed_at_unix_ms: 1_700_000_000_000,
        },
      },
    ],
  };
}

async function startFixtureServer(manifest, fixture, sharedAssets) {
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
          const networkId = String(envelope?.request?.payload?.network_id ?? "home");
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
        jsonResponse(response, {
          schema_version: 1,
          request_id: 1,
          server_time: FIXED_SERVER_TIME,
          snapshot: fixture.snapshot,
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
  if (!address || typeof address === "string") throw new Error("fixture server did not bind TCP");
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

async function activeElement(session) {
  return evaluate(
    session,
    `(() => {
      const element = document.activeElement;
      if (!element) return null;
      return {
        tag: element.tagName.toLowerCase(),
        role: element.getAttribute("role"),
        text: element.textContent?.trim().slice(0, 180) ?? "",
        aria_label: element.getAttribute("aria-label"),
        test_id: element.getAttribute("data-testid"),
        node_id: element.getAttribute("data-node-id"),
        class_name: element.getAttribute("class"),
      };
    })()`,
  );
}

async function waitForActive(session, predicate, description, timeoutMs = 10_000) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    const info = await activeElement(session);
    if (await evaluate(
      session,
      `(() => { const el = document.activeElement; return Boolean(${predicate}); })()`,
    )) {
      return info;
    }
    await sleep(50);
  }
  throw new Error(`timed out waiting for focus on ${description}: ${JSON.stringify(await activeElement(session))}`);
}

async function pressKey(session, key, options = {}) {
  const definitions = {
    Tab: { code: "Tab", virtualKeyCode: 9 },
    Enter: { code: "Enter", virtualKeyCode: 13, text: "\r" },
    Escape: { code: "Escape", virtualKeyCode: 27 },
    " ": { code: "Space", virtualKeyCode: 32, text: " " },
    ArrowLeft: { code: "ArrowLeft", virtualKeyCode: 37 },
    ArrowUp: { code: "ArrowUp", virtualKeyCode: 38 },
    ArrowRight: { code: "ArrowRight", virtualKeyCode: 39 },
    ArrowDown: { code: "ArrowDown", virtualKeyCode: 40 },
    Home: { code: "Home", virtualKeyCode: 36 },
    End: { code: "End", virtualKeyCode: 35 },
    F10: { code: "F10", virtualKeyCode: 121 },
  };
  const definition = definitions[key];
  if (!definition) throw new Error(`unsupported key ${key}`);
  const modifiers = options.shift ? 8 : 0;
  const common = {
    key,
    code: definition.code,
    modifiers,
    windowsVirtualKeyCode: definition.virtualKeyCode,
    nativeVirtualKeyCode: definition.virtualKeyCode,
    ...(definition.text ? { text: definition.text, unmodifiedText: definition.text } : {}),
  };
  await session.send("Input.dispatchKeyEvent", { type: "keyDown", ...common });
  await session.send("Input.dispatchKeyEvent", { type: "keyUp", ...common });
}

async function tabTo(session, predicate, description, maxTabs = 100) {
  for (let index = 0; index < maxTabs; index += 1) {
    await pressKey(session, "Tab");
    const matched = await evaluate(
      session,
      `(() => { const el = document.activeElement; return Boolean(${predicate}); })()`,
    );
    if (matched) return activeElement(session);
  }
  throw new Error(`could not reach ${description} with Tab; focus is ${JSON.stringify(await activeElement(session))}`);
}

async function navigateToTopologyWithKeyboard(session) {
  await tabTo(
    session,
    `el?.classList.contains("module-button") && el.textContent?.trim() === "Networks"`,
    "Networks module",
  );
  const focusedNetworks = await activeElement(session);
  assert.equal(focusedNetworks?.text, "Networks");
  await pressKey(session, "Enter");
  // The page switcher is a <details> disclosure with menuitemradio entries, not a
  // tablist: the old tab-based wait could never match once the shell switched to
  // this control.
  await waitFor(
    session,
    `Boolean(document.querySelector(".page-switcher"))`,
    "page switcher",
  );
  await tabTo(
    session,
    `el?.tagName === "SUMMARY" && Boolean(el.closest(".page-switcher"))`,
    "page switcher trigger",
  );
  await pressKey(session, "Enter");
  await waitFor(
    session,
    `[...document.querySelectorAll('.page-switcher-menu [role="menuitemradio"]')].some((element) => element.textContent?.trim() === "Topology")`,
    "Topology page entry",
  );
  await tabTo(
    session,
    `el?.getAttribute("role") === "menuitemradio" && el.textContent?.trim() === "Topology"`,
    "Topology page entry",
  );
  await pressKey(session, "Enter");
  await waitFor(
    session,
    `Boolean(document.querySelector(".topology-view"))`,
    "topology view",
  );
  await waitForActive(
    session,
    `el?.id === "page-content"`,
    "page content after topology navigation",
  );
}

async function dispatchTouchLongPress(session, selector) {
  const point = await evaluate(
    session,
    `(async () => {
      const element = document.querySelector(${JSON.stringify(selector)});
      if (!element) return null;
      element.scrollIntoView({ block: "center", inline: "center" });
      await new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve)));
      const rect = element.getBoundingClientRect();
      const x = rect.left + rect.width / 2;
      const y = rect.top + rect.height / 2;
      const hit = document.elementFromPoint(x, y);
      if (!hit || !element.contains(hit)) {
        throw new Error("touch target is not visible after scrolling into view");
      }
      return { x, y };
    })()`,
  );
  if (!point) throw new Error(`touch target not found: ${selector}`);
  await session.send("Input.dispatchTouchEvent", {
    type: "touchStart",
    touchPoints: [{ x: point.x, y: point.y, radiusX: 2, radiusY: 2, force: 1 }],
  });
  await sleep(560);
  await session.send("Input.dispatchTouchEvent", {
    type: "touchEnd",
    touchPoints: [],
  });
}

async function main() {
  if (!process.argv.includes("--skip-build")) {
    await run("npm", ["run", "build"], { cwd: WEBUI_ROOT });
  }
  await mkdir(REPORT_ROOT, { recursive: true });
  const workspace = await mkdtemp(join(tmpdir(), "fuckipv6-topology-e2e-"));
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
    fixture.snapshot.topology = topologyFixture();
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

    const targets = await fetch(`http://127.0.0.1:${port}/json/list`).then((response) => response.json());
    const target = targets.find((candidate) => candidate.type === "page");
    if (!target) throw new Error("Chrome has no page target");
    cdp = await CdpSession.connect(target.webSocketDebuggerUrl);

    await cdp.send("Page.enable");
    await cdp.send("Runtime.enable");
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
    await cdp.send("Page.navigate", { url: `${fixtureServer.origin}/` });
    await waitForReady(cdp);

    await navigateToTopologyWithKeyboard(cdp);
    checks.push("keyboard-only navigation reaches Networks > Topology");
    assert.equal(
      await evaluate(cdp, `document.querySelector(".topology-view")?.dataset.view`),
      "graph",
    );
    // The graph renders the members of the selected network: `home` has the local
    // node and Bob, while Carol belongs to `studio` only. Layout placements are
    // produced per visible network, so a node from another network is absent here
    // by design rather than by a rendering bug.
    assert.equal(
      await evaluate(cdp, `document.querySelectorAll("g.topology-node[role=option]").length`),
      2,
    );
    const selfAriaLabel = await evaluate(
      cdp,
      `document.querySelector('g.topology-node[data-node-id="${SELF_ID}"]')?.getAttribute("aria-label")`,
    );
    assert.match(selfAriaLabel, /Local node/);
    assert.match(selfAriaLabel, /Healthy/);
    assert.match(selfAriaLabel, /Relay/);
    assert.match(selfAriaLabel, /Connections: Active 1/);
    checks.push("graph renders the selected network's members only");
    checks.push("graph exposes object name, capability, network and relationship summary");

    assert.equal(
      await evaluate(cdp, `matchMedia("(prefers-reduced-motion: reduce)").matches`),
      true,
    );
    const reducedTransitionDuration = await evaluate(
      cdp,
      `getComputedStyle(document.querySelector(".topology-control-button")).transitionDuration`,
    );
    const reducedDurationMilliseconds = reducedTransitionDuration.endsWith("ms")
      ? Number.parseFloat(reducedTransitionDuration)
      : Number.parseFloat(reducedTransitionDuration) * 1_000;
    assert.ok(
      Number.isFinite(reducedDurationMilliseconds) && reducedDurationMilliseconds <= 0.01,
      `expected reduced transition duration, received ${reducedTransitionDuration}`,
    );
    checks.push("reduced-motion preference disables topology motion");

    await tabTo(
      cdp,
      `el?.matches('button[aria-label="Zoom topology in"]')`,
      "zoom-in control",
    );
    await pressKey(cdp, "Enter");
    await waitFor(
      cdp,
      `document.querySelector('[data-testid="topology-zoom-status"]')?.textContent?.trim() === "125%"`,
      "zoom status at 125 percent",
    );
    await tabTo(
      cdp,
      `el?.matches('button[aria-label="Reset topology zoom"]')`,
      "reset-zoom control",
    );
    await pressKey(cdp, "Enter");
    await waitFor(
      cdp,
      `document.querySelector('[data-testid="topology-zoom-status"]')?.textContent?.trim() === "100%"`,
      "zoom status reset to 100 percent",
    );
    // Arrow handling is scoped to the graph, so focus must be inside it first.
    await tabTo(
      cdp,
      `el?.matches('g.topology-node[data-node-id="${SELF_ID}"]')`,
      "self node in the graph",
    );
    await pressKey(cdp, "ArrowRight");
    await waitForActive(
      cdp,
      `el?.matches('g.topology-node[data-node-id="${BOB_ID}"]')`,
      "next graph node after ArrowRight",
    );
    // Space before Enter: Enter opens the detail drawer, which moves focus into it, so a
    // Space pressed afterwards reaches the drawer rather than the node.
    await pressKey(cdp, " ");
    await waitFor(
      cdp,
      `document.querySelector('g.topology-node[data-node-id="${BOB_ID}"]')?.getAttribute("data-selected") === "true"`,
      "Bob selection after Space",
    );
    await pressKey(cdp, "Enter");
    await waitFor(
      cdp,
      `document.querySelector('[data-testid="topology-node-detail"] h2')?.textContent?.trim() === "Bob"`,
      "Bob node detail after Enter",
    );
    // Close the drawer so the keyboard walk returns to the graph; the drawer's trap
    // restores focus to the node it was opened from.
    await pressKey(cdp, "Escape");
    await waitForActive(
      cdp,
      `el?.matches('g.topology-node[data-node-id="${BOB_ID}"]')`,
      "focus restored to the node after closing details",
    );
    checks.push("graph roving focus, Enter open and Space selection work by keyboard");
    // Two-pass sweep: locate-current is disabled until a node has a focus target, and a
    // disabled button is not in the tab order, so node selection must come first.
    await tabTo(
      cdp,
      `el?.matches('button[aria-label="Locate current node"]')`,
      "locate-current control",
    );
    await pressKey(cdp, "Enter");
    await waitForActive(
      cdp,
      `el?.matches('g.topology-node[data-node-id="${SELF_ID}"]')`,
      "current node after keyboard locate",
    );
    // The layout controls live inside a collapsed <details class="topology-options">,
    // so the disclosure must be opened with the keyboard first.
    await tabTo(
      cdp,
      `el?.tagName === "SUMMARY" && Boolean(el.closest(".topology-options"))`,
      "topology options summary",
    );
    await pressKey(cdp, "Enter");
    await waitFor(
      cdp,
      `document.querySelector(".topology-options")?.hasAttribute("open") === true`,
      "open topology options",
    );
    // The freeze control carries its label as visible text rather than aria-label,
    // so it has to be matched by name instead of by attribute.
    await tabTo(
      cdp,
      `el?.tagName === "BUTTON" && (el.textContent || "").trim() === "Freeze live layout"`,
      "freeze control",
    );
    await pressKey(cdp, "Enter");
    await waitFor(
      cdp,
      `[...document.querySelectorAll("button")].some((button) => (button.textContent || "").trim() === "Resume live layout" && button.getAttribute("aria-pressed") === "true")`,
      "frozen topology state",
    );
    checks.push("zoom, reset, freeze and locate-current work with keyboard only");


    await pressKey(cdp, "F10", { shift: true });
    await waitFor(
      cdp,
      `Boolean(document.querySelector('[data-testid="topology-node-menu"]'))`,
      "node menu after Shift+F10",
    );
    await waitForActive(
      cdp,
      `el?.getAttribute("role") === "menuitem"`,
      "first enabled menu item",
    );
    assert.equal((await activeElement(cdp))?.text, "Open node details");
    await pressKey(cdp, "ArrowDown");
    await waitForActive(
      cdp,
      `el?.getAttribute("role") === "menuitem" && el?.textContent?.includes("batch selection")`,
      "second menu item",
    );
    await pressKey(cdp, "Home");
    await waitForActive(
      cdp,
      `el?.getAttribute("role") === "menuitem" && el?.textContent?.trim() === "Open node details"`,
      "menu Home target",
    );
    await pressKey(cdp, "End");
    await waitForActive(
      cdp,
      `el?.getAttribute("role") === "menuitem" && el?.textContent?.trim() === "Pin current layout"`,
      "menu End target",
    );
    await pressKey(cdp, "Escape");
    await waitFor(
      cdp,
      `!document.querySelector('[data-testid="topology-node-menu"]')`,
      "menu close after Escape",
    );
    await waitForActive(
      cdp,
      `el?.matches('g.topology-node[data-node-id="${BOB_ID}"]')`,
      "graph node restored after menu Escape",
    );
    await pressKey(cdp, "F10", { shift: true });
    await waitForActive(
      cdp,
      `el?.getAttribute("role") === "menuitem"`,
      "menu reopened for Enter action",
    );
    await pressKey(cdp, "Enter");
    await waitFor(
      cdp,
      `!document.querySelector('[data-testid="topology-node-menu"]')`,
      "menu close after activating Open node details",
    );
    await waitFor(
      cdp,
      `document.querySelector('[data-testid="topology-node-detail"] h2')?.textContent?.trim() === "Bob"`,
      "node detail opened from menu",
    );
    checks.push("Shift+F10 menu supports Arrow/Home/End, Escape and Enter");

    await cdp.send("Emulation.setDeviceMetricsOverride", {
      ...MOBILE_VIEWPORT,
      screenWidth: MOBILE_VIEWPORT.width,
      screenHeight: MOBILE_VIEWPORT.height,
      mobile: true,
    });
    await cdp.send("Emulation.setTouchEmulationEnabled", {
      enabled: true,
      maxTouchPoints: 1,
    });
    await cdp.send("Page.reload", { ignoreCache: true });
    await waitForReady(cdp);
    await navigateToTopologyWithKeyboard(cdp);
    assert.equal(
      await evaluate(cdp, `document.querySelector(".topology-view")?.dataset.view`),
      "list",
    );
    await waitFor(
      cdp,
      `document.querySelectorAll("tbody tr[data-node-id]").length === 3`,
      "narrow topology list rows",
    );
    checks.push("narrow viewport defaults to the searchable object list");

    await tabTo(
      cdp,
      `el?.matches('input[type="search"][aria-controls="topology-object-navigation"]')`,
      "topology search field",
    );
    await cdp.send("Input.insertText", { text: "Bob" });
    await waitFor(
      cdp,
      `document.querySelectorAll("tbody tr[data-node-id]").length === 1 &&
       document.querySelector("tbody tr[data-node-id]")?.textContent?.includes("Bob")`,
      "topology list filtered to Bob",
    );
    assert.equal(
      await evaluate(
        cdp,
        `document.querySelector(".topology-filter-count")?.textContent?.trim()`,
      ),
      "Showing 1 of 3 nodes",
    );
    checks.push("list filtering exposes a live visible/total result count");

    await tabTo(
      cdp,
      `el?.matches('tr[data-node-id]')`,
      "filtered topology row",
    );
    await pressKey(cdp, "Enter");
    await waitFor(
      cdp,
      `document.querySelector('[data-testid="topology-node-detail"] h2')?.textContent?.trim() === "Bob"`,
      "Bob detail from list Enter",
    );
    await pressKey(cdp, "F10", { shift: true });
    await waitFor(
      cdp,
      `Boolean(document.querySelector('[data-testid="topology-node-menu"]'))`,
      "list row menu after Shift+F10",
    );
    await pressKey(cdp, "Escape");
    await waitForActive(
      cdp,
      `el?.matches('tr[data-node-id]')`,
      "list row restored after menu Escape",
    );
    checks.push("list rows retain keyboard focus and open the same node menu");

    await dispatchTouchLongPress(cdp, `tr[data-node-id="${BOB_ID}"]`);
    await waitFor(
      cdp,
      `Boolean(document.querySelector('[data-testid="topology-node-menu"]'))`,
      "touch long-press menu",
      5_000,
    );
    await pressKey(cdp, "Escape");
    checks.push("touch long-press opens the row context menu");

    await writeFile(
      join(REPORT_ROOT, "report.json"),
      `${JSON.stringify(
        {
          schema_version: 1,
          surface: "webui",
          fixture_revision: fixture.snapshot.revision,
          topology_revision: fixture.snapshot.topology.revision,
          chrome: (await cdp.send("Browser.getVersion")).product,
          platform: process.platform,
          node: process.version,
          viewports: { desktop: DESKTOP_VIEWPORT, mobile: MOBILE_VIEWPORT },
          reduced_motion: "reduce",
          checks,
        },
        null,
        2,
      )}\n`,
    );
    process.stdout.write(`topology keyboard E2E passed (${checks.length} checks)\n`);
    for (const check of checks) process.stdout.write(`- ${check}\n`);
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
