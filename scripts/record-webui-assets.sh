#!/usr/bin/env bash
#
# Release gate: record the hash of every embedded WebUI asset.
#
# The bundle is compiled into the binary with include_str!, so the release record
# must show which files went in. Android assets are recorded by the Android gate
# from its own APK; this covers the browser surface.
set -Eeuo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DIST="${WEBUI_DIST_DIR:-${ROOT}/crates/client/src/webui/dist}"
OUT_DIR="${RELEASE_MATERIAL_DIR:-${ROOT}/target/release-material}"
mkdir -p "$OUT_DIR"

if [[ ! -d "$DIST" ]]; then
    echo "error: webui dist not found: $DIST" >&2
    exit 2
fi

OUT="$OUT_DIR/webui-assets.sha256"
(
    cd "$DIST"
    find . -type f | LC_ALL=C sort | while read -r file; do
        sha256sum "$file"
    done
) > "$OUT"

count=$(wc -l < "$OUT")
echo "[assets] recorded $count WebUI asset hash(es) in $OUT"
head -3 "$OUT"
