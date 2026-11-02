#!/usr/bin/env node

import { spawn, spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { mkdtemp, mkdir, readFile, rm, writeFile } from "node:fs/promises";
import { createServer } from "node:http";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const WEBUI_ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const REPO_ROOT = resolve(WEBUI_ROOT, "..");
const DIST_ROOT = join(REPO_ROOT, "crates/client/src/webui/dist");
const FIXTURE_ROOT = join(REPO_ROOT, "contracts/typescript/fixtures");
const OUTPUT_ROOT = resolve(
  process.env.VISUAL_OUTPUT_DIR ?? join(REPO_ROOT, "target/visual-regression/webui"),
);
const VIEWPORT = { width: 1440, height: 900, deviceScaleFactor: 2 };
const FIXED_SERVER_TIME = "2026-01-01T00:00:05Z";
const WEBSOCKET_GUID = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

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

function jsonResponse(response, value) {
  response.writeHead(200, {
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

async function startFixtureServer(manifest, fixture, sharedAssets) {
  const appHtml = await readFile(join(DIST_ROOT, "index.html"));
  const appJs = await readFile(join(DIST_ROOT, "assets/app.js"));
  const appCss = await readFile(join(DIST_ROOT, "assets/app.css"));
  const webuiCss = await readFile(join(sharedAssets, "webui.css"));
  const i18nJs = await readFile(join(sharedAssets, "i18n.js"));
  const sockets = new Set();

  const server = createServer((request, response) => {
    const url = new URL(request.url ?? "/", "http://127.0.0.1");
    switch (url.pathname) {
      case "/":
        fileResponse(response, appHtml, "text/html; charset=utf-8");
        return;
      case "/assets/app.js":
        fileResponse(response, appJs, "text/javascript; charset=utf-8");
        return;
      case "/assets/app.css":
        fileResponse(response, appCss, "text/css; charset=utf-8");
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
      default:
        response.writeHead(404, { "content-type": "text/plain; charset=utf-8" });
        response.end("not found");
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
  });
  if (result.exceptionDetails) {
    throw new Error(result.exceptionDetails.exception?.description ?? result.exceptionDetails.text);
  }
  return result.result.value;
}

async function waitForReady(session, timeoutMs = 20_000) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    const state = await evaluate(
      session,
      `(() => {
        const body = document.body;
        const shell = document.querySelector(".app-shell");
        const status = document.querySelector(".nav-footer");
        return {
          ready: Boolean(body && shell && status && status.textContent.includes("Live")),
          body: body?.innerText?.slice(0, 240) ?? "",
          font: body ? getComputedStyle(body).fontFamily : "",
        };
      })()`,
    );
    if (state.ready) {
      if (!state.font.includes("Inter")) throw new Error(`Inter is not the active font: ${state.font}`);
      await evaluate(session, "document.fonts.ready.then(() => true)");
      await new Promise((resolvePromise) => setTimeout(resolvePromise, 250));
      return;
    }
    await new Promise((resolvePromise) => setTimeout(resolvePromise, 50));
  }
  throw new Error("WebUI product shell did not become ready");
}

function pngDimensions(buffer) {
  if (buffer.toString("ascii", 1, 4) !== "PNG") throw new Error("capture is not PNG");
  return { width: buffer.readUInt32BE(16), height: buffer.readUInt32BE(20) };
}

async function main() {
  if (!process.argv.includes("--skip-build")) {
    await run("npm", ["run", "build"], { cwd: WEBUI_ROOT });
  }
  await mkdir(OUTPUT_ROOT, { recursive: true });
  const workspace = await mkdtemp(join(tmpdir(), "fuckipv6-visual-"));
  const chromeProfile = join(workspace, "chrome-profile");
  const sharedAssets = join(workspace, "shared-assets");
  let fixtureServer;
  let chrome;
  let cdp;
  try {
    const manifest = JSON.parse(await readFile(join(FIXTURE_ROOT, "node-manifest.json"), "utf8"));
    const fixture = JSON.parse(
      await readFile(join(FIXTURE_ROOT, "product-visual-snapshot.json"), "utf8"),
    );
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
        "--force-color-profile=srgb",
        "--font-render-hinting=none",
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
    await cdp.send("Emulation.setDeviceMetricsOverride", { ...VIEWPORT, screenWidth: VIEWPORT.width, screenHeight: VIEWPORT.height, mobile: false });
    await cdp.send("Emulation.setLocaleOverride", { locale: "en-US" });
    await cdp.send("Emulation.setTimezoneOverride", { timezoneId: "UTC" });
    await cdp.send("Page.navigate", { url: `${fixtureServer.origin}/` });
    await waitForReady(cdp);

    const screenshot = await cdp.send("Page.captureScreenshot", {
      format: "png",
      fromSurface: true,
      captureBeyondViewport: false,
    });
    const image = Buffer.from(screenshot.data, "base64");
    const dimensions = pngDimensions(image);
    const expected = {
      width: VIEWPORT.width * VIEWPORT.deviceScaleFactor,
      height: VIEWPORT.height * VIEWPORT.deviceScaleFactor,
    };
    if (dimensions.width !== expected.width || dimensions.height !== expected.height) {
      throw new Error(
        `unexpected screenshot dimensions ${dimensions.width}x${dimensions.height}; expected ${expected.width}x${expected.height}`,
      );
    }

    const semantics = await evaluate(
      cdp,
      `(() => {
        const text = (element) => element?.textContent?.trim() ?? "";
        const fontFamily = getComputedStyle(document.body).fontFamily
          .split(",")[0]
          .trim()
          .replace(/^["']|["']$/g, "");
        return {
          title: document.title,
          locale: document.documentElement.lang,
          font_family: fontFamily,
          viewport: { width: innerWidth, height: innerHeight, devicePixelRatio },
          modules: [...document.querySelectorAll(".module-button")].map(text),
          page_tabs: [...document.querySelectorAll('[role="tab"]')].map(text),
          primary_status: {
            label: text(document.querySelector(".top-status strong")),
            detail: text(document.querySelector(".top-status span:last-child")),
          },
          page_heading: text(document.querySelector(".workspace h1")),
          action_panel: text(document.querySelector(".actions-panel .panel-title")),
          actions: [...document.querySelectorAll(".actions-panel .action-card-head span")].map(text),
          shortcuts: [...document.querySelectorAll('[data-testid="overview-shortcuts"] .overview-shortcuts > *')].map(
            (element) => text(element.querySelector("span") ?? element),
          ),
          network_statuses: [...document.querySelectorAll('[data-testid="overview-networks"] .status-pill')].map(text),
          geometry: Object.fromEntries(
            [
              ["sidebar", ".primary-nav"],
              ["topbar", ".global-bar"],
              ["toolbar", ".local-toolbar"],
              ["workspace", ".workspace"],
              ["content", ".page-content-stack"],
              ["page_heading", ".page-heading"],
              ["hero", ".hero-status"],
              ["hero_copy", ".hero-status-copy"],
              ["networks", '[data-testid="overview-networks"]'],
              ["networks_heading", '[data-testid="overview-networks"] .card-heading'],
              ["network_first", '[data-testid="overview-networks"] .compact-row'],
              ["network_second", '[data-testid="overview-networks"] .compact-row:nth-of-type(2)'],
              ["activity", '[data-testid="overview-activity"]'],
              ["activity_heading", '[data-testid="overview-activity"] .card-heading'],
              ["activity_empty", '[data-testid="overview-activity"] .empty-state'],
              ["shortcuts", '[data-testid="overview-shortcuts"]'],
              ["shortcuts_heading", '[data-testid="overview-shortcuts"] .card-heading'],
              ["shortcuts_grid", '[data-testid="overview-shortcuts"] .overview-shortcuts'],
              ["shortcut_first", '[data-testid="overview-shortcuts"] .overview-shortcuts > :first-child'],
              ["actions", ".actions-panel"],
              ["action_title", ".actions-panel .panel-title"],
              ["action_first", ".actions-panel .action-card-head"],
            ].map(([name, selector]) => {
              const element = document.querySelector(selector);
              if (!element) return [name, null];
              const bounds = element.getBoundingClientRect();
              const style = getComputedStyle(element);
              return [
                name,
                {
                  x: bounds.x,
                  y: bounds.y,
                  width: bounds.width,
                  height: bounds.height,
                  background_color: style.backgroundColor,
                  background_image: style.backgroundImage,
                  border_color: style.borderColor,
                  border_radius: style.borderRadius,
                  color: style.color,
                  font_size: style.fontSize,
                  font_weight: style.fontWeight,
                },
              ];
            }),
          ),
          overview_network_rows: [...document.querySelectorAll(".compact-row")].map((element) => {
            const bounds = element.getBoundingClientRect();
            const cardBounds = element.closest(".surface-card")?.getBoundingClientRect();
            const style = getComputedStyle(element);
            return {
              width: bounds.width,
              card_width: cardBounds?.width ?? 0,
              background_color: style.backgroundColor,
              background_image: style.backgroundImage,
            };
          }),
          hierarchy: [...document.querySelectorAll(".primary-nav, .global-bar, .local-toolbar, .workspace > *")].map(
            (element) => ({
              tag: element.tagName.toLowerCase(),
              classes: [...element.classList],
              role: element.getAttribute("role"),
              label: element.getAttribute("aria-label"),
            }),
          ),
        };
      })()`,
    );

    const expectedModules = ["Overview", "Nodes", "Networks", "Activity", "Settings"];
    if (JSON.stringify(semantics.modules) !== JSON.stringify(expectedModules)) {
      throw new Error(`unexpected module order: ${JSON.stringify(semantics.modules)}`);
    }
    if (JSON.stringify(semantics.page_tabs) !== JSON.stringify(["Overview"])) {
      throw new Error(`unexpected page tabs: ${JSON.stringify(semantics.page_tabs)}`);
    }
    if (
      semantics.primary_status.label !== "Running normally" ||
      semantics.page_heading !== "Node overview" ||
      semantics.action_panel !== "Shared actions"
    ) {
      throw new Error("visual fixture did not render the expected English overview state");
    }
    if (semantics.overview_network_rows.length !== 2) {
      throw new Error("overview fixture did not render both compact network rows");
    }
    for (const row of semantics.overview_network_rows) {
      if (row.width < row.card_width - 40) {
        throw new Error(`compact network row did not fill its card: ${JSON.stringify(row)}`);
      }
      if (row.background_color !== "rgba(0, 0, 0, 0)" || row.background_image !== "none") {
        throw new Error(`compact network row retained native button paint: ${JSON.stringify(row)}`);
      }
    }

    const imagePath = join(OUTPUT_ROOT, "overview.png");
    const metadataPath = join(OUTPUT_ROOT, "overview.json");
    await writeFile(imagePath, image);
    await writeFile(
      metadataPath,
      `${JSON.stringify(
        {
          schema_version: 1,
          surface: "webui",
          fixture_revision: fixture.snapshot.revision,
          fixture_locale: fixture.locale,
          chrome: (await cdp.send("Browser.getVersion")).product,
          platform: process.platform,
          node: process.version,
          dimensions,
          sha256: createHash("sha256").update(image).digest("hex"),
          semantics,
        },
        null,
        2,
      )}\n`,
    );
    process.stdout.write(`${imagePath}\n${metadataPath}\n`);
  } catch (error) {
    if (chrome && chrome.exitCode === null) {
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
