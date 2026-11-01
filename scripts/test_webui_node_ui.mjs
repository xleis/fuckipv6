#!/usr/bin/env node
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { createRequire } from "node:module";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const require = createRequire(import.meta.url);
const ui = require(path.join(root, "crates/client/src/webui/node_ui.js"));

function snapshot(overrides = {}) {
  return {
    revision: 1,
    lifecycle: {
      phase: "stopped",
      configured_networks: 0,
      active_networks: 0,
      failed_networks: [],
    },
    control_health: "available",
    identity: null,
    configuration: { networks: [] },
    networks: [],
    peers: [],
    embedded_services: [],
    last_error: null,
    ...overrides,
  };
}

const cases = [
  ["uninitialized", "未初始化"],
  ["stopped", "已停止"],
  ["starting", "正在启动"],
  ["stopping", "正在停止"],
  ["running", "运行正常"],
  ["degraded", "运行降级"],
  ["failed", "节点故障"],
  ["control_unavailable", "控制服务异常"],
];

for (const [id, label] of cases) {
  let input;
  if (id === "uninitialized") {
    input = snapshot();
  } else if (id === "stopped") {
    input = snapshot({ identity: { node_id: "node-1", display_name: null, public_key_fingerprint: "fixture" } });
  } else if (id === "starting" || id === "stopping" || id === "failed") {
    input = snapshot({ lifecycle: { ...snapshot().lifecycle, phase: id } });
  } else if (id === "running") {
    input = snapshot({ lifecycle: { ...snapshot().lifecycle, phase: "running" } });
  } else if (id === "degraded") {
    input = snapshot({
      lifecycle: { ...snapshot().lifecycle, phase: "running" },
      networks: [{ name: "home", status: "degraded" }],
      control_health: "unavailable",
    });
  } else {
    input = snapshot({
      lifecycle: { ...snapshot().lifecycle, phase: "running" },
      control_health: "unavailable",
    });
  }
  const primary = ui.primaryStatusFor(input);
  assert.equal(primary.id, id);
  assert.equal(primary.label, label);
  assert.ok(primary.detail.length > 0);
}

const overview = ui.projectPage({ id: "overview", title_key: "page.overview", actions: [] }, snapshot({
  lifecycle: { ...snapshot().lifecycle, phase: "running" },
  control_health: "unavailable",
}), null);
assert.equal(overview.rows[0].title, "运行状态");
assert.match(overview.rows[0].detail, /控制服务异常/);
assert.ok(!overview.rows.some((row) => row.title === "节点生命周期"));

const diagnosticsAction = ui.ACTION_CONTRACT.get_node_diagnostics;
assert.deepEqual(
  ui.commandForAction(diagnosticsAction, {}),
  { command: "get_node_diagnostics" },
);

const cleanupAction = ui.ACTION_CONTRACT.cleanup_node_residuals;
assert.deepEqual(
  ui.commandForAction(cleanupAction, { networks: "home, office", all: false }),
  { command: "cleanup_node_residuals", payload: { networks: ["home", "office"], all: false } },
);
assert.deepEqual(
  ui.commandForAction(cleanupAction, { networks: "ignored", all: true }),
  { command: "cleanup_node_residuals", payload: { networks: [], all: true } },
);
assert.deepEqual(
  ui.validateActionValues(cleanupAction, { networks: "", all: false }),
  ["清理时必须指定网络名称，或勾选全部网络"],
);
assert.equal(ui.defaultFieldValue(cleanupAction, cleanupAction.fields[1], snapshot(), ""), true);

const residue = {
  sessions_remaining: 1,
  listeners_remaining: 1,
  peers_remaining: 2,
  embedded_services_remaining: 0,
  configuration_retained: true,
  residual_networks: ["home"],
  cleanup_steps: [{
    network_name: "home",
    resource: "TUN",
    status: "deferred",
    detail: "清理已延迟",
  }],
  issues: ["残留网络: home"],
};
const diagnosticsResponse = {
  schema_version: 1,
  result: {
    request_id: 7,
    outcome: {
      status: "diagnostics",
      data: {
        lifecycle: snapshot().lifecycle,
        retained_owner_networks: ["home"],
        cleanup_candidate_networks: ["home"],
        residue,
      },
    },
  },
};
const diagnosticsOutcome = ui.validateCommandResponse(diagnosticsResponse, 7);
assert.match(ui.outcomeSummary(diagnosticsOutcome), /可重试清理 1 个网络/);

const cleanupResponse = {
  schema_version: 1,
  result: {
    request_id: 8,
    outcome: {
      status: "cleanup",
      data: {
        cleaned_networks: [],
        failed_networks: [{ network_name: "home", reason: "timed_out" }],
        residue,
      },
    },
  },
};
const cleanupOutcome = ui.validateCommandResponse(cleanupResponse, 8);
assert.match(ui.outcomeSummary(cleanupOutcome), /成功 0 个 · 失败 1 个/);

const invalidCleanupResponse = structuredClone(cleanupResponse);
invalidCleanupResponse.result.outcome.data.failed_networks[0].reason = "mystery";
assert.throws(
  () => ui.validateCommandResponse(invalidCleanupResponse, 8),
  /unknown cleanup failure reason/,
);


const binary = new Uint8Array([0, 1, 2, 250]).buffer;
assert.equal(ui.bufferToBase64url(binary), "AAEC-g");
assert.deepEqual([...new Uint8Array(ui.base64urlToBuffer("AAEC-g"))], [0, 1, 2, 250]);
assert.throws(() => ui.base64urlToBuffer(""), /base64url/);

const requestOptions = ui.decodeRequestOptions({
  publicKey: {
    challenge: "AQI",
    allowCredentials: [{ type: "public-key", id: "AwQ" }],
  },
});
assert.ok(requestOptions.publicKey.challenge instanceof ArrayBuffer);
assert.equal(requestOptions.publicKey.allowCredentials[0].id.byteLength, 2);

const authentication = ui.encodeAuthenticationCredential({
  id: "credential-id",
  rawId: new Uint8Array([1, 2]).buffer,
  type: "public-key",
  response: {
    authenticatorData: new Uint8Array([3]).buffer,
    clientDataJSON: new Uint8Array([4]).buffer,
    signature: new Uint8Array([5]).buffer,
    userHandle: new Uint8Array([6]).buffer,
  },
  getClientExtensionResults: () => ({ appid: true }),
});
assert.deepEqual(authentication, {
  id: "credential-id",
  rawId: "AQI",
  type: "public-key",
  response: {
    authenticatorData: "Aw",
    clientDataJSON: "BA",
    signature: "BQ",
    userHandle: "Bg",
  },
  clientExtensionResults: { appid: true },
});

const creationOptions = ui.decodeCreationOptions({
  publicKey: {
    challenge: "AQI",
    user: { id: "AwQ", name: "owner", displayName: "Owner" },
    excludeCredentials: [{ type: "public-key", id: "BQ" }],
  },
});
assert.equal(creationOptions.publicKey.user.id.byteLength, 2);
assert.equal(creationOptions.publicKey.excludeCredentials[0].id.byteLength, 1);

const registration = ui.encodeRegistrationCredential({
  id: "new-credential",
  rawId: new Uint8Array([7]).buffer,
  type: "public-key",
  response: {
    attestationObject: new Uint8Array([8]).buffer,
    clientDataJSON: new Uint8Array([9]).buffer,
    getTransports: () => ["internal", "hybrid"],
  },
  getClientExtensionResults: () => ({}),
});
assert.deepEqual(registration.response, {
  attestationObject: "CA",
  clientDataJSON: "CQ",
  transports: ["internal", "hybrid"],
});

const html = readFileSync(path.join(root, "crates/client/src/webui/node_ui.html"), "utf8");
assert.doesNotMatch(html, /localStorage|Authorization|Bearer /);
assert.doesNotMatch(html, /\?token=|&token=/);
assert.match(html, /credentials: "same-origin"/);
assert.match(html, /\/api\/v1\/auth\/passkey\/authentication\/start/);
assert.match(html, /\/api\/v1\/auth\/passkey\/authentication\/finish/);
assert.match(html, /\/api\/v1\/auth\/session/);
assert.match(html, /\/api\/v1\/auth\/passkey\/logout/);
assert.match(html, /\/api\/node\/events\?schema_version=1/);
assert.match(html, /function expireSession\(message\)/);
const inlineScripts = [...html.matchAll(/<script nonce="__CSP_NONCE__">([\s\S]*?)<\/script>/g)];
assert.equal(inlineScripts.length, 1, "node UI must have one inline renderer script");
new Function(inlineScripts[0][1]);

console.log("test_webui_node_ui: passed");
