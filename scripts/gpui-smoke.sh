#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BIN="${FUCKIPV6_GPUI_BIN:-$ROOT/target/debug/fuckipv6}"
SMOKE_SECONDS="${GPUI_SMOKE_SECONDS:-5}"

if [[ ! -x "$BIN" ]]; then
  echo "gpui-smoke: binary not found or not executable: $BIN" >&2
  exit 1
fi

for command in timeout xvfb-run; do
  if ! command -v "$command" >/dev/null 2>&1; then
    echo "gpui-smoke: required command not found: $command" >&2
    exit 1
  fi
done

tmp_dir="$(mktemp -d)"
trap 'rm -rf "$tmp_dir"' EXIT

# The explicit headless override must reject GPUI before any window is opened.
set +e
FUCKIPV6_HEADLESS=true timeout --signal=TERM 10s "$BIN" --gpui \
  >"$tmp_dir/headless.log" 2>&1
headless_status=$?
set -e

cat "$tmp_dir/headless.log"
if [[ "$headless_status" -eq 0 || "$headless_status" -eq 124 || "$headless_status" -eq 143 ]]; then
  echo "gpui-smoke: headless capability gate did not fail promptly (status $headless_status)" >&2
  exit 1
fi
if ! grep -Eq "没有可用的桌面显示|no display environment" "$tmp_dir/headless.log"; then
  echo "gpui-smoke: headless failure did not report the display capability gate" >&2
  exit 1
fi

# Force X11 so a Wayland session inherited from the host cannot mask the test.
set +e
env -u WAYLAND_DISPLAY RUST_LOG="${RUST_LOG:-gpui=info,blade_graphics=info}" \
  timeout --signal=TERM "${SMOKE_SECONDS}s" \
  xvfb-run -a -s '-screen 0 1280x900x24' "$BIN" --gpui \
  >"$tmp_dir/x11.log" 2>&1
window_status=$?
set -e

cat "$tmp_dir/x11.log"
if [[ "$window_status" -ne 124 && "$window_status" -ne 143 ]]; then
  echo "gpui-smoke: native window exited before timeout (status $window_status)" >&2
  exit 1
fi
if ! grep -q "gpui::platform::linux::x11::window" "$tmp_dir/x11.log"; then
  echo "gpui-smoke: no native GPUI X11 window initialization was observed" >&2
  exit 1
fi

echo "gpui-smoke: passed (native X11 window survived ${SMOKE_SECONDS}s)"
