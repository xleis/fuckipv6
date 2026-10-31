#!/bin/bash
#
# fuckipv6 macOS system-service installer.
# Usage: sudo ./install-macos-node.sh
#

set -euo pipefail

if [ "$(id -u)" -ne 0 ]; then
    echo "error: run this script with sudo" >&2
    exit 1
fi

if [ "$(uname -s)" != "Darwin" ]; then
    echo "error: this installer is for macOS" >&2
    exit 1
fi

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
BINARY=""

for candidate in \
    "${SCRIPT_DIR}/bin/fuckipv6" \
    "${SCRIPT_DIR}/../bin/fuckipv6" \
    "${SCRIPT_DIR}/fuckipv6" \
    "${SCRIPT_DIR}/../target/release/fuckipv6" \
    "${SCRIPT_DIR}/target/release/fuckipv6"
do
    if [ -x "${candidate}" ]; then
        BINARY="$(cd "$(dirname "${candidate}")" && pwd)/$(basename "${candidate}")"
        break
    fi
done

if [ -z "${BINARY}" ]; then
    echo "error: fuckipv6 binary not found; run from a release package or build with cargo build --release" >&2
    exit 1
fi

WEBUI_PORT="${FUCKIPV6_WEBUI_PORT:-8080}"
"${BINARY}" service install --scope system --webui-port "${WEBUI_PORT}"
ln -sfn /usr/local/fuckipv6/fuckipv6 /usr/local/bin/fuckipv6

echo
echo "fuckipv6 launchd service installed."
echo "Status:  sudo fuckipv6 service status"
echo "Restart: sudo fuckipv6 service restart"
