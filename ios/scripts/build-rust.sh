#!/usr/bin/env bash
# Builds the Rust bridge for the platform and configuration Xcode is building.
#
# Requirements: a rustup toolchain with the aarch64-apple-ios and
# aarch64-apple-ios-sim targets. The script never touches the default rustup
# toolchain of the machine, it only runs cargo against an explicit target.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
CARGO="${CARGO:-$HOME/.cargo/bin/cargo}"
PLATFORM="${PLATFORM_NAME:-iphoneos}"
CONFIGURATION="${CONFIGURATION:-Debug}"

case "$PLATFORM" in
  iphoneos) TARGET="aarch64-apple-ios" ;;
  iphonesimulator)
    TARGET="aarch64-apple-ios-sim"
    if [ "${ARCHS:-arm64}" != "arm64" ]; then
      echo "error: only arm64 simulators are supported (got ARCHS=${ARCHS})" >&2
      exit 1
    fi
    ;;
  *)
    echo "error: unsupported platform ${PLATFORM}" >&2
    exit 1
    ;;
esac

if [ "$CONFIGURATION" = "Release" ]; then
  PROFILE="release"
  PROFILE_FLAG="--release"
  LIB_DIR="$ROOT/target/$TARGET/release"
else
  PROFILE="debug"
  PROFILE_FLAG=""
  LIB_DIR="$ROOT/target/$TARGET/debug"
fi

echo "note: building libfuckipv6_ios.a for ${TARGET} (${PROFILE})"
"$CARGO" build -p fuckipv6-ios-bridge --target "$TARGET" $PROFILE_FLAG

OUT_DIR="$ROOT/ios/Rust/lib/$PLATFORM"
mkdir -p "$OUT_DIR" "$ROOT/ios/Rust/include"
cp "$LIB_DIR/libfuckipv6_ios.a" "$OUT_DIR/libfuckipv6_ios.a"
cp "$ROOT/crates/ios-bridge/include/fuckipv6_ios.h" "$ROOT/ios/Rust/include/fuckipv6_ios.h"
echo "note: wrote $OUT_DIR/libfuckipv6_ios.a"
