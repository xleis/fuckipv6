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
        if (url.pathname.startsWith("/assets/")) {
          const assetName = url.pathname.slice("/assets/".length);
          if (
            assetName.length > 0 &&
            !assetName.includes("/") &&
            !assetName.includes("\\") &&
            assetName !== "." &&
            assetName !== ".."
          ) {
            readFile(join(DIST_ROOT, "assets", assetName))
              .then((body) => {
                const contentType = assetName.endsWith(".js")
                  ? "text/javascript; charset=utf-8"
                  : "application/octet-stream";
                fileResponse(response, body, contentType);
              })
              .catch(() => {
                response.writeHead(404, {
                  "content-type": "text/plain; charset=utf-8",
                });
                response.end("not found");
              });
            return;
          }
        }
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

async function waitForExpression(session, expression, description, timeoutMs = 10_000) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (await evaluate(session, expression)) return;
    await new Promise((resolvePromise) => setTimeout(resolvePromise, 50));
  }
  throw new Error(`timed out waiting for ${description}`);
}

async function clickSelector(session, selector) {
  const clicked = await evaluate(
    session,
    `(() => {
      const element = document.querySelector(${JSON.stringify(selector)});
      if (!(element instanceof HTMLElement)) return false;
      element.click();
      return true;
    })()`,
  );
  if (!clicked) throw new Error(`could not find ${selector}`);
}

async function clickText(session, selector, text) {
  const clicked = await evaluate(
    session,
    `(() => {
      const element = [...document.querySelectorAll(${JSON.stringify(selector)})]
        .find((candidate) => candidate.textContent?.trim() === ${JSON.stringify(text)});
      if (!(element instanceof HTMLElement)) return false;
      element.click();
      return true;
    })()`,
  );
  if (!clicked) throw new Error(`could not find ${selector} containing ${text}`);
}

async function selectModulePage(session, moduleLabel, pageLabel) {
  await clickText(session, ".module-button", moduleLabel);
  await waitForExpression(
    session,
    `document.querySelector('.module-button[aria-current="page"]')?.textContent?.trim() === ${JSON.stringify(moduleLabel)}`,
    `${moduleLabel} module selection`,
  );
  const currentLabel = await evaluate(
    session,
    `document.querySelector(".page-switcher-trigger")?.textContent?.trim() ?? ""`,
  );
  if (currentLabel !== pageLabel) {
    await clickSelector(session, ".page-switcher-trigger");
    await waitForExpression(
      session,
      `document.querySelector('.page-switcher[open] [role="menuitemradio"]') !== null`,
      `${moduleLabel} page switcher`,
    );
    await clickText(session, '.page-switcher-menu [role="menuitemradio"]', pageLabel);
    await waitForExpression(
      session,
      `document.querySelector(".page-switcher-trigger")?.textContent?.trim() === ${JSON.stringify(pageLabel)}`,
      `${pageLabel} page selection`,
    );
  }
}

async function captureScreenshot(session, filename) {
  const screenshot = await session.send("Page.captureScreenshot", {
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
  const path = join(OUTPUT_ROOT, filename);
  await writeFile(path, image);
  return { image, dimensions, path };
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
    const topologyScenarios = JSON.parse(
      await readFile(join(FIXTURE_ROOT, "topology-presentation-scenarios.json"), "utf8"),
    );
    const topologyScenario = topologyScenarios.scenarios.find(
      (scenario) => scenario.id === "multi-network",
    );
    if (!topologyScenario) throw new Error("multi-network topology visual fixture is missing");
    fixture.snapshot.topology = topologyScenario.topology;
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

    const overviewCapture = await captureScreenshot(cdp, "overview.png");
    const { image, dimensions } = overviewCapture;

    const semantics = await evaluate(
      cdp,
      `(() => {
        const text = (element) => element?.textContent?.trim() ?? "";
        const fontFamily = getComputedStyle(document.body).fontFamily
          .split(",")[0]
          .trim()
          .replace(/^["']|["']$/g, "");
        const pageOptions = [...document.querySelectorAll('.page-switcher-menu [role="menuitemradio"]')]
          .map(text)
          .filter(Boolean);
        const currentPage =
          text(document.querySelector(".page-switcher-trigger")) ||
          text(document.querySelector('.module-button[aria-current="page"]')) ||
          text(document.querySelector(".workspace h1"));
        return {
          title: document.title,
          locale: document.documentElement.lang,
          font_family: fontFamily,
          viewport: { width: innerWidth, height: innerHeight, devicePixelRatio },
          modules: [...document.querySelectorAll(".module-button")].map(text),
          page_switcher: {
            current: currentPage,
            options: pageOptions.length > 0 ? pageOptions : [currentPage],
          },
          topbar_search: {
            placeholder:
              document.querySelector(".global-search-topbar input")?.getAttribute("placeholder") ??
              "",
          },
          network_picker: document.querySelector(".network-picker")
            ? text(document.querySelector(".network-picker"))
            : null,
          primary_status: {
            label: text(document.querySelector(".top-status strong")),
            detail: text(document.querySelector(".top-status span:last-child")),
          },
          page_heading: text(document.querySelector(".workspace h1")),
          shell_regions: [...document.querySelectorAll("[data-ui-region]")].map(
            (element) => element.getAttribute("data-ui-region"),
          ),
          actions: [...document.querySelectorAll(".page-command-menu-item strong")].map(text),
          persistent_action_surface: Boolean(
            document.querySelector(
              '[data-ui-region="action-panel"], .actions-panel, .visual-actions',
            ),
          ),
          shortcuts: [...document.querySelectorAll('[data-testid="overview-shortcuts"] .overview-shortcuts > *')].map(
            (element) => text(element.querySelector("span") ?? element),
          ),
          network_statuses: [...document.querySelectorAll('[data-testid="overview-networks"] .status-pill')].map(text),
          geometry: Object.fromEntries(
            [
              ["sidebar_icon_rail", ".primary-nav"],
              ["topbar", ".global-bar"],
              ["toolbar", ".local-toolbar"],
              ["workspace", ".workspace"],
              ["overview_stack", ".overview-stack"],
              ["page_heading", ".page-heading"],
              ["hero", ".hero-status"],
              ["hero_copy", ".hero-status-copy"],
              ["global_context", ".global-context"],
              ["top_status_group", ".top-status-group"],
              ["primary_status", '[data-testid="top-lifecycle-status"]'],
              ["traffic_session_status", '[data-testid="top-traffic-status"]'],
              ["network_picker", ".network-picker"],
              ["network_picker_label", ".network-picker > span"],
              ["network_picker_trigger", ".network-picker select"],
              ["language_control", ".language-control"],
              ["global_actions", ".global-actions"],
              ["traffic_session", '[data-testid="overview-traffic-session"]'],
              ["traffic_heading", '[data-testid="overview-traffic-session"] .card-heading'],
              ["traffic_summary", '[data-testid="overview-traffic-session"] .traffic-session-summary'],
              ["traffic_meta", '[data-testid="overview-traffic-session"] .traffic-session-meta'],
              ["traffic_validation_title", '[data-testid="overview-traffic-session"] h4:nth-of-type(1)'],
              ["traffic_validation", '[data-testid="overview-traffic-session"] > .traffic-session-empty:nth-of-type(1)'],
              ["traffic_events_title", '[data-testid="overview-traffic-session"] h4:nth-of-type(2)'],
              ["traffic_events", '[data-testid="overview-traffic-session"] > .traffic-session-empty:nth-of-type(2)'],
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
          hierarchy: [...document.querySelectorAll(".primary-nav, .global-bar, .workspace > *")].map(
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
    if (
      semantics.page_switcher.current !== "Overview" ||
      semantics.topbar_search.placeholder.length === 0 ||
      semantics.network_picker !== null
    ) {
      throw new Error(`unexpected topbar hierarchy: ${JSON.stringify({
        page_switcher: semantics.page_switcher,
        topbar_search: semantics.topbar_search,
        network_picker: semantics.network_picker,
      })}`);
    }
    if (
      semantics.primary_status.label !== "Running normally" ||
      semantics.page_heading !== "Node overview"
    ) {
      throw new Error("visual fixture did not render the expected English overview state");
    }
    if (
      JSON.stringify(semantics.shell_regions) !==
      JSON.stringify(["sidebar", "topbar", "content"])
    ) {
      throw new Error(`unexpected shell region order: ${JSON.stringify(semantics.shell_regions)}`);
    }
    if (semantics.persistent_action_surface) {
      throw new Error("overview retained a persistent action surface");
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

    await selectModulePage(cdp, "Nodes", "Node runtime");
    await waitForExpression(
      cdp,
      `Boolean(document.querySelector(".page-command-primary"))`,
      "Node runtime command surface",
    );

    const commandSemantics = await evaluate(
      cdp,
      `(() => {
        const text = (element) => element?.textContent?.trim() ?? "";
        return {
          heading: text(document.querySelector(".workspace h1")),
          primary: [...document.querySelectorAll(".page-command-primary")].map(text),
          quick: [...document.querySelectorAll(".page-command-icon")].map((button) =>
            button.getAttribute("aria-label"),
          ),
          more: document.querySelectorAll(".page-command-more").length,
          persistent_action_surface: Boolean(
            document.querySelector(
              '[data-ui-region="action-panel"], .actions-panel, .visual-actions',
            ),
          ),
        };
      })()`,
    );
    if (commandSemantics.heading !== "Node runtime") {
      throw new Error(`unexpected runtime heading: ${JSON.stringify(commandSemantics.heading)}`);
    }
    if (commandSemantics.primary.length !== 1 || commandSemantics.more !== 1) {
      throw new Error(`runtime command hierarchy is invalid: ${JSON.stringify(commandSemantics)}`);
    }
    if (commandSemantics.quick.length < 2 || commandSemantics.persistent_action_surface) {
      throw new Error(`runtime quick actions or shell layout is invalid: ${JSON.stringify(commandSemantics)}`);
    }
    const commandCapture = await captureScreenshot(cdp, "node-runtime-commands.png");
    await writeFile(
      join(OUTPUT_ROOT, "node-runtime-commands.json"),
      `${JSON.stringify({ schema_version: 1, surface: "webui", dimensions: commandCapture.dimensions, semantics: commandSemantics }, null, 2)}\n`,
    );

    await clickSelector(cdp, "button.page-command-more");
    await waitForExpression(
      cdp,
      `Boolean(document.querySelector('.page-command-menu[role="menu"]'))`,
      "grouped command menu",
    );
    const menuSemantics = await evaluate(
      cdp,
      `(() => {
        const text = (element) => element?.textContent?.trim() ?? "";
        return {
          groups: [...document.querySelectorAll(".page-command-menu-group h2")].map(text),
          items: [...document.querySelectorAll(".page-command-menu-item strong")].map(text),
          search_placeholder: document.querySelector(
            '.page-command-menu input[type="search"]',
          )?.getAttribute("placeholder"),
          role: document.querySelector(".page-command-menu")?.getAttribute("role"),
        };
      })()`,
    );
    if (
      menuSemantics.role !== "menu" ||
      menuSemantics.items.length === 0 ||
      menuSemantics.groups.length === 0 ||
      !menuSemantics.search_placeholder
    ) {
      throw new Error(`runtime command menu is not grouped and searchable: ${JSON.stringify(menuSemantics)}`);
    }
    const menuCapture = await captureScreenshot(cdp, "node-runtime-menu.png");
    await writeFile(
      join(OUTPUT_ROOT, "node-runtime-menu.json"),
      `${JSON.stringify({ schema_version: 1, surface: "webui", dimensions: menuCapture.dimensions, semantics: menuSemantics }, null, 2)}\n`,
    );

    await clickText(cdp, ".page-command-menu-item strong", "Clean up runtime residue");
    await waitForExpression(
      cdp,
      `Boolean(document.querySelector('.action-workspace[role="dialog"]'))`,
      "action workspace dialog",
    );
    const workspaceSemantics = await evaluate(
      cdp,
      `(() => ({
        role: document.querySelector(".action-workspace")?.getAttribute("role"),
        modal: document.querySelector(".action-workspace")?.getAttribute("aria-modal"),
        title: document.querySelector(".action-workspace h2")?.textContent?.trim() ?? "",
        menu_open: Boolean(document.querySelector(".page-command-menu")),
        persistent_action_surface: Boolean(
          document.querySelector(
            '[data-ui-region="action-panel"], .actions-panel, .visual-actions',
          ),
        ),
      }))()`,
    );
    if (
      workspaceSemantics.role !== "dialog" ||
      workspaceSemantics.modal !== "true" ||
      workspaceSemantics.title !== "Clean up runtime residue" ||
      workspaceSemantics.menu_open ||
      workspaceSemantics.persistent_action_surface
    ) {
      throw new Error(`action workspace is not a focused dialog: ${JSON.stringify(workspaceSemantics)}`);
    }
    const workspaceCapture = await captureScreenshot(cdp, "node-runtime-workspace.png");
    await writeFile(
      join(OUTPUT_ROOT, "node-runtime-workspace.json"),
      `${JSON.stringify({ schema_version: 1, surface: "webui", dimensions: workspaceCapture.dimensions, semantics: workspaceSemantics }, null, 2)}\n`,
    );

    await clickSelector(cdp, ".action-workspace-heading > button[aria-label]");
    await waitForExpression(
      cdp,
      `!document.querySelector(".action-workspace")`,
      "action workspace close",
    );
    await selectModulePage(cdp, "Networks", "Topology");
    await waitForExpression(
      cdp,
      `Boolean(document.querySelector(".topology-page .topology-canvas"))`,
      "Topology canvas",
    );

    const topologyCapture = await captureScreenshot(cdp, "topology.png");
    const topologySemantics = await evaluate(
      cdp,
      `(() => {
        const text = (element) => element?.textContent?.trim() ?? "";
        const geometry = Object.fromEntries(
          [
            ["sidebar_icon_rail", ".primary-nav"],
            ["topbar", ".global-bar"],
            ["page_switcher", ".page-switcher"],
            ["topbar_search", ".global-search-topbar"],
            ["workspace", ".workspace"],
            ["page_heading", ".topology-hero h1"],
            ["topology_hero", ".topology-hero"],
            ["topology_state", ".topology-state-band"],
            ["topology_canvas_stage", ".topology-stage"],
            ["topology_canvas", ".topology-canvas"],
            ["topology_floating_toolbar", ".topology-floating-toolbar"],
            ["topology_canvas_footer", ".topology-canvas-footer"],
            ["topology_detail", ".topology-detail-drawer"],
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
              },
            ];
          }),
        );
        return {
          title: document.title,
          locale: document.documentElement.lang,
          font_family: getComputedStyle(document.body).fontFamily
            .split(",")[0]
            .trim()
            .replace(/^["']|["']$/g, ""),
          viewport: { width: innerWidth, height: innerHeight, devicePixelRatio },
          modules: [...document.querySelectorAll(".module-button")].map(text),
          page_switcher: {
            current: text(document.querySelector(".page-switcher-trigger")),
            options: [...document.querySelectorAll('.page-switcher-menu [role="menuitemradio"]')]
              .map(text)
              .filter(Boolean),
          },
          topbar_search: {
            placeholder:
              document.querySelector(".global-search-topbar input")?.getAttribute("placeholder") ??
              "",
          },
          network_picker: document.querySelector(".network-picker")
            ? text(document.querySelector(".network-picker"))
            : null,
          page_heading: text(document.querySelector(".topology-hero h1")),
          shell_regions: [...document.querySelectorAll("[data-ui-region]")].map(
            (element) => element.getAttribute("data-ui-region"),
          ),
          topology: {
            boundaries: document.querySelectorAll(".topology-network-chips button").length,
            nodes: document.querySelectorAll("g.topology-node").length,
            detail_open: Boolean(document.querySelector(".topology-detail-drawer")),
          },
          geometry,
        };
      })()`,
    );
    if (
      topologySemantics.page_heading !== "Topology" ||
      topologySemantics.network_picker !== null ||
      topologySemantics.topology.detail_open ||
      topologySemantics.topology.boundaries === 0 ||
      topologySemantics.topology.nodes === 0
    ) {
      throw new Error(`invalid topology hierarchy: ${JSON.stringify(topologySemantics)}`);
    }
    await writeFile(
      join(OUTPUT_ROOT, "topology.json"),
      `${JSON.stringify(
        {
          schema_version: 1,
          surface: "webui",
          fixture_revision: fixture.snapshot.revision,
          fixture_locale: fixture.locale,
          chrome: (await cdp.send("Browser.getVersion")).product,
          platform: process.platform,
          node: process.version,
          dimensions: topologyCapture.dimensions,
          sha256: createHash("sha256").update(topologyCapture.image).digest("hex"),
          semantics: topologySemantics,
        },
        null,
        2,
      )}
`,
    );

    const imagePath = overviewCapture.path;
    const metadataPath = join(OUTPUT_ROOT, "overview.json");
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
    process.stdout.write(
      `${imagePath}\n${metadataPath}\n${topologyCapture.path}\n${join(OUTPUT_ROOT, "topology.json")}\n`,
    );
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
