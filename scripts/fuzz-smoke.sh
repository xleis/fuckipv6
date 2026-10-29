#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

RUNS="${FUZZ_RUNS:-2000}"
MAX_LEN="${FUZZ_MAX_LEN:-4096}"
TARGETS=(wire_decoders handshake_inputs data_packet)

if ! command -v cargo-fuzz >/dev/null 2>&1 && [[ -x "$HOME/.cargo/bin/cargo-fuzz" ]]; then
    export PATH="$HOME/.cargo/bin:$PATH"
fi

if ! command -v cargo-fuzz >/dev/null 2>&1; then
    echo "cargo-fuzz is required: cargo install cargo-fuzz --locked" >&2
    exit 127
fi
if ! command -v rustup >/dev/null 2>&1; then
    echo "rustup with the nightly toolchain is required" >&2
    exit 127
fi

# Use one coherent nightly toolchain even when the host configures a rustc
# wrapper or puts a non-rustup cargo proxy earlier on PATH.
NIGHTLY_RUSTC="$(rustup which --toolchain nightly rustc)"
NIGHTLY_BIN="$(dirname "$NIGHTLY_RUSTC")"
export PATH="$NIGHTLY_BIN:$HOME/.cargo/bin:$PATH"
export RUSTUP_TOOLCHAIN=nightly
export RUSTC="$NIGHTLY_RUSTC"
export RUSTDOC="$NIGHTLY_BIN/rustdoc"
export RUSTC_WRAPPER=""

for target in "${TARGETS[@]}"; do
    echo "[fuzz] ${target}: runs=${RUNS} max_len=${MAX_LEN}"
    cargo fuzz run "$target" -- \
        -runs="$RUNS" \
        -max_len="$MAX_LEN" \
        -print_final_stats=1
done
