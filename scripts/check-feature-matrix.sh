#!/usr/bin/env bash
#
# Build the feature matrix the release gate asks for (W18-02 item 4).
#
# `--all-features` proves one combination; it says nothing about whether each
# supported subset still compiles. This script checks each subset the crate
# declares, records PASS/FAIL per combination, and exits non-zero if any of them
# fails so the same command can run in CI.
set -Eeuo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

REPORT_DIR="${FEATURE_MATRIX_REPORT_DIR:-${ROOT}/target/feature-matrix}"
mkdir -p "$REPORT_DIR"

# name|extra args
COMBINATIONS=(
    "default|"
    "no-default|--no-default-features"
    "gpui-shell|--no-default-features --features gpui-shell"
    "secure-lock-metrics|--features secure-lock-metrics"
    "userspace-nat|--features userspace-nat-prototype"
    "all|--all-features"
)

failures=0
summary="$REPORT_DIR/summary.tsv"
printf 'combination\tpackage\tstatus\tseconds\n' > "$summary"

for entry in "${COMBINATIONS[@]}"; do
    name="${entry%%|*}"
    args="${entry#*|}"
    log="$REPORT_DIR/${name}.log"
    started=$SECONDS
    # shellcheck disable=SC2086
    if cargo check -p fuckipv6-client --all-targets $args >"$log" 2>&1; then
        status=passed
    else
        status=failed
        failures=$((failures + 1))
    fi
    elapsed=$((SECONDS - started))
    printf '%s\tfuckipv6-client\t%s\t%s\n' "$name" "$status" "$elapsed" | tee -a "$summary"
done

echo "[feature-matrix] report: $summary"
if [ "$failures" -ne 0 ]; then
    echo "[feature-matrix] $failures combination(s) failed" >&2
    exit 1
fi
echo "[feature-matrix] all combinations passed"
