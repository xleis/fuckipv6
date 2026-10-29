#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT_DIR="${OUT_DIR:-"$ROOT_DIR/target/profiling"}"
BENCH="${BENCH:-protocol}"
PACKAGE="${PACKAGE:-fuckipv6-common}"
PROFILE_SECONDS="${PROFILE_SECONDS:-10}"

mkdir -p "$OUT_DIR"

if ! command -v perf >/dev/null 2>&1; then
  echo "perf is required for CPU profiling" >&2
  exit 1
fi

cd "$ROOT_DIR"

echo "Building benchmark target..."
cargo bench -p "$PACKAGE" --bench "$BENCH" --no-run

BENCH_BIN="$(
  find "$ROOT_DIR/target/release/deps" -maxdepth 1 -type f -executable -name "${BENCH}-*" -printf '%T@ %p\n' \
    | sort -nr \
    | awk 'NR == 1 { $1 = ""; sub(/^ /, ""); print }'
)"
if [[ -z "$BENCH_BIN" ]]; then
  echo "Unable to locate benchmark binary for $PACKAGE/$BENCH" >&2
  exit 1
fi

echo "Profiling $BENCH_BIN"
echo "Output directory: $OUT_DIR"

if cargo flamegraph --help >/dev/null 2>&1; then
  cargo flamegraph \
    --output "$OUT_DIR/${PACKAGE}-${BENCH}.svg" \
    --bench "$BENCH" \
    --package "$PACKAGE" \
    -- \
    --profile-time "$PROFILE_SECONDS"
  echo "Flamegraph: $OUT_DIR/${PACKAGE}-${BENCH}.svg"
else
  DATA_FILE="$OUT_DIR/${PACKAGE}-${BENCH}.perf.data"
  REPORT_FILE="$OUT_DIR/${PACKAGE}-${BENCH}.perf-report.txt"

  perf record \
    --call-graph dwarf \
    --output "$DATA_FILE" \
    "$BENCH_BIN" \
    --bench \
    --profile-time "$PROFILE_SECONDS"
  perf report \
    --stdio \
    --input "$DATA_FILE" \
    --no-children \
    --sort comm,dso,symbol \
    > "$REPORT_FILE"

  echo "cargo-flamegraph is not installed; wrote perf report instead."
  echo "Perf data: $DATA_FILE"
  echo "Perf report: $REPORT_FILE"
  echo "Install flamegraph with: cargo install flamegraph"
fi
