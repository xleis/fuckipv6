#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REPORT_DIR="${SUPPLY_CHAIN_REPORT_DIR:-${ROOT}/target/supply-chain}"
DENY_CONFIG="${ROOT}/deny.toml"

if ! cargo deny --version >/dev/null 2>&1; then
    echo "error: cargo-deny is required; install cargo-deny 0.20.2" >&2
    exit 127
fi

DENY_VERSION="$(cargo deny --version | awk '{print $2}')"
if [[ "${DENY_VERSION}" != "0.20.2" ]]; then
    echo "error: cargo-deny 0.20.2 is required, found ${DENY_VERSION}" >&2
    exit 127
fi

mkdir -p "${REPORT_DIR}"
cd "${ROOT}"

run_audit() {
    local name="$1"
    local manifest="$2"
    local log="${REPORT_DIR}/${name}.log"
    local status

    printf '==> supply-chain audit: %s\n' "${name}"
    set +e
    CARGO_TERM_COLOR=never cargo deny \
        --manifest-path "${manifest}" \
        --config "${DENY_CONFIG}" \
        --locked \
        --all-features \
        --format human \
        check advisories bans licenses sources \
        -A unmatched-skip \
        -A unnecessary-skip \
        -A license-not-encountered 2>&1 | tee "${log}"
    status=${PIPESTATUS[0]}
    set -e

    if ((status != 0)); then
        printf 'supply-chain audit failed for %s; report: %s\n' "${name}" "${log}" >&2
        return "${status}"
    fi
}

run_audit workspace "${ROOT}/Cargo.toml"
run_audit fuzz "${ROOT}/fuzz/Cargo.toml"

printf 'supply-chain audit passed; reports: %s\n' "${REPORT_DIR}"
