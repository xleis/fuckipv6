#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BIN="${FUCKIPV6_GPUI_BIN:-$ROOT/target/debug/fuckipv6}"
SMOKE_SECONDS="${GPUI_SMOKE_SECONDS:-5}"

if [[ ! -x "$BIN" ]]; then
  echo "gpui-smoke: binary not found or not executable: $BIN" >&2
  exit 1
fi

for command in timeout xvfb-run xdotool ps; do
  if ! command -v "$command" >/dev/null 2>&1; then
    echo "gpui-smoke: required command not found: $command" >&2
    exit 1
  fi
done

tmp_dir="$(mktemp -d)"
runtime_dir="$tmp_dir/runtime"
config_dir="$tmp_dir/config"
home_dir="$tmp_dir/home"
mkdir -p "$runtime_dir" "$config_dir" "$home_dir"

cleanup() {
  # GPUI starts a detached control process. It inherits the isolated runtime
  # directory, so its PID can be recovered from that directory's lock without
  # touching any instance owned by the current desktop session.
  if [[ -f "$runtime_dir/control.lock" ]]; then
    control_pid="$(head -n 1 "$runtime_dir/control.lock" 2>/dev/null || true)"
    if [[ "$control_pid" =~ ^[0-9]+$ ]] && kill -0 "$control_pid" 2>/dev/null; then
      kill -TERM -- "-$control_pid" 2>/dev/null || true
      for _ in $(seq 1 50); do
        kill -0 "$control_pid" 2>/dev/null || break
        sleep 0.1
      done
      kill -KILL -- "-$control_pid" 2>/dev/null || true
    fi
  fi
  if [[ "${GPUI_SMOKE_KEEP_TMP:-0}" == "1" ]]; then
    echo "gpui-smoke: kept temporary directory: $tmp_dir" >&2
    return
  fi
  rm -rf "$tmp_dir"
}
trap cleanup EXIT

smoke_env=(
  env -u WAYLAND_DISPLAY
  HOME="$home_dir"
  XDG_CONFIG_HOME="$config_dir"
  XDG_RUNTIME_DIR="$tmp_dir"
  FUCKIPV6_RUNTIME_DIR="$runtime_dir"
  FUCKIPV6_CONFIG_DIR="$config_dir"
)

# An environment override must not turn a desktop GPUI launch into WebUI/headless.
set +e
FUCKIPV6_HEADLESS=true "${smoke_env[@]}" timeout --signal=TERM 10s "$BIN" --gpui \
  >"$tmp_dir/override.log" 2>&1
override_status=$?
set -e

cat "$tmp_dir/override.log"
if [[ "$override_status" -eq 0 || "$override_status" -eq 124 || "$override_status" -eq 143 ]]; then
  echo "gpui-smoke: conflicting UI override did not fail promptly (status $override_status)" >&2
  exit 1
fi
if ! grep -q "mutually exclusive" "$tmp_dir/override.log"; then
  echo "gpui-smoke: conflicting UI override was not rejected explicitly" >&2
  exit 1
fi

# A desktop session must reject both browser-hosted and headless UI modes.
for mode in --webui --headless; do
  set +e
  "${smoke_env[@]}" timeout --signal=TERM 10s \
    xvfb-run -a -s '-screen 0 1280x900x24' "$BIN" "$mode" \
    >"$tmp_dir/${mode#--}.log" 2>&1
  desktop_status=$?
  set -e

  cat "$tmp_dir/${mode#--}.log"
  if [[ "$desktop_status" -eq 0 || "$desktop_status" -eq 124 || "$desktop_status" -eq 143 ]]; then
    echo "gpui-smoke: desktop $mode gate did not fail promptly (status $desktop_status)" >&2
    exit 1
  fi
  if ! grep -Eq "only available without a desktop session|仅.*无桌面" "$tmp_dir/${mode#--}.log"; then
    echo "gpui-smoke: desktop $mode failure did not report the desktop-only policy" >&2
    exit 1
  fi
done

# Force X11 so a Wayland session inherited from the host cannot mask the test.
# Inspect the isolated X server directly; logging targets are not a stable
# contract for proving that a native window was created.
set +e
"${smoke_env[@]}" timeout --signal=TERM "$((SMOKE_SECONDS + 25))s" \
  xvfb-run -a -s '-screen 0 1280x900x24' bash -c '
    set -euo pipefail
    bin="$1"
    log="$2"
    hold_seconds="$3"
    runtime_dir="$4"

    "$bin" --gpui >"$log" 2>&1 &
    app_pid=$!
    cleanup_app() {
      kill -TERM "$app_pid" 2>/dev/null || true
      wait "$app_pid" 2>/dev/null || true
    }
    trap cleanup_app EXIT

    startup_deadline=$((SECONDS + 20))
    while (( SECONDS < startup_deadline )); do
      if ! kill -0 "$app_pid" 2>/dev/null; then
        echo "gpui-smoke: GPUI process exited before creating a window" >&2
        exit 1
      fi
      if xdotool search --name "^fuckipv6$" >/dev/null 2>&1; then
        break
      fi
      sleep 0.2
    done
    if ! xdotool search --name "^fuckipv6$" >/dev/null 2>&1; then
      echo "gpui-smoke: no native GPUI X11 window was observed" >&2
      exit 1
    fi

    window_count="$(xdotool search --name "^fuckipv6$" | wc -l)"
    if [[ "$window_count" -ne 1 ]]; then
      echo "gpui-smoke: expected exactly one native GPUI window, found $window_count" >&2
      exit 1
    fi

    control_pid="$(head -n 1 "$runtime_dir/control.lock" 2>/dev/null || true)"
    if [[ ! "$control_pid" =~ ^[0-9]+$ ]] || ! kill -0 "$control_pid" 2>/dev/null; then
      echo "gpui-smoke: GPUI did not leave a live control process in the isolated runtime" >&2
      exit 1
    fi
    control_group="$(ps -eo pgid=,args= | awk -v pgid="$control_pid" "\$1 == pgid { print }")"
    if grep -q "fuckipv6 __service" <<<"$control_group"; then
      echo "gpui-smoke: desktop GPUI started the WebUI service role" >&2
      exit 1
    fi

    found_at=$SECONDS
    while (( SECONDS < found_at + hold_seconds )); do
      if ! kill -0 "$app_pid" 2>/dev/null; then
        echo "gpui-smoke: GPUI process exited while the native window was open" >&2
        exit 1
      fi
      sleep 0.2
    done
    echo "gpui-smoke: one native X11 window observed; no WebUI service role started"
  ' _ "$BIN" "$tmp_dir/x11.log" "$SMOKE_SECONDS" "$runtime_dir" \
  >"$tmp_dir/x11.log" 2>&1
window_status=$?
set -e

cat "$tmp_dir/x11.log"
if [[ "$window_status" -ne 0 ]]; then
  echo "gpui-smoke: native X11 window check failed (status $window_status)" >&2
  exit 1
fi

echo "gpui-smoke: passed (exclusive GPUI survived ${SMOKE_SECONDS}s; no WebUI service role)"
