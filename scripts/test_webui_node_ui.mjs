#!/usr/bin/env node
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { createRequire } from "node:module";
import path from "node:path";
import { fileURLToPath } from "node:url";
import vm from "node:vm";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const require = createRequire(import.meta.url);
const catalog = JSON.parse(
  readFileSync(path.join(root, "crates/common/assets/i18n.json"), "utf8"),
);
const runtimeSource = readFileSync(
  path.join(root, "crates/common/assets/i18n-runtime.js"),
  "utf8",
).replace("__FUCKIPV6_I18N_CATALOG__", JSON.stringify(catalog));
const runtimeContext = vm.createContext({});
vm.runInContext(runtimeSource, runtimeContext, { filename: "i18n-runtime.js" });
globalThis.FuckIPV6I18n = runtimeContext.FuckIPV6I18n;
const ui = require(path.join(root, "crates/client/src/webui/node_ui.js"));
const uiSource = readFileSync(path.join(root, "crates/client/src/webui/node_ui.js"), "utf8");

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
  ["uninitialized", "未初始化", "Not initialized"],
  ["stopped", "已停止", "Stopped"],
  ["starting", "正在启动", "Starting"],
  ["stopping", "正在停止", "Stopping"],
  ["running", "运行正常", "Running normally"],
  ["degraded", "运行降级", "Degraded"],
  ["failed", "节点故障", "Node failure"],
  ["control_unavailable", "控制服务异常", "Control service unavailable"],
];

for (const [id, zhLabel, enLabel] of cases) {
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
  const zhPrimary = ui.primaryStatusFor(input, "zh-CN");
  const enPrimary = ui.primaryStatusFor(input, "en");
  assert.equal(zhPrimary.id, id);
  assert.equal(zhPrimary.label, zhLabel);
  assert.ok(zhPrimary.detail.length > 0);
  assert.equal(enPrimary.id, id);
  assert.equal(enPrimary.label, enLabel);
  assert.ok(enPrimary.detail.length > 0);
}

const overview = ui.projectPage({ id: "overview", title_key: "page.overview", actions: [] }, snapshot({
  lifecycle: { ...snapshot().lifecycle, phase: "running" },
  control_health: "unavailable",
}), null, "zh-CN");
assert.equal(overview.rows[0].title, "运行状态");
assert.match(overview.rows[0].detail, /控制服务异常/);
assert.ok(!overview.rows.some((row) => row.title === "节点生命周期"));
const overviewEn = ui.projectPage({ id: "overview", title_key: "page.overview", actions: [] }, snapshot({
  lifecycle: { ...snapshot().lifecycle, phase: "running" },
  control_health: "unavailable",
}), null, "en");
assert.equal(overviewEn.rows[0].title, "Runtime status");
assert.match(overviewEn.rows[0].detail, /Control service unavailable/);

const topologySnapshot = snapshot({
  capabilities: {
    capabilities: [
      { kind: "coordinator", state: "disabled" },
      { kind: "relay", state: "enabled" },
      { kind: "exit", state: "degraded" },
      { kind: "file", state: "unsupported" },
    ],
  },
  networks: [{
    name: "home",
    status: "online",
    roles: ["relay"],
    peer_count: 2,
  }],
  peers: [
    {
      peer_id: "peer-online",
      network_name: "home",
      virtual_ipv6: "fd00::2",
      online: true,
      p2p_active: true,
      p2p_healthy: true,
      rtt_us: 12345,
      loss_rate: 0.025,
    },
    {
      peer_id: "peer-offline",
      network_name: "home",
      virtual_ipv6: "fd00::3",
      online: false,
      p2p_active: false,
      p2p_healthy: false,
      rtt_us: null,
      loss_rate: 1,
    },
  ],
});
const topologyPage = { id: "topology", title_key: "page.topology", actions: [] };
const topology = ui.projectPage(topologyPage, topologySnapshot, "home", "zh-CN");
assert.equal(topology.rows.length, 3);
assert.equal(topology.rows[0].kind, "network");
assert.match(topology.rows[0].detail, /主状态 在线/);
assert.match(topology.rows[0].detail, /本节点角色 中继/);
assert.match(topology.rows[0].detail, /Relay: 已启用/);
assert.match(topology.rows[0].detail, /Exit: 降级/);
assert.match(topology.rows[0].detail, /Peer 能力：接口未提供/);
assert.equal(topology.rows[1].peer_id, "peer-online");
assert.match(topology.rows[1].detail, /在线/);
assert.match(topology.rows[1].detail, /P2P/);
assert.match(topology.rows[1].detail, /P2P 健康 健康/);
assert.match(topology.rows[1].detail, /RTT 12\.35 ms/);
assert.match(topology.rows[1].detail, /丢包 2\.50%/);
assert.equal(topology.rows[2].peer_id, "peer-offline");
assert.match(topology.rows[2].detail, /离线/);
assert.match(topology.rows[2].detail, /中继/);
assert.match(topology.rows[2].detail, /P2P 健康 异常/);
assert.match(topology.rows[2].detail, /RTT 不适用/);
assert.match(topology.rows[2].detail, /丢包 100\.00%/);
const topologyEn = ui.projectPage(topologyPage, topologySnapshot, "home", "en");
assert.match(topologyEn.rows[0].detail, /Primary status Online/);
assert.match(topologyEn.rows[0].detail, /Local roles Relay/);
assert.match(topologyEn.rows[0].detail, /Peer capabilities: not provided by the interface/);
assert.match(topologyEn.rows[1].detail, /P2P health Healthy/);
assert.match(topologyEn.rows[2].detail, /· Relay ·/);

const validSnapshot = {
  revision: 1,
  lifecycle: { phase: "running", configured_networks: 0, active_networks: 0, failed_networks: [] },
  control_health: "available",
  identity: null,
  configuration: { networks: [] },
  exit_node_capabilities: {
    schema_version: 1,
    platform: "linux",
    supported: true,
    ipv4_forwarding: true,
    ipv6_forwarding: true,
    nat: true,
    cleanup: true,
    unsupported_reasons: [],
  },
  capabilities: {
    capabilities: [
      { kind: "coordinator", state: "enabled" },
      { kind: "relay", state: "enabled" },
      { kind: "exit", state: "degraded" },
      { kind: "file", state: "unsupported" },
    ],
  },
  permissions: { access_model: "trusted_control_plane", permissions: ["view_state"] },
  networks: [],
  peers: [],
  logs: [],
  embedded_services: [],
  ui_mode: null,
  last_error: null,
};
assert.equal(ui.validateSnapshotResponse({ schema_version: 1, snapshot: validSnapshot }), validSnapshot);
const missingCapability = structuredClone(validSnapshot);
missingCapability.capabilities.capabilities = missingCapability.capabilities.capabilities.filter((item) => item.kind !== "file");
assert.throws(
  () => ui.validateSnapshotResponse({ schema_version: 1, snapshot: missingCapability }),
  /snapshot capability kind is missing: file/,
);
const duplicateCapability = structuredClone(validSnapshot);
duplicateCapability.capabilities.capabilities[3].kind = "exit";
assert.throws(
  () => ui.validateSnapshotResponse({ schema_version: 1, snapshot: duplicateCapability }),
  /duplicate snapshot capability kind: exit/,
);
const unknownCapabilityState = structuredClone(validSnapshot);
unknownCapabilityState.capabilities.capabilities[0].state = "mystery";
assert.throws(
  () => ui.validateSnapshotResponse({ schema_version: 1, snapshot: unknownCapabilityState }),
  /unknown snapshot capability state: mystery/,
);

const diagnosticsAction = ui.ACTION_CONTRACT.get_node_diagnostics;
assert.deepEqual(
  ui.commandForAction(diagnosticsAction, {}, "zh-CN"),
  { command: "get_node_diagnostics" },
);

const cleanupAction = ui.ACTION_CONTRACT.cleanup_node_residuals;
assert.deepEqual(
  ui.commandForAction(cleanupAction, { networks: "home, office", all: false }, "zh-CN"),
  { command: "cleanup_node_residuals", payload: { networks: ["home", "office"], all: false } },
);
assert.deepEqual(
  ui.commandForAction(cleanupAction, { networks: "ignored", all: true }, "zh-CN"),
  { command: "cleanup_node_residuals", payload: { networks: [], all: true } },
);
assert.deepEqual(
  ui.validateActionValues(cleanupAction, { networks: "", all: false }, "zh-CN"),
  ["清理时必须指定网络名称，或勾选全部网络。"],
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
assert.match(ui.outcomeSummary(diagnosticsOutcome, "zh-CN"), /可重试清理 1 个网络/);
assert.match(ui.outcomeSummary(diagnosticsOutcome, "en"), /1 networks can be retried for cleanup/);

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
assert.match(ui.outcomeSummary(cleanupOutcome, "zh-CN"), /成功 0 个 · 失败 1 个/);
assert.match(ui.outcomeSummary(cleanupOutcome, "en"), /0 succeeded · 1 failed/);

assert.equal(
  ui.httpErrorText({
    problem: {
      code: "permission_denied",
      message: "raw backend message",
      message_key: "error.operation_permission_denied",
    },
  }, "error.request_failed", "en"),
  "The current identity is not allowed to perform this operation.",
);
assert.equal(
  ui.validateActionValues(cleanupAction, { networks: "", all: false }, "en")[0],
  "Select network names or all networks before cleanup.",
);

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
assert.doesNotMatch(html, /localStorage|sessionStorage|Authorization|Bearer /);
assert.doesNotMatch(html, /\?token=|&token=/);
assert.match(html, /<script src="\/assets\/i18n\.js"><\/script>/);
assert.match(html, /credentials: "same-origin"/);
assert.match(html, /\/api\/v1\/auth\/passkey\/authentication\/start/);
assert.match(html, /\/api\/v1\/auth\/passkey\/authentication\/finish/);
assert.match(html, /\/api\/v1\/auth\/passkey\/reauth\/start/);
assert.match(html, /\/api\/v1\/auth\/passkey\/reauth\/finish/);
assert.match(html, /\/api\/v1\/auth\/session/);
assert.match(html, /\/api\/v1\/auth\/passkey\/logout/);
assert.match(html, /\/api\/v1\/auth\/passkeys/);
assert.match(html, /\/api\/v1\/auth\/passkeys\/registration\/start/);
assert.match(html, /\/api\/v1\/auth\/passkeys\/registration\/finish/);
assert.match(html, /\/api\/v1\/auth\/passkeys\/\$\{encodeURIComponent\(credentialId\)\}/);
assert.match(html, /\/api\/node\/events\?schema_version=1/);
assert.match(html, /function applyLocalizedDocument\(\)/);
assert.match(html, /id="language-control"/);
assert.match(html, /id="language-preference"/);
assert.match(html, /data-i18n="common\.language"/);
assert.match(html, /\/api\/v1\/settings\/language/);
assert.match(html, /method: "PUT"/);
assert.match(html, /function loadLanguagePreference\(\)/);
assert.match(html, /function changeLanguagePreference\(\)/);
assert.match(html, /i18n\.resolveLocale\("system", i18n\.detectLocale\(\)\)/);
assert.match(html, /error\.language_persistence_failed/);
assert.doesNotMatch(html, /authCopy|owner_auth_copy/);
assert.match(html, /function expireSession\(message\)/);
assert.match(html, /pending\.resolve\(authenticated\)/);
assert.match(html, /expected_origin/);
assert.match(html, /recent_authentication_required/);
assert.match(html, /payload\.code === "recent_authentication_required"/);
assert.doesNotMatch(html, /payload\.error\.includes/);
assert.match(html, /last_passkey/);
const inlineScripts = [...html.matchAll(/<script nonce="__CSP_NONCE__">([\s\S]*?)<\/script>/g)];
assert.equal(inlineScripts.length, 1, "node UI must have one inline renderer script");
assert.doesNotMatch(inlineScripts[0][1], /\p{Script=Han}/u, "node renderer JavaScript must not hard-code Chinese copy");
const hardCodedStaticCopy = html
  .split("\n")
  .filter((line) => /\p{Script=Han}/u.test(line) && !/data-i18n(?:-|=)/.test(line));
assert.deepEqual(hardCodedStaticCopy, [], "static Chinese copy must be declared through data-i18n attributes");
new Function(inlineScripts[0][1]);

assert.doesNotMatch(uiSource, /LABELS_BEGIN|LABELS_END|ERROR_TEXT_BEGIN|ERROR_TEXT_END/);
assert.match(uiSource, /root\.FuckIPV6I18n/);

for (const relative of [
  "crates/client/src/webui/owner_bootstrap.html",
  "crates/client/src/owner_security/owner_recovery.html",
]) {
  const surface = readFileSync(path.join(root, relative), "utf8");
  assert.match(surface, /<script src="\/assets\/i18n\.js"><\/script>/);
  assert.match(surface, /data-i18n="common\.language"/);
  assert.doesNotMatch(surface, /localStorage|sessionStorage|Authorization|Bearer /);
}

const recoveryAssets = [
  "crates/client/src/owner_security/owner_recovery.html",
  "crates/client/src/owner_security/owner_recovery.js",
].map((relative) => readFileSync(path.join(root, relative), "utf8"));
for (const asset of recoveryAssets) {
  assert.doesNotMatch(asset, /\p{Script=Han}/u, "recovery assets must not hard-code Chinese copy");
}
const recoveryScript = recoveryAssets[1];
assert.match(recoveryScript, /FuckIPV6I18n/);
assert.match(recoveryScript, /applyDocument/);
assert.match(recoveryScript, /resolveLocale/);
assert.match(recoveryScript, /statusState/);
assert.doesNotMatch(recoveryScript, /payload\.error/);

console.log("test_webui_node_ui: passed");
