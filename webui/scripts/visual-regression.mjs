#!/usr/bin/env node

import { spawn } from "node:child_process";
import { readFile, mkdir, writeFile } from "node:fs/promises";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { PNG } from "pngjs";

const WEBUI_ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const REPO_ROOT = resolve(WEBUI_ROOT, "..");
const OUTPUT_ROOT = resolve(
  process.env.VISUAL_REGRESSION_DIR ?? join(REPO_ROOT, "target/visual-regression"),
);
const GPUI_OUTPUT_ROOT = join(OUTPUT_ROOT, "gpui");
const WEBUI_OUTPUT_ROOT = join(OUTPUT_ROOT, "webui");
const GPUI_IMAGE = join(GPUI_OUTPUT_ROOT, "overview.png");
const GPUI_METADATA = join(GPUI_OUTPUT_ROOT, "overview.json");
const GPUI_GEOMETRY = join(GPUI_OUTPUT_ROOT, "overview.geometry.json");
const WEBUI_IMAGE = join(WEBUI_OUTPUT_ROOT, "overview.png");
const WEBUI_METADATA = join(WEBUI_OUTPUT_ROOT, "overview.json");
const DIFF_IMAGE = join(OUTPUT_ROOT, "diff.png");
const REPORT = join(OUTPUT_ROOT, "visual-regression.json");
const SKIP_BUILD = process.argv.includes("--skip-build");

const THRESHOLDS = {
  geometryPx: numberFromEnv("VISUAL_GEOMETRY_TOLERANCE_PX", 1.5),
  pixelMae: numberFromEnv("VISUAL_PIXEL_MAE_MAX", 10),
  pixelRmse: numberFromEnv("VISUAL_PIXEL_RMSE_MAX", 22),
  pixelsAbove16Ratio: numberFromEnv("VISUAL_PIXEL_ABOVE_16_RATIO_MAX", 0.23),
  pixelsAbove32Ratio: numberFromEnv("VISUAL_PIXEL_ABOVE_32_RATIO_MAX", 0.03),
};

const TEXT_METRIC_GEOMETRY_ALLOWANCES = {
  hero_copy: { width: 2.5 },
};

const SEMANTIC_KEYS = [
  "title",
  "locale",
  "font_family",
  "viewport",
  "modules",
  "page_tabs",
  "primary_status",
  "page_heading",
  "action_panel",
  "actions",
  "shortcuts",
  "network_statuses",
];

function numberFromEnv(name, fallback) {
  const raw = process.env[name];
  if (raw === undefined || raw === "") return fallback;
  const value = Number(raw);
  if (!Number.isFinite(value) || value < 0) {
    throw new Error(`${name} must be a non-negative finite number`);
  }
  return value;
}

function run(command, args, options = {}) {
  return new Promise((resolvePromise, reject) => {
    const child = spawn(command, args, {
      cwd: REPO_ROOT,
      stdio: "inherit",
      ...options,
      env: { ...process.env, ...options.env },
    });
    child.once("error", reject);
    child.once("exit", (code, signal) => {
      if (code === 0) resolvePromise();
      else reject(new Error(`${command} ${args.join(" ")} exited with ${signal ?? code}`));
    });
  });
}

async function readJson(path) {
  return JSON.parse(await readFile(path, "utf8"));
}

function compareSemantics(gpui, webui) {
  const differences = [];
  for (const key of SEMANTIC_KEYS) {
    const left = JSON.stringify(gpui?.[key]);
    const right = JSON.stringify(webui?.[key]);
    if (left !== right) {
      differences.push({ key, gpui: gpui?.[key], webui: webui?.[key] });
    }
  }
  return differences;
}

function geometryTolerance(key, field, baseTolerancePx) {
  return Math.max(baseTolerancePx, TEXT_METRIC_GEOMETRY_ALLOWANCES[key]?.[field] ?? 0);
}

function compareGeometry(gpui, webui, tolerancePx) {
  const differences = [];
  const gpuiKeys = Object.keys(gpui ?? {}).sort();
  const webuiKeys = Object.keys(webui ?? {}).sort();
  if (JSON.stringify(gpuiKeys) !== JSON.stringify(webuiKeys)) {
    differences.push({ key: "<keys>", gpui: gpuiKeys, webui: webuiKeys });
    return { checkedProperties: 0, maxDeltaPx: null, differences };
  }

  let checkedProperties = 0;
  let maxDeltaPx = 0;
  for (const key of gpuiKeys) {
    const left = gpui[key];
    const right = webui[key];
    if (!left || !right) {
      if (left !== right) differences.push({ key, gpui: left, webui: right });
      continue;
    }
    for (const field of ["x", "y", "width", "height"]) {
      checkedProperties += 1;
      const delta = Math.abs(Number(left[field]) - Number(right[field]));
      if (Number.isFinite(delta)) maxDeltaPx = Math.max(maxDeltaPx, delta);
      const allowance = geometryTolerance(key, field, tolerancePx);
      if (!Number.isFinite(delta) || delta > allowance) {
        differences.push({
          key,
          field,
          gpui: left[field],
          webui: right[field],
          delta: Number.isFinite(delta) ? Number(delta.toFixed(4)) : null,
          tolerance_px: allowance,
        });
      }
    }
  }
  return {
    checkedProperties,
    maxDeltaPx: Number(maxDeltaPx.toFixed(4)),
    differences,
  };
}

function comparePixels(gpuiBuffer, webuiBuffer) {
  const left = PNG.sync.read(gpuiBuffer);
  const right = PNG.sync.read(webuiBuffer);
  if (left.width !== right.width || left.height !== right.height) {
    throw new Error(
      `screenshot dimensions differ: GPUI ${left.width}x${left.height}, WebUI ${right.width}x${right.height}`,
    );
  }

  const diff = new PNG({ width: left.width, height: left.height });
  let absoluteError = 0;
  let squaredError = 0;
  let above16 = 0;
  let above32 = 0;
  const pixelCount = left.width * left.height;

  for (let pixel = 0; pixel < pixelCount; pixel += 1) {
    const offset = pixel * 4;
    let maxChannelDelta = 0;
    for (let channel = 0; channel < 3; channel += 1) {
      const delta = Math.abs(left.data[offset + channel] - right.data[offset + channel]);
      absoluteError += delta;
      squaredError += delta * delta;
      maxChannelDelta = Math.max(maxChannelDelta, delta);
    }
    if (maxChannelDelta > 16) above16 += 1;
    if (maxChannelDelta > 32) above32 += 1;

    const highlighted = Math.min(255, maxChannelDelta * 4);
    diff.data[offset] = 255;
    diff.data[offset + 1] = 255 - highlighted;
    diff.data[offset + 2] = 255 - highlighted;
    diff.data[offset + 3] = 255;
  }

  return {
    width: left.width,
    height: left.height,
    pixel_count: pixelCount,
    mae: absoluteError / (pixelCount * 3),
    rmse: Math.sqrt(squaredError / (pixelCount * 3)),
    pixels_above_16_ratio: above16 / pixelCount,
    pixels_above_32_ratio: above32 / pixelCount,
    diff,
  };
}

function roundedPixelMetrics(metrics) {
  return {
    width: metrics.width,
    height: metrics.height,
    pixel_count: metrics.pixel_count,
    mae: Number(metrics.mae.toFixed(6)),
    rmse: Number(metrics.rmse.toFixed(6)),
    pixels_above_16_ratio: Number(metrics.pixels_above_16_ratio.toFixed(6)),
    pixels_above_32_ratio: Number(metrics.pixels_above_32_ratio.toFixed(6)),
  };
}

function pixelFailures(metrics) {
  const failures = [];
  if (metrics.mae > THRESHOLDS.pixelMae) failures.push("mae");
  if (metrics.rmse > THRESHOLDS.pixelRmse) failures.push("rmse");
  if (metrics.pixels_above_16_ratio > THRESHOLDS.pixelsAbove16Ratio) {
    failures.push("pixels_above_16_ratio");
  }
  if (metrics.pixels_above_32_ratio > THRESHOLDS.pixelsAbove32Ratio) {
    failures.push("pixels_above_32_ratio");
  }
  return failures;
}

async function main() {
  await mkdir(GPUI_OUTPUT_ROOT, { recursive: true });
  await mkdir(WEBUI_OUTPUT_ROOT, { recursive: true });

  await run(
    "cargo",
    [
      "test",
      "-p",
      "fuckipv6-client",
      "--all-features",
      "--lib",
      "writes_headless_product_shell_screenshot_when_requested",
      "--",
      "--nocapture",
    ],
    { env: { GPUI_VISUAL_SNAPSHOT: GPUI_IMAGE } },
  );

  await run(
    "cargo",
    [
      "test",
      "-p",
      "fuckipv6-client",
      "--all-features",
      "--lib",
      "reports_product_shell_geometry_for_visual_regression",
      "--",
      "--nocapture",
    ],
    {
      env: {
        GPUI_VISUAL_GEOMETRY: "1",
        GPUI_VISUAL_GEOMETRY_JSON: GPUI_GEOMETRY,
      },
    },
  );

  const snapshotArgs = ["run", "visual:snapshot", "--prefix", "webui"];
  if (SKIP_BUILD) snapshotArgs.push("--", "--skip-build");
  await run("npm", snapshotArgs, {
    env: { VISUAL_OUTPUT_DIR: WEBUI_OUTPUT_ROOT },
  });

  const [gpuiMetadata, webuiMetadata, gpuiGeometry, webuiBuffer, gpuiBuffer] =
    await Promise.all([
      readJson(GPUI_METADATA),
      readJson(WEBUI_METADATA),
      readJson(GPUI_GEOMETRY),
      readFile(WEBUI_IMAGE),
      readFile(GPUI_IMAGE),
    ]);

  const semanticDifferences = compareSemantics(
    gpuiMetadata.semantics,
    webuiMetadata.semantics,
  );
  const geometryComparison = compareGeometry(
    gpuiGeometry,
    webuiMetadata.semantics.geometry,
    THRESHOLDS.geometryPx,
  );
  const geometryDifferences = geometryComparison.differences;
  const metrics = comparePixels(gpuiBuffer, webuiBuffer);
  const failedPixelChecks = pixelFailures(metrics);
  const passed =
    semanticDifferences.length === 0 &&
    geometryDifferences.length === 0 &&
    failedPixelChecks.length === 0;

  await writeFile(DIFF_IMAGE, PNG.sync.write(metrics.diff));
  const report = {
    schema_version: 1,
    result: passed ? "passed" : "failed",
    fixture: {
      viewport: { width: 1440, height: 900, device_scale_factor: 2 },
      locale: "en",
      page: "overview",
    },
    acceptable_differences: [
      "platform font rasterization and subpixel text metrics",
      "native versus browser scrollbar painting when content scrolls",
      "IME composition UI, which is not shown in an idle screenshot",
    ],
    thresholds: THRESHOLDS,
    semantics: {
      passed: semanticDifferences.length === 0,
      differences: semanticDifferences,
    },
    geometry: {
      passed: geometryDifferences.length === 0,
      tolerance_px: THRESHOLDS.geometryPx,
      text_metric_allowances: TEXT_METRIC_GEOMETRY_ALLOWANCES,
      checked_properties: geometryComparison.checkedProperties,
      max_delta_px: geometryComparison.maxDeltaPx,
      differences: geometryDifferences,
    },
    pixels: {
      ...roundedPixelMetrics(metrics),
      passed: failedPixelChecks.length === 0,
      failed_checks: failedPixelChecks,
    },
    metadata: {
      gpui: {
        platform: gpuiMetadata.platform,
        fixture_revision: gpuiMetadata.fixture_revision,
        dimensions: gpuiMetadata.dimensions,
      },
      webui: {
        platform: webuiMetadata.platform,
        fixture_revision: webuiMetadata.fixture_revision,
        dimensions: webuiMetadata.dimensions,
        chrome: webuiMetadata.chrome,
      },
    },
    artifacts: {
      gpui: GPUI_IMAGE,
      webui: WEBUI_IMAGE,
      diff: DIFF_IMAGE,
      report: REPORT,
    },
  };
  await writeFile(REPORT, `${JSON.stringify(report, null, 2)}\n`);

  process.stdout.write(`${JSON.stringify(report, null, 2)}\n`);
  if (!passed) {
    throw new Error("cross-surface visual regression failed");
  }
}

main().catch((error) => {
  process.stderr.write(`${error.stack ?? error}\n`);
  process.exitCode = 1;
});
