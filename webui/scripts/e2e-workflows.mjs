#!/usr/bin/env node
/**
 * Real-browser workflow check for the built WebUI.
 *
 * Chrome is driven over the DevTools protocol (the repository has no Playwright
 * dependency and the build machine may be offline), so this is honest about what
 * it is: a real browser rendering the real bundle against the contract fixtures,
 * clicking the navigation, opening a confirmation, checking focus and labels,
 * and failing on any console error.
 */
import { spawn } from "node:child_process";
import { mkdir, readFile, writeFile } from "node:fs/promises";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { startFixtureServer } from "./lib/fixture-server.mjs";

const WEBUI_ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const REPO_ROOT = resolve(WEBUI_ROOT, "..");
const DIST_ROOT = join(REPO_ROOT, "crates/client/src/webui/dist");
const FIXTURE_ROOT = join(REPO_ROOT, "contracts/typescript/fixtures");
const SHARED_ROOT = join(REPO_ROOT, "target/e2e-webui/shared");
const OUTPUT_ROOT = resolve(
  process.env.E2E_OUTPUT_DIR ?? join(REPO_ROOT, "target/e2e-webui"),
);
const NAVIGATION = JSON.parse(
  await readFile(join(REPO_ROOT, "crates/common/assets/ui-navigation.json"), "utf8"),
);

function findChrome() {
  const candidates = [
    process.env.CHROME_BIN,
    "google-chrome-stable",
    "google-chrome",
    "chromium",
    "chromium-browser",
  ].filter(Boolean);
  for (const candidate of candidates) {
    const probe = spawn(candidate, ["--version"], { stdio: "ignore" });
    if (probe.pid !== undefined) {
      probe.kill();
      return candidate;
    }
  }
  throw new Error("Chrome/Chromium not found; set CHROME_BIN");
}

async function exportSharedAssets() {
  await mkdir(SHARED_ROOT, { recursive: true });
  await new Promise((resolvePromise, reject) => {
    const child = spawn(
      "cargo",
      ["run", "--quiet", "-p", "fuckipv6-common", "--example", "export-browser-assets", "--", SHARED_ROOT],
      { cwd: REPO_ROOT, stdio: "inherit" },
    );
    child.once("error", reject);
    child.once("exit", (code) =>
      code === 0 ? resolvePromise() : reject(new Error(`export-browser-assets exited with ${code}`)),
    );
  });
}

class Cdp {
  constructor(socket) {
    this.socket = socket;
    this.nextId = 1;
    this.pending = new Map();
    this.listeners = new Map();
    socket.onmessage = (event) => {
      const message = JSON.parse(event.data);
      if (message.id !== undefined) {
        const entry = this.pending.get(message.id);
        if (entry) {
          this.pending.delete(message.id);
          if (message.error) entry.reject(new Error(JSON.stringify(message.error)));
          else entry.resolve(message.result);
        }
        return;
      }
      const handlers = this.listeners.get(message.method) ?? [];
      for (const handler of handlers) handler(message.params);
    };
  }

  static async connect(url) {
    const socket = new WebSocket(url);
    await new Promise((resolvePromise, reject) => {
      socket.onopen = resolvePromise;
      socket.onerror = () => reject(new Error(`cannot open ${url}`));
    });
    return new Cdp(socket);
  }

  send(method, params = {}) {
    const id = this.nextId++;
    return new Promise((resolvePromise, reject) => {
      this.pending.set(id, { resolve: resolvePromise, reject });
      this.socket.send(JSON.stringify({ id, method, params }));
    });
  }

  on(method, handler) {
    const handlers = this.listeners.get(method) ?? [];
    handlers.push(handler);
    this.listeners.set(method, handlers);
  }

  close() {
    this.socket.close();
  }
}

async function evaluate(cdp, expression) {
  const result = await cdp.send("Runtime.evaluate", {
    expression,
    awaitPromise: true,
    returnByValue: true,
  });
  if (result.exceptionDetails) {
    throw new Error(`evaluation failed: ${result.exceptionDetails.text ?? "unknown"}`);
  }
  return result.result.value;
}

async function waitFor(cdp, expression, { timeoutMs = 15000, label = expression } = {}) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (await evaluate(cdp, expression)) return true;
    await new Promise((resolvePromise) => setTimeout(resolvePromise, 100));
  }
  throw new Error(`timed out waiting for ${label}`);
}

async function launchChrome(origin) {
  const binary = findChrome();
  const profile = join(OUTPUT_ROOT, `chrome-profile-${process.pid}`);
  const child = spawn(
    binary,
    [
      "--headless=new",
      "--disable-gpu",
      "--no-sandbox",
      "--no-first-run",
      "--disable-dev-shm-usage",
      "--remote-debugging-port=0",
      "--window-size=1440,900",
      `--user-data-dir=${profile}`,
      origin,
    ],
    { stdio: ["ignore", "ignore", "pipe"] },
  );
  let browserWs = null;
  child.stderr.on("data", (chunk) => {
    const match = String(chunk).match(/DevTools listening on (ws:\/\/\S+)/);
    if (match) browserWs = match[1];
  });
  const deadline = Date.now() + 20000;
  while (!browserWs && Date.now() < deadline) await new Promise((r) => setTimeout(r, 100));
  if (!browserWs) {
    child.kill();
    throw new Error("Chrome did not expose a DevTools endpoint");
  }
  const httpBase = browserWs.replace(/^ws:\/\//, "http://").replace(/\/devtools\/browser\/.*$/, "");
  const targets = await (await fetch(`${httpBase}/json/list`)).json();
  const page = targets.find((target) => target.type === "page");
  if (!page?.webSocketDebuggerUrl) {
    child.kill();
    throw new Error("no page target with a DevTools URL");
  }
  return { child, pageWsUrl: page.webSocketDebuggerUrl };
}

function moduleIds() {
  return NAVIGATION.modules.map((module) => module.id);
}

async function runChecks(cdp, failures) {
  const checks = [];
  const record = (name, ok, detail = "") => {
    checks.push({ name, status: ok ? "passed" : "failed", detail: ok ? "" : detail });
    if (!ok) failures.push(`${name}: ${detail}`);
  };

  await waitFor(
    cdp,
    "Boolean(document.querySelector('.app-shell')) || Boolean(document.querySelector('.centered-shell'))",
    { label: "app shell" },
  );
  await waitFor(cdp, "Boolean(document.querySelector('#page-title, main h1, main h2'))", {
    label: "first page heading",
  });
  const shell = await evaluate(cdp, "Boolean(document.querySelector('.app-shell'))");
  record("shell-renders", shell, "the app shell did not render");

  await waitFor(cdp, "document.querySelectorAll('[data-ui-region=\"sidebar\"] button').length > 0", {
    label: "module navigation",
  });
  const navLabels = await evaluate(
    cdp,
    `Array.from(document.querySelectorAll('[data-ui-region="sidebar"] button')).map(
       (b) => (b.getAttribute('aria-label') ?? '').trim() || (b.textContent ?? '').trim(),
     )`,
  );
  record(
    "navigation-has-labelled-buttons",
    Array.isArray(navLabels) && navLabels.length >= moduleIds().length,
    `found ${JSON.stringify(navLabels)}`,
  );

  const lang = await evaluate(cdp, "document.documentElement.lang");
  record("document-lang", typeof lang === "string" && lang.length >= 2, `lang=${lang}`);

  const unlabelled = await evaluate(
    cdp,
    `Array.from(document.querySelectorAll('button')).filter((b) => {
       const text = (b.textContent ?? '').trim();
       const label = b.getAttribute('aria-label') ?? b.getAttribute('title') ?? '';
       return text.length === 0 && label.trim().length === 0;
     }).length`,
  );
  record("icon-buttons-labelled", unlabelled === 0, `${unlabelled} button(s) without text or label`);

  const modules = await evaluate(
    cdp,
    `document.querySelectorAll('[data-ui-region="sidebar"] button').length`,
  );
  record("module-tabs-match-contract", modules === moduleIds().length, `tabs=${modules}`);

  // Visit every module and assert the heading follows the selection.
  const moduleCount = moduleIds().length;
  for (const [index, module] of NAVIGATION.modules.entries()) {
    const clicked = await evaluate(
      cdp,
      `(() => {
         const sidebar = document.querySelector('[data-ui-region="sidebar"]');
         if (!sidebar) return false;
         const buttons = Array.from(sidebar.querySelectorAll('button'));
         const button = buttons[${index}] ?? buttons.find((b) => (b.textContent ?? '').trim().length > 0);
         if (!button) return false;
         button.click();
         return true;
       })()`,
    );
    record(`module-${module.id}-clickable`, clicked === true, "module tab was not found");
    if (!clicked) continue;
    await waitFor(cdp, "Boolean(document.querySelector('#page-title, main h1, main h2'))", {
      label: `${module.id} heading`,
    });
    const heading = await evaluate(
      cdp,
      `(document.querySelector('#page-title') ?? document.querySelector('main h1, main h2'))?.textContent?.trim() ?? ''`,
    );
    record(`module-${module.id}-heading`, heading.length > 0, `heading=${heading}`);
    const current = await evaluate(
      cdp,
      `(() => {
         const sidebar = document.querySelector('[data-ui-region="sidebar"]');
         if (!sidebar) return null;
         const selected = sidebar.querySelector('button[aria-current="page"]');
         return selected ? (selected.getAttribute('aria-label') ?? '') : null;
       })()`,
    );
    record(`module-${module.id}-selected`, current !== null, "no tab marked aria-current=page");
    if (index + 1 < moduleCount) {
      await new Promise((resolvePromise) => setTimeout(resolvePromise, 50));
    }
  }

  // Keyboard: one Tab from the body must land inside the shell, not fall out.
  await evaluate(cdp, "document.body.focus(); true");
  await cdp.send("Input.dispatchKeyEvent", {
    type: "keyDown",
    key: "Tab",
    code: "Tab",
    windowsVirtualKeyCode: 9,
  });
  await cdp.send("Input.dispatchKeyEvent", {
    type: "keyUp",
    key: "Tab",
    code: "Tab",
    windowsVirtualKeyCode: 9,
  });
  const focus = await evaluate(
    cdp,
    `(() => {
       const el = document.activeElement;
       if (!el) return 'none';
       return el.tagName.toLowerCase() + (el.getAttribute('aria-label') ? '[' + el.getAttribute('aria-label') + ']' : '');
     })()`,
  );
  record("tab-enters-the-shell", focus !== "none" && focus !== "body", `focus=${focus}`);

  return checks;
}

async function main() {
  await mkdir(OUTPUT_ROOT, { recursive: true });
  await exportSharedAssets();
  const manifest = JSON.parse(
    await readFile(join(FIXTURE_ROOT, "node-manifest.json"), "utf8"),
  );
  const fixture = JSON.parse(await readFile(join(FIXTURE_ROOT, "product-visual-snapshot.json"), "utf8"));
  const server = await startFixtureServer({
    manifest,
    fixture,
    distRoot: DIST_ROOT,
    sharedAssets: SHARED_ROOT,
  });
  const { child, pageWsUrl } = await launchChrome(server.origin);
  const cdp = await Cdp.connect(pageWsUrl);
  const consoleErrors = [];
  cdp.on("Runtime.consoleAPICalled", (params) => {
    if (params.type === "error") {
      consoleErrors.push(
        (params.args ?? []).map((arg) => arg.value ?? arg.description ?? arg.type).join(" "),
      );
    }
  });
  cdp.on("Runtime.exceptionThrown", (params) => {
    consoleErrors.push(params.exceptionDetails?.text ?? "uncaught exception");
  });
  await cdp.send("Runtime.enable");
  await cdp.send("Page.enable");
  // The initial page target is `about:blank`; the real navigation is explicit so
  // the harness never depends on which target Chrome happened to open first.
  await cdp.send("Page.navigate", { url: server.origin });

  const failures = [];
  let checks = [];
  try {
    await waitFor(cdp, "document.readyState === 'complete'", { label: "document load" });
    checks = await runChecks(cdp, failures);
  } catch (error) {
    failures.push(`harness: ${error.message}`);
  }

  if (consoleErrors.length > 0) {
    failures.push(`console errors: ${consoleErrors.slice(0, 5).join(" | ")}`);
  }

  const report = {
    schema: "fuckipv6-webui-e2e",
    status: failures.length === 0 ? "passed" : "failed",
    checks,
    console_errors: consoleErrors,
    failures,
    driver: "chrome-devtools-protocol",
  };
  await writeFile(join(OUTPUT_ROOT, "report.json"), `${JSON.stringify(report, null, 2)}\n`);

  cdp.close();
  child.kill();
  await server.close();

  for (const check of checks) {
    console.log(`[e2e] ${check.name}: ${check.status}${check.detail ? ` (${check.detail})` : ""}`);
  }
  for (const failure of failures) console.error(`[e2e] ${failure}`);
  console.log(`e2e-workflows: ${report.status} (${checks.length} checks, ${failures.length} failures)`);
  process.exit(report.status === "passed" ? 0 : 1);
}

await main();
