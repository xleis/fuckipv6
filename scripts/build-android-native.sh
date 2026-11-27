#!/usr/bin/env bash
set -euo pipefail

# Build libfuckipv6_android.so for Android arm64-v8a and stage it into jniLibs.
ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ANDROID_SDK="${ANDROID_HOME:-/opt/android-sdk}"
NDK_DIR="${ANDROID_NDK_HOME:-$(ls -1d "${ANDROID_SDK}"/ndk/* 2>/dev/null | sort -V | tail -n 1)}"

if [[ -z "${NDK_DIR}" || ! -d "${NDK_DIR}" ]]; then
  echo "ERROR: Android NDK not found under ${ANDROID_SDK}/ndk (set ANDROID_NDK_HOME)" >&2
  exit 1
fi

NDK_BIN="${NDK_DIR}/toolchains/llvm/prebuilt/linux-x86_64/bin"
CLANG="${NDK_BIN}/aarch64-linux-android28-clang"
AR="${NDK_BIN}/llvm-ar"
if [[ ! -x "${CLANG}" ]]; then
  echo "ERROR: Expected NDK clang at ${CLANG}" >&2
  exit 1
fi

PROFILE="${ANDROID_NATIVE_PROFILE:-release}"
CARGO_FLAGS=()
if [[ "${PROFILE}" == "release" ]]; then
  CARGO_FLAGS+=(--release)
fi

RUSTC_BIN="$(rustup which --toolchain stable rustc)"
CARGO_BIN="$(rustup which --toolchain stable cargo)"

echo "[build-android-native] target=aarch64-linux-android profile=${PROFILE} ndk=${NDK_DIR}"
cd "${ROOT_DIR}"
CC_aarch64_linux_android="${CLANG}" \
AR_aarch64_linux_android="${AR}" \
CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER="${CLANG}" \
RUSTC="${RUSTC_BIN}" \
"${CARGO_BIN}" build -p fuckipv6-android-bridge --lib --target aarch64-linux-android "${CARGO_FLAGS[@]}"

OUT_SO="${ROOT_DIR}/target/aarch64-linux-android/${PROFILE}/libfuckipv6_android.so"
DEST_DIR="${ROOT_DIR}/android/app/src/main/jniLibs/arm64-v8a"
mkdir -p "${DEST_DIR}"
cp -f "${OUT_SO}" "${DEST_DIR}/libfuckipv6_android.so"
if [[ "${PROFILE}" == "release" && -x "${NDK_BIN}/llvm-strip" ]]; then
  "${NDK_BIN}/llvm-strip" --strip-unneeded "${DEST_DIR}/libfuckipv6_android.so"
fi
echo "[build-android-native] staged $(du -h "${DEST_DIR}/libfuckipv6_android.so" | cut -f1) -> ${DEST_DIR}/libfuckipv6_android.so"
