#!/usr/bin/env node
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { existsSync, mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";

const baseUrl = process.env.FUCKIPV6_E2E_URL;
const bootstrapCode = process.env.FUCKIPV6_E2E_BOOTSTRAP_CODE;
if (!baseUrl || !bootstrapCode) {
  throw new Error("FUCKIPV6_E2E_URL and FUCKIPV6_E2E_BOOTSTRAP_CODE are required");
}

function findChrome() {
  const candidates = [
    process.env.CHROME_BIN,
    "/usr/bin/google-chrome-stable",
    "/usr/bin/google-chrome",
    "/usr/bin/chromium",
    "/usr/bin/chromium-browser",
    "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
    "C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe",
    "C:\\Program Files (x86)\\Google\\Chrome\\Application\\chrome.exe",
  ].filter(Boolean);
  const chrome = candidates.find((candidate) => existsSync(candidate));
  if (!chrome) {
    throw new Error("Chrome/Chromium not found; set CHROME_BIN to run the WebAuthn browser test");
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
      if (message.error) {
        pending.reject(new Error(`${pending.method}: ${JSON.stringify(message.error)}`));
      } else {
        pending.resolve(message.result);
      }
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
  chrome.stderr.on("data", (chunk) => {
    stderr += chunk;
  });
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    const match = stderr.match(/DevTools listening on (ws:\/\/[^\s]+)/);
    if (match) return match[1];
    if (chrome.exitCode !== null) {
      throw new Error(`Chrome exited before DevTools was ready: ${stderr}`);
    }
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
  throw new Error(`Timed out waiting for Chrome DevTools: ${stderr}`);
}

async function main() {
  const userDataDir = mkdtempSync(path.join(tmpdir(), "fuckipv6-webauthn-"));
  const chrome = spawn(findChrome(), [
    "--headless=new",
    "--no-sandbox",
    "--disable-gpu",
    "--disable-background-networking",
    "--disable-component-update",
    "--disable-default-apps",
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
    await send("Network.enable");
    await send("WebAuthn.enable");

    async function addAuthenticator(enabled, transport) {
      const { authenticatorId } = await send("WebAuthn.addVirtualAuthenticator", {
        options: {
          protocol: "ctap2",
          transport,
          hasResidentKey: true,
          hasUserVerification: true,
          isUserVerified: true,
          automaticPresenceSimulation: enabled,
        },
      });
      return authenticatorId;
    }

    async function setPresence(authenticatorId, enabled) {
      await send("WebAuthn.setAutomaticPresenceSimulation", {
        authenticatorId,
        enabled,
      });
    }

    async function evaluate(expression) {
      const result = await send("Runtime.evaluate", {
        expression,
        awaitPromise: true,
        returnByValue: true,
        userGesture: true,
      });
      if (result.exceptionDetails) {
        const detail = result.exceptionDetails.exception?.description ||
          result.exceptionDetails.text ||
          "browser evaluation failed";
        throw new Error(detail);
      }
      return result.result.value;
    }

    async function waitFor(expression, description, timeoutMs = 20_000) {
      const deadline = Date.now() + timeoutMs;
      let lastError;
      while (Date.now() < deadline) {
        try {
          if (await evaluate(expression)) return;
        } catch (error) {
          lastError = error;
        }
        await new Promise((resolve) => setTimeout(resolve, 100));
      }
      throw new Error(`Timed out waiting for ${description}${lastError ? `: ${lastError.message}` : ""}`);
    }

    async function ownerCookie() {
      const { cookies } = await send("Network.getCookies", { urls: [baseUrl] });
      return cookies.find((cookie) => cookie.name === "fuckipv6_owner_session")?.value || null;
    }

    const authenticatorA = await addAuthenticator(true, "internal");
    await send("Page.navigate", { url: `${baseUrl}/` });
    await waitFor(
      "document.readyState === 'complete' && !!document.querySelector('#bootstrap-code')",
      "owner bootstrap page",
    );

    await evaluate(`(() => {
      const input = document.querySelector('#bootstrap-code');
      input.value = ${JSON.stringify(bootstrapCode)};
      document.querySelector('#start-passkey').click();
      return true;
    })()`);
    await waitFor(
      "!!document.querySelector('#step-recovery.active') || document.querySelector('#status.error')?.textContent.length > 0",
      "initial Passkey registration",
    );
    const languageSwitch = await evaluate(`(() => {
      const select = document.querySelector('#language-preference');
      const snapshot = () => ({
        locale: document.documentElement.lang,
        title: document.querySelector('#page-title').textContent,
        status: document.querySelector('#status').textContent,
      });
      const before = snapshot();
      select.value = 'zh-CN';
      select.dispatchEvent(new Event('change', { bubbles: true }));
      return { before, after: snapshot(), preference: localStorage.getItem('fuckipv6.language-preference') };
    })()`);
    assert.equal(languageSwitch.before.locale, "en");
    assert.match(languageSwitch.before.status, /Passkey registered/);
    assert.equal(languageSwitch.after.locale, "zh-CN");
    assert.match(languageSwitch.after.title, /本机所有者初始化/);
    assert.match(languageSwitch.after.status, /Passkey 已注册/);
    assert.equal(languageSwitch.preference, "zh-CN");
    const recoveryCode = await evaluate("document.querySelector('#recovery-code').textContent.trim()");
    assert.ok(recoveryCode.length >= 20, "recovery code must be displayed once");

    await evaluate(`(() => {
      document.querySelector('#recovery-saved').click();
      document.querySelector('#confirm-recovery').click();
      return true;
    })()`);
    await waitFor(
      "document.readyState === 'complete' && !!globalThis.FuckIPV6NodeUi && !document.querySelector('#bootstrap-code')",
      "authenticated product page shell",
    );
    assert.equal(await evaluate("document.documentElement.lang"), "zh-CN");

    async function loginWithCurrentAuthenticator() {
      return evaluate(`(async () => {
        const api = async (path, options = {}) => {
          const response = await fetch(path, {
            ...options,
            cache: 'no-store',
            credentials: 'same-origin',
            headers: { 'content-type': 'application/json', ...(options.headers || {}) },
          });
          const text = await response.text();
          let payload = null;
          try { payload = text ? JSON.parse(text) : null; } catch (_) { payload = text; }
          return { status: response.status, payload };
        };
        const start = await api('/api/v1/auth/passkey/authentication/start', { method: 'POST' });
        if (start.status !== 200) throw new Error('authentication start failed: ' + JSON.stringify(start));
        const credential = await navigator.credentials.get(
          globalThis.FuckIPV6NodeUi.decodeRequestOptions(start.payload),
        );
        if (!credential) throw new Error('authentication credential missing');
        const body = JSON.stringify({
          credential: globalThis.FuckIPV6NodeUi.encodeAuthenticationCredential(credential),
        });
        const finish = await api('/api/v1/auth/passkey/authentication/finish', {
          method: 'POST',
          body,
        });
        if (finish.status !== 200) throw new Error('authentication finish failed: ' + JSON.stringify(finish));
        return finish.payload;
      })()`);
    }

    const initialSession = await loginWithCurrentAuthenticator();
    assert.equal(initialSession.authenticated, true);
    const firstPasskeyId = initialSession.credential_id;
    const firstSessionCookie = await ownerCookie();
    assert.ok(firstPasskeyId);
    assert.ok(firstSessionCookie);

    async function registerPasskey(label) {
      const registered = await evaluate(`(async () => {
        const api = async (path, options = {}) => {
          const response = await fetch(path, {
            ...options,
            cache: 'no-store',
            credentials: 'same-origin',
            headers: { 'content-type': 'application/json', ...(options.headers || {}) },
          });
          const text = await response.text();
          let payload = null;
          try { payload = text ? JSON.parse(text) : null; } catch (_) { payload = text; }
          return { status: response.status, payload };
        };
        const start = await api('/api/v1/auth/passkeys/registration/start', {
          method: 'POST',
          body: JSON.stringify({ label: ${JSON.stringify(label)} }),
        });
        if (start.status !== 200) throw new Error('registration start failed: ' + JSON.stringify(start));
        const credential = await navigator.credentials.create(
          globalThis.FuckIPV6NodeUi.decodeCreationOptions(start.payload),
        );
        if (!credential) throw new Error('registration credential missing');
        const finish = await api('/api/v1/auth/passkeys/registration/finish', {
          method: 'POST',
          body: JSON.stringify({
            credential: globalThis.FuckIPV6NodeUi.encodeRegistrationCredential(credential),
          }),
        });
        if (finish.status !== 201) throw new Error('registration finish failed: ' + JSON.stringify(finish));
        return finish.payload.passkey;
      })()`);
      assert.equal(registered.label, label);
      assert.ok(registered.credential_id);
      return registered.credential_id;
    }

    await setPresence(authenticatorA, false);
    const authenticatorB = await addAuthenticator(true, "usb");
    const secondPasskeyId = await registerPasskey("浏览器测试 B");
    await setPresence(authenticatorB, false);
    const authenticatorC = await addAuthenticator(true, "ble");
    const thirdPasskeyId = await registerPasskey("浏览器测试 C");

    const listAfterRegistration = await evaluate(`(async () => {
      const response = await fetch('/api/v1/auth/passkeys', { cache: 'no-store', credentials: 'same-origin' });
      return { status: response.status, payload: await response.json() };
    })()`);
    assert.equal(listAfterRegistration.status, 200);
    assert.equal(listAfterRegistration.payload.passkeys.length, 3);

    const duplicate = await evaluate(`(async () => {
      const response = await fetch('/api/v1/auth/passkeys/registration/start', {
        method: 'POST',
        cache: 'no-store',
        credentials: 'same-origin',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify({ label: '重复凭据' }),
      });
      if (response.status !== 200) return { rejected: true, status: response.status };
      const options = globalThis.FuckIPV6NodeUi.decodeCreationOptions(await response.json());
      try {
        await navigator.credentials.create(options);
        return { rejected: false };
      } catch (error) {
        return { rejected: true, name: error.name };
      }
    })()`);
    assert.equal(duplicate.rejected, true, "excluded resident credential must not be replaced");

    const logout = await evaluate(`fetch('/api/v1/auth/passkey/logout', {
      method: 'POST', cache: 'no-store', credentials: 'same-origin',
    }).then((response) => response.status)`);
    assert.equal(logout, 204);
    assert.equal(await ownerCookie(), null);

    // Leave only the second Passkey's authenticator present so the next
    // assertion proves login can select a newly registered Passkey.
    await send("WebAuthn.removeVirtualAuthenticator", { authenticatorId: authenticatorA });
    await send("WebAuthn.removeVirtualAuthenticator", { authenticatorId: authenticatorC });
    await setPresence(authenticatorB, true);
    const secondSession = await loginWithCurrentAuthenticator();
    assert.equal(secondSession.authenticated, true);
    assert.equal(secondSession.credential_id, secondPasskeyId);

    const preReauthenticationCookie = await ownerCookie();
    assert.ok(preReauthenticationCookie);
    const reauthenticated = await evaluate(`(async () => {
      const api = async (path, options = {}) => {
        const response = await fetch(path, {
          ...options,
          cache: 'no-store',
          credentials: 'same-origin',
          headers: { 'content-type': 'application/json', ...(options.headers || {}) },
        });
        const text = await response.text();
        let payload = null;
        try { payload = text ? JSON.parse(text) : null; } catch (_) { payload = text; }
        return { status: response.status, payload };
      };
      const start = await api('/api/v1/auth/passkey/reauth/start', { method: 'POST' });
      if (start.status !== 200) throw new Error('reauthentication start failed: ' + JSON.stringify(start));
      const credential = await navigator.credentials.get(
        globalThis.FuckIPV6NodeUi.decodeRequestOptions(start.payload),
      );
      if (!credential) throw new Error('reauthentication credential missing');
      const finishBody = JSON.stringify({
        credential: globalThis.FuckIPV6NodeUi.encodeAuthenticationCredential(credential),
      });
      const finish = await api('/api/v1/auth/passkey/reauth/finish', {
        method: 'POST',
        body: finishBody,
      });
      if (finish.status !== 200) throw new Error('reauthentication finish failed: ' + JSON.stringify(finish));
      const replay = await api('/api/v1/auth/passkey/reauth/finish', {
        method: 'POST',
        body: finishBody,
      });
      return { session: finish.payload, replayStatus: replay.status };
    })()`);
    assert.equal(reauthenticated.session.authenticated, true);
    assert.ok(reauthenticated.replayStatus >= 400 && reauthenticated.replayStatus < 500);
    const rotatedCookie = await ownerCookie();
    assert.ok(rotatedCookie);
    assert.notEqual(rotatedCookie, preReauthenticationCookie);

    const oldCookieResponse = await fetch(`${baseUrl}/api/v1/auth/passkeys`, {
      headers: { cookie: `fuckipv6_owner_session=${preReauthenticationCookie}` },
    });
    assert.equal(oldCookieResponse.status, 401, "reauth must invalidate the previous session cookie");

    async function revokePasskey(credentialId) {
      return evaluate(`(async () => {
        const response = await fetch(${JSON.stringify(`/api/v1/auth/passkeys/${encodeURIComponent(credentialId)}`)}, {
          method: 'DELETE', cache: 'no-store', credentials: 'same-origin',
        });
        const text = await response.text();
        let payload = null;
        try { payload = text ? JSON.parse(text) : null; } catch (_) { payload = text; }
        return { status: response.status, payload };
      })()`);
    }

    const revokeFirst = await revokePasskey(firstPasskeyId);
    assert.equal(revokeFirst.status, 200);
    assert.equal(revokeFirst.payload.current_session_revoked, false);
    const revokeThird = await revokePasskey(thirdPasskeyId);
    assert.equal(revokeThird.status, 200);
    assert.equal(revokeThird.payload.current_session_revoked, false);
    const revokeLast = await revokePasskey(secondPasskeyId);
    assert.equal(revokeLast.status, 409, "the last active Passkey must not be revocable");

    const finalList = await evaluate(`(async () => {
      const response = await fetch('/api/v1/auth/passkeys', { cache: 'no-store', credentials: 'same-origin' });
      return { status: response.status, payload: await response.json() };
    })()`);
    assert.equal(finalList.status, 200);
    assert.equal(finalList.payload.passkeys.filter((passkey) => passkey.revoked_at_unix_ms == null).length, 1);

    console.log("test_webui_webauthn_browser: passed (Chrome virtual authenticator)");
  } finally {
    cdp?.close();
    if (chrome.exitCode === null) chrome.kill("SIGKILL");
    await new Promise((resolve) => {
      if (chrome.exitCode !== null) return resolve();
      chrome.once("exit", resolve);
      setTimeout(resolve, 2_000);
    });
    rmSync(userDataDir, { recursive: true, force: true });
  }
}

await main();
