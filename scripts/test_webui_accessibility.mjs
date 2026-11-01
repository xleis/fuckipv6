#!/usr/bin/env node
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { existsSync, mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const sharedCss = readFileSync(path.join(root, "crates/common/assets/ui-components.css"), "utf8");
const sharedJs = readFileSync(path.join(root, "crates/common/assets/ui-components.js"), "utf8");
const nodeHtml = readFileSync(path.join(root, "crates/client/src/webui/node_ui.html"), "utf8");
const nodeJs = readFileSync(path.join(root, "crates/client/src/webui/node_ui.js"), "utf8");

function findChrome() {
  const candidates = [
    process.env.CHROME_BIN,
    "/usr/bin/google-chrome-stable",
    "/usr/bin/google-chrome",
    "/usr/bin/chromium",
    "/usr/bin/chromium-browser",
  ].filter(Boolean);
  const chrome = candidates.find((candidate) => existsSync(candidate));
  if (!chrome) {
    throw new Error("Chrome/Chromium not found; set CHROME_BIN to run the WebUI accessibility test");
  }
  return chrome;
}

class CdpClient {
  constructor(webSocketUrl) {
    this.socket = new WebSocket(webSocketUrl);
    this.nextId = 1;
    this.pending = new Map();
    this.socket.addEventListener("message", (event) => {
      const message = JSON.parse(event.data);
      if (!message.id || !this.pending.has(message.id)) return;
      const pending = this.pending.get(message.id);
      this.pending.delete(message.id);
      if (message.error) pending.reject(new Error(`${pending.method}: ${JSON.stringify(message.error)}`));
      else pending.resolve(message.result);
    });
  }

  async open() {
    await new Promise((resolve, reject) => {
      this.socket.addEventListener("open", resolve, { once: true });
      this.socket.addEventListener("error", reject, { once: true });
    });
  }

  send(method, params = {}, sessionId) {
    return new Promise((resolve, reject) => {
      const id = this.nextId++;
      this.pending.set(id, { resolve, reject, method });
      this.socket.send(JSON.stringify({ id, method, params, ...(sessionId ? { sessionId } : {}) }));
    });
  }

  close() {
    this.socket.close();
  }
}

async function waitForDevToolsUrl(chrome, timeoutMs = 15_000) {
  let stderr = "";
  chrome.stderr.setEncoding("utf8");
  chrome.stderr.on("data", (chunk) => { stderr += chunk; });
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    const match = stderr.match(/DevTools listening on (ws:\/\/[^\s]+)/);
    if (match) return match[1];
    if (chrome.exitCode !== null) throw new Error(`Chrome exited before DevTools was ready: ${stderr}`);
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
  throw new Error(`Timed out waiting for Chrome DevTools: ${stderr}`);
}

const fixture = `<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <title>WebUI accessibility fixture</title>
  <style>
    :root {
      --accent: #38bdf8;
      --accent-secondary: #7dd3fc;
      --background-deep: #071018;
      --border: #1d4256;
      --border-soft: #173243;
      --border-strong: #2a5a72;
      --danger: #fb7185;
      --danger-soft: #3b1721;
      --danger-text: #ffd7df;
      --font-mono: monospace;
      --font-size-sm: 14px;
      --font-size-xs: 12px;
      --font-weight-medium: 500;
      --font-weight-semibold: 600;
      --motion-duration-base: 120ms;
      --motion-ease-standard: ease;
      --panel-solid: #0d1b26;
      --radius-md: 10px;
      --radius-sm: 8px;
      --radius-xs: 6px;
      --shadow-pop: 0 18px 50px #000a;
      --space-1: 4px;
      --space-2: 8px;
      --space-3: 12px;
      --space-4: 16px;
      --space-8: 32px;
      --surface: #0d1b26;
      --surface-hover: #142b3a;
      --text: #d8edf6;
      --text-secondary: #b5d5e2;
      --text-muted: #7fa3b3;
      --control-sm-h: 32px;
      --control-md-h: 38px;
    }
    body { margin: 20px; background: var(--background-deep); color: var(--text); font: 14px sans-serif; }
    .ui-dialog-backdrop { position: fixed; inset: 0; z-index: 20; display: grid; place-items: center; background: #000b; }
    .ui-dialog-backdrop[hidden] { display: none; }
    .ui-dialog { width: min(420px, calc(100vw - 40px)); padding: 20px; background: var(--panel-solid); border: 1px solid var(--border); }
    .ui-local-tabs { display: flex; gap: 8px; margin-top: 24px; }
  </style>
  <style>${sharedCss}</style>
</head>
<body>
  <button id="background-control" type="button">Background control</button>
  <button id="open-dialog" type="button">Open dialog</button>
  <div id="test-dialog" class="ui-dialog-backdrop" data-ui-dialog hidden>
    <section class="ui-dialog" role="dialog" aria-modal="true" aria-labelledby="test-dialog-title">
      <h2 id="test-dialog-title">Accessible dialog</h2>
      <button id="dialog-first" type="button">First action</button>
      <input id="dialog-middle" aria-label="Middle field">
      <button id="dialog-last" type="button">Last action</button>
    </section>
  </div>

  <button id="menu-trigger" type="button" aria-label="More actions for peer-one">…</button>
  <div id="test-menu" class="ui-context-menu" role="menu" aria-label="Row actions" hidden>
    <button class="ui-menu-item" type="button" role="menuitem">Open peers</button>
    <button class="ui-menu-item" type="button" role="menuitem">Show details</button>
    <button class="ui-menu-item" type="button" role="menuitem">Copy address</button>
  </div>

  <section class="ui-app-shell">
    <div class="ui-local-tabs" data-ui-local-tabs aria-label="Page regions">
      <button class="ui-local-tab" type="button" data-ui-local-tab="data" aria-selected="true">Data</button>
      <button class="ui-local-tab" type="button" data-ui-local-tab="actions" aria-selected="false">Actions</button>
    </div>
    <section data-ui-page-panel="data" role="tabpanel">Data panel</section>
    <section data-ui-page-panel="actions" role="tabpanel" hidden>Actions panel</section>
  </section>
  <div id="capability-host"></div>
  <script>${sharedJs}</script>
  <script>
    const ui = window.FuckIPv6UI;
    ui.init(document);
    const dialog = document.getElementById("test-dialog");
    document.getElementById("open-dialog").addEventListener("click", () => {
      ui.openDialog(dialog, { initialFocus: "#dialog-first" });
    });
    const menu = document.getElementById("test-menu");
    ui.bindContextMenu(document.getElementById("menu-trigger"), menu);
    const ring = ui.capabilityRing(["relay"], {
      labels: { coordinator: "Coordinator", relay: "Relay", exit: "Exit", file: "File" },
      stateLabels: { active: "Enabled", inactive: "Disabled" },
    });
    document.getElementById("capability-host").append(ring);
  </script>
</body>
</html>`;

async function main() {
  const userDataDir = mkdtempSync(path.join(tmpdir(), "fuckipv6-a11y-"));
  const chrome = spawn(findChrome(), [
    "--headless=new",
    "--no-sandbox",
    "--disable-gpu",
    "--disable-background-networking",
    "--disable-component-update",
    "--disable-default-apps",
    "--disable-extensions",
    "--disable-sync",
    "--no-first-run",
    "--lang=en-US",
    "--remote-debugging-port=0",
    `--user-data-dir=${userDataDir}`,
    "about:blank",
  ], { stdio: ["ignore", "ignore", "pipe"] });

  let cdp;
  try {
    const browserWebSocketUrl = await waitForDevToolsUrl(chrome);
    cdp = new CdpClient(browserWebSocketUrl);
    await cdp.open();

    const { targetId } = await cdp.send("Target.createTarget", { url: "about:blank" });
    const { sessionId } = await cdp.send("Target.attachToTarget", { targetId, flatten: true });
    const send = (method, params = {}) => cdp.send(method, params, sessionId);

    await send("Page.enable");
    await send("Runtime.enable");
    await send("Page.bringToFront");
    const { frameTree } = await send("Page.getFrameTree");
    await send("Page.setDocumentContent", { frameId: frameTree.frame.id, html: fixture });

    async function evaluate(expression) {
      const result = await send("Runtime.evaluate", {
        expression,
        awaitPromise: true,
        returnByValue: true,
        userGesture: true,
      });
      if (result.exceptionDetails) {
        throw new Error(result.exceptionDetails.exception?.description || result.exceptionDetails.text);
      }
      return result.result.value;
    }

    async function waitFor(expression, description, timeoutMs = 10_000) {
      const deadline = Date.now() + timeoutMs;
      let lastError;
      while (Date.now() < deadline) {
        try {
          if (await evaluate(expression)) return;
        } catch (error) {
          lastError = error;
        }
        await new Promise((resolve) => setTimeout(resolve, 50));
      }
      throw new Error(`Timed out waiting for ${description}${lastError ? `: ${lastError.message}` : ""}`);
    }

    async function key(key, code, keyCode, modifiers = 0) {
      await send("Input.dispatchKeyEvent", {
        type: "keyDown",
        key,
        code,
        windowsVirtualKeyCode: keyCode,
        nativeVirtualKeyCode: keyCode,
        modifiers,
      });
      await send("Input.dispatchKeyEvent", {
        type: "keyUp",
        key,
        code,
        windowsVirtualKeyCode: keyCode,
        nativeVirtualKeyCode: keyCode,
        modifiers,
      });
    }

    await waitFor("window.FuckIPv6UI && document.readyState === 'complete'", "shared UI runtime");

    const initialTabs = await evaluate(`(() => {
      const tabs = [...document.querySelectorAll('[role="tab"]')];
      return {
        count: tabs.length,
        controls: tabs.map((tab) => tab.getAttribute('aria-controls')),
        selected: tabs.map((tab) => tab.getAttribute('aria-selected')),
        panelLabels: [...document.querySelectorAll('[role="tabpanel"]')].map((panel) => panel.getAttribute('aria-labelledby')),
      };
    })()`);
    assert.equal(initialTabs.count, 2);
    assert.ok(initialTabs.controls.every(Boolean));
    assert.deepEqual(initialTabs.selected, ["true", "false"]);
    assert.ok(initialTabs.panelLabels.every(Boolean));
    await evaluate("document.querySelector('[role=tab]').focus()");
    await key("ArrowRight", "ArrowRight", 39);
    await waitFor(
      "document.activeElement?.textContent === 'Actions' && document.activeElement?.getAttribute('aria-selected') === 'true'",
      "ArrowRight tab selection",
    );
    assert.equal(await evaluate("document.querySelector('[data-ui-page-panel=actions]').hidden"), false);
    await key("Home", "Home", 36);
    await waitFor("document.activeElement?.textContent === 'Data'", "Home tab selection");

    await evaluate(`(() => {
      const trigger = document.getElementById('open-dialog');
      trigger.focus();
      trigger.click();
    })()`);
    await waitFor(
      "!document.getElementById('test-dialog').hidden && document.activeElement.id === 'dialog-first'",
      "dialog open and initial focus",
    );
    assert.equal(await evaluate("document.getElementById('test-dialog').querySelector('[role=dialog]').getAttribute('aria-modal')"), "true");
    assert.equal(await evaluate("document.getElementById('background-control').inert"), true);
    assert.equal(await evaluate("document.getElementById('background-control').getAttribute('aria-hidden')"), "true");

    await evaluate("document.getElementById('dialog-last').focus()");
    await key("Tab", "Tab", 9);
    await waitFor("document.activeElement.id === 'dialog-first'", "Tab focus trap");
    await key("Tab", "Tab", 9, 8);
    await waitFor("document.activeElement.id === 'dialog-last'", "Shift+Tab focus trap");
    assert.notEqual(await evaluate("getComputedStyle(document.activeElement).outlineStyle"), "none");
    await key("Escape", "Escape", 27);
    await waitFor("document.getElementById('test-dialog').hidden", "Escape dialog close");
    assert.equal(await evaluate("document.activeElement.id"), "open-dialog");
    assert.equal(await evaluate("document.getElementById('background-control').inert"), false);
    assert.equal(await evaluate("document.getElementById('background-control').getAttribute('aria-hidden')"), null);

    await evaluate("document.getElementById('menu-trigger').focus()");
    await key("Enter", "Enter", 13);
    await waitFor(
      "!document.getElementById('test-menu').hidden && document.activeElement.textContent === 'Open peers'",
      "Enter menu open and initial focus",
    );
    assert.equal(await evaluate("document.getElementById('menu-trigger').getAttribute('aria-expanded')"), "true");
    await key("ArrowDown", "ArrowDown", 40);
    await waitFor("document.activeElement.textContent === 'Show details'", "ArrowDown menu navigation");
    await key("End", "End", 35);
    await waitFor("document.activeElement.textContent === 'Copy address'", "End menu navigation");
    await key("Home", "Home", 36);
    await waitFor("document.activeElement.textContent === 'Open peers'", "Home menu navigation");
    await key("Escape", "Escape", 27);
    await waitFor("document.getElementById('test-menu').hidden", "Escape menu close");
    assert.equal(await evaluate("document.activeElement.id"), "menu-trigger");

    await key("F10", "F10", 121, 8);
    await waitFor(
      "!document.getElementById('test-menu').hidden && document.activeElement.textContent === 'Open peers'",
      "Shift+F10 menu open and initial focus",
    );
    await key("Escape", "Escape", 27);

    await key("ContextMenu", "ContextMenu", 93);
    await waitFor(
      "!document.getElementById('test-menu').hidden && document.activeElement.textContent === 'Open peers'",
      "ContextMenu key menu open and initial focus",
    );
    await key("Escape", "Escape", 27);

    const point = await evaluate(`(() => {
      const rect = document.getElementById('menu-trigger').getBoundingClientRect();
      return { x: rect.left + rect.width / 2, y: rect.top + rect.height / 2 };
    })()`);
    await send("Input.dispatchMouseEvent", {
      type: "mousePressed", x: point.x, y: point.y, button: "right", buttons: 2, clickCount: 1,
    });
    await send("Input.dispatchMouseEvent", {
      type: "mouseReleased", x: point.x, y: point.y, button: "right", buttons: 0, clickCount: 1,
    });
    await waitFor(
      "!document.getElementById('test-menu').hidden && document.activeElement.textContent === 'Open peers'",
      "right-click menu open and initial focus",
    );
    assert.equal(await evaluate("document.getElementById('menu-trigger').getAttribute('aria-expanded')"), "true");
    await key("Escape", "Escape", 27);
    assert.equal(await evaluate("document.activeElement.id"), "menu-trigger");

    const ring = await evaluate(`(() => {
      const element = document.querySelector('[data-component="capability-ring"]');
      return {
        text: element.textContent,
        svgHidden: element.querySelector('svg').getAttribute('aria-hidden'),
        states: [...element.querySelectorAll('.ui-capability-ring-legend [data-capability]')].map((item) => ({
          name: item.dataset.capability,
          active: item.dataset.active,
          text: item.textContent,
        })),
      };
    })()`);
    assert.match(ring.text, /Relay: Enabled/);
    assert.match(ring.text, /Coordinator: Disabled/);
    assert.match(ring.text, /Exit: Disabled/);
    assert.match(ring.text, /File: Disabled/);
    assert.equal(ring.svgHidden, "true");
    assert.ok(ring.states.every((state) => state.text.includes(state.active === "true" ? "Enabled" : "Disabled")));

    assert.ok((nodeHtml.match(/data-ui-dialog/g) || []).length >= 3, "node surfaces must expose accessible dialog roots");
    assert.match(nodeHtml, /button:focus-visible,input:focus-visible,select:focus-visible/);
    assert.match(nodeHtml, /bindTopologyMenuRows/);
    assert.match(nodeHtml, /ui\.bindContextMenu\(button, menu\)/);
    assert.match(nodeHtml, /ui\.bindContextMenu\(rowElement, menu,/);
    assert.match(nodeHtml, /topology\.more_actions/);
    assert.match(nodeJs, /function topologyRows/);
    assert.match(nodeJs, /topology\.peer_row/);
    assert.match(nodeJs, /topology\.peer_capabilities_unavailable/);

    console.log("test_webui_accessibility: passed");
  } finally {
    cdp?.close();
    chrome.kill("SIGTERM");
    await new Promise((resolve) => setTimeout(resolve, 100));
    rmSync(userDataDir, { recursive: true, force: true });
  }
}

main().catch((error) => {
  console.error(error.stack || error);
  process.exitCode = 1;
});
