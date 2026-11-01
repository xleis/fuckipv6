#!/usr/bin/env node
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import path from "node:path";
import vm from "node:vm";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const template = readFileSync(path.join(root, "crates/common/assets/i18n-runtime.js"), "utf8");
const catalog = JSON.parse(readFileSync(path.join(root, "crates/common/assets/i18n.json"), "utf8"));
const source = template.replace("__FUCKIPV6_I18N_CATALOG__", JSON.stringify(catalog));
const context = { navigator: { languages: ["zh-CN", "en"] } };
context.globalThis = context;
vm.runInNewContext(source, context);
const i18n = context.FuckIPV6I18n;

assert.equal(i18n.detectLocale(), "zh-CN");
assert.equal(i18n.resolveLocale("en", "zh-CN"), "en");
assert.equal(i18n.resolveLocale("system", "zh-Hans-CN"), "zh-CN");
assert.equal(i18n.t("en", "common.language"), "Language");
assert.equal(i18n.t("zh-CN", "common.language"), "语言");
assert.equal(i18n.tr("en", "manifest.loaded", { version: 1, count: 11 }), "Manifest v1 · 11 pages");
assert.equal(i18n.formatInteger("en", 1234567), "1,234,567");
assert.equal(i18n.formatBytes("en", 1536), "1.5 KiB");
assert.equal(i18n.formatDuration("zh-CN", 125), "2 分 5 秒");
assert.match(i18n.formatTimestamp("zh-CN", Date.UTC(2026, 10, 1, 9, 3, 4)), /UTC[+-]\d\d:\d\d$/);
assert.equal(i18n.formatAddress("2001:db8::1", 443), "[2001:db8::1]:443");
assert.ok(i18n.pseudoLocalize("Network").startsWith("⟦"));
assert.equal(i18n.truncateUnicode("A😀B", 2), "A…");
assert.deepEqual([...i18n.locales].sort(), ["en", "zh-CN"]);
console.log("test_i18n_runtime: passed");
