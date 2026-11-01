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

function placeholders(text) {
  return [...String(text).matchAll(/\{[A-Za-z0-9_.-]+\}/g)].map((match) => match[0]).sort();
}

assert.equal(i18n.detectLocale(), "zh-CN");
assert.equal(i18n.resolveLocale("en", "zh-CN"), "en");
assert.equal(i18n.resolveLocale("system", "zh-Hans-CN"), "zh-CN");
assert.equal(i18n.t("en", "common.language"), "Language");
assert.equal(i18n.t("zh-CN", "common.language"), "语言");
assert.equal(i18n.tr("en", "manifest.loaded", { version: 1, count: 11 }), "Manifest v1 · 11 pages");
assert.equal(i18n.formatInteger("en", 1234567), "1,234,567");
assert.equal(i18n.formatInteger("en", -1234567), "-1,234,567");
assert.equal(i18n.formatInteger("en", 0), "0");
assert.equal(i18n.formatInteger("en", Number.MAX_SAFE_INTEGER + 1), "N/A");
assert.equal(i18n.formatDecimal("en", 12.345, 1), "12.3");
assert.equal(i18n.formatDecimal("zh-CN", -12.345, 2), "-12.35");
assert.equal(i18n.formatDecimal("en", Number.NaN, 1), "N/A");
assert.equal(i18n.formatBytes("en", 0), "0 B");
assert.equal(i18n.formatBytes("en", 1536), "1.5 KiB");
assert.equal(i18n.formatBytes("en", 1048576), "1.0 MiB");
assert.equal(i18n.formatBytes("en", Number.POSITIVE_INFINITY), "N/A");
assert.equal(i18n.formatDuration("zh-CN", 125), "2 分 5 秒");
assert.equal(i18n.formatDuration("en", 3605), "1h 0m");
assert.equal(i18n.formatDuration("en", Number.NaN), "N/A");
assert.equal(
  i18n.formatTimestamp("zh-CN", Date.UTC(2026, 10, 1, 9, 3, 4), 480),
  "2026-11-01 17:03:04 UTC+08:00",
);
assert.equal(
  i18n.formatTimestamp("en", Date.UTC(2026, 10, 1, 9, 3, 4), 480),
  "Nov 1, 2026, 5:03:04 PM UTC+08:00",
);
assert.equal(i18n.formatTimestamp("en", Date.UTC(2026, 10, 1, 9, 3, 4), -300), "Nov 1, 2026, 4:03:04 AM UTC-05:00");
assert.equal(i18n.formatTimestamp("en", Number.NaN, 480), "N/A");
assert.equal(i18n.formatAddress("192.0.2.1", 8080), "192.0.2.1:8080");
assert.equal(i18n.formatAddress("2001:db8::1", 443), "[2001:db8::1]:443");

const pseudo = i18n.pseudoLocalize("Network {network}");
assert.ok(pseudo.startsWith("⟦"));
assert.ok(pseudo.endsWith("⟧"));
assert.ok(pseudo.includes("{network}"));
assert.ok(!pseudo.includes("{nétwórk}"));
for (const [locale, messages] of Object.entries(catalog.messages)) {
  for (const [key, original] of Object.entries(messages)) {
    const localized = i18n.pseudoLocalize(original);
    assert.ok(localized.startsWith("⟦"), `pseudo locale marker missing for ${locale}/${key}`);
    assert.ok(localized.endsWith("⟧"), `pseudo locale terminator missing for ${locale}/${key}`);
    assert.ok(localized.slice(1, -1).trim(), `pseudo locale empty for ${locale}/${key}`);
    assert.deepEqual(placeholders(localized), placeholders(original), `placeholder changed for ${locale}/${key}`);
    assert.ok(!localized.includes("\uFFFD"), `invalid Unicode in pseudo locale for ${locale}/${key}`);
  }
}

const family = "👨‍👩‍👧‍👦";
assert.equal(i18n.truncateUnicode("网络节点", 3), "网络…");
assert.equal(i18n.truncateUnicode("e\u0301xy", 2), "e\u0301…");
assert.equal(i18n.truncateUnicode("e\u0301xy", 3), "e\u0301xy");
assert.equal(i18n.truncateUnicode(family, 1), family);
assert.equal(i18n.truncateUnicode(family + "x", 1), "…");
assert.equal(i18n.truncateUnicode(family + "xy", 2), family + "…");
assert.equal(i18n.truncateUnicode(family + "x", 2), family + "x");
assert.equal(i18n.truncateUnicode("anything", 0), "");
assert.deepEqual([...i18n.locales].sort(), ["en", "zh-CN"]);
console.log("test_i18n_runtime: passed");
