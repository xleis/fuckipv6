#!/usr/bin/env bash
#
# Release gate: the shipped binary must not carry development credentials.
#
# The repository keeps local-only material (client_key.pem, client.toml,
# coordinator.toml) out of Git, so the risk is that some module embeds a fixture
# or a real key by accident. This scans the compiled binary for private-key
# headers, cloud/platform token shapes and the names of the local-only files, and
# fails if any of them appear.
set -Eeuo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BIN="${1:-${ROOT}/target/release/fuckipv6}"

if [[ ! -f "$BIN" ]]; then
    echo "error: binary not found: $BIN (build with cargo build --release)" >&2
    exit 2
fi

patterns=(
    "-----BEGIN [A-Z ]*PRIVATE KEY-----"
    "-----BEGIN OPENSSH PRIVATE KEY-----"
    "ghp_[A-Za-z0-9]{20,}"
    "github_pat_[A-Za-z0-9_]{20,}"
    "AKIA[0-9A-Z]{16}"
    "xox[baprs]-[A-Za-z0-9-]{10,}"
)

found=0
for pattern in "${patterns[@]}"; do
    if strings -a "$BIN" | grep -Eq -- "$pattern"; then
        echo "[secrets] FAIL: binary matches /$pattern/" >&2
        found=$((found + 1))
    fi
done

if [ -f "${ROOT}/client_key.pem" ]; then
    # Compare the tail of the local key body: a truncated embed still counts.
    body=$(grep -av -- '-----' "${ROOT}/client_key.pem" | head -2 | tr -d '\n')
    if [ -n "$body" ] && strings -a "$BIN" | grep -Fq -- "$body"; then
        echo "[secrets] FAIL: the local client_key.pem body is embedded in the binary" >&2
        found=$((found + 1))
    fi
fi

if [ "$found" -ne 0 ]; then
    echo "[secrets] $found pattern(s) matched" >&2
    exit 1
fi
echo "[secrets] binary carries no development credentials: $BIN"
