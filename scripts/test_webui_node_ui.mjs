#!/usr/bin/env node
import assert from "node:assert/strict";
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

console.log("test_webui_node_ui: passed");
