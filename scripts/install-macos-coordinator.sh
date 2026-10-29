#!/bin/bash
#
# fuckipv6 macOS coordinator installer.
# Usage: sudo ./install-macos-coordinator.sh
#

set -euo pipefail

if [ "$(id -u)" -ne 0 ]; then
    echo "error: run this script with sudo"
    exit 1
fi

if [ "$(uname -s)" != "Darwin" ]; then
    echo "error: this installer is for macOS"
    exit 1
fi

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
BINARY=""

for candidate in \
    "${SCRIPT_DIR}/bin/fuckipv6-coordinator" \
    "${SCRIPT_DIR}/fuckipv6-coordinator" \
    "${SCRIPT_DIR}/../target/release/fuckipv6-coordinator" \
    "${SCRIPT_DIR}/target/release/fuckipv6-coordinator"
do
    if [ -x "${candidate}" ]; then
        BINARY="${candidate}"
        break
    fi
done

if [ -z "${BINARY}" ]; then
    echo "error: fuckipv6-coordinator binary not found; run from a release package or build with cargo build --release"
    exit 1
fi

install -d /usr/local/fuckipv6 /usr/local/etc/fuckipv6 /usr/local/var/log/fuckipv6
install -m 0755 "${BINARY}" /usr/local/fuckipv6/fuckipv6-coordinator
ln -sf /usr/local/fuckipv6/fuckipv6-coordinator /usr/local/bin/fuckipv6-coordinator

install -m 0644 "${SCRIPT_DIR}/fuckipv6-coordinator.plist" /Library/LaunchDaemons/cc.xlei.fuckipv6.coordinator.plist
launchctl bootout system /Library/LaunchDaemons/cc.xlei.fuckipv6.coordinator.plist >/dev/null 2>&1 || true
launchctl bootstrap system /Library/LaunchDaemons/cc.xlei.fuckipv6.coordinator.plist

echo "macOS coordinator installed."
echo "Start/restart: sudo launchctl kickstart -k system/cc.xlei.fuckipv6.coordinator"
echo "Stop:          sudo launchctl bootout system /Library/LaunchDaemons/cc.xlei.fuckipv6.coordinator.plist"
