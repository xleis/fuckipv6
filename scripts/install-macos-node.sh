#!/bin/bash
#
# fuckipv6 macOS node installer.
# Usage: sudo ./install-macos-node.sh
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
    "${SCRIPT_DIR}/bin/fuckipv6" \
    "${SCRIPT_DIR}/fuckipv6" \
    "${SCRIPT_DIR}/../target/release/fuckipv6" \
    "${SCRIPT_DIR}/target/release/fuckipv6"
do
    if [ -x "${candidate}" ]; then
        BINARY="${candidate}"
        break
    fi
done

if [ -z "${BINARY}" ]; then
    echo "error: fuckipv6 binary not found; run from a release package or build with cargo build --release"
    exit 1
fi

install -d /usr/local/fuckipv6 /usr/local/etc/fuckipv6 /usr/local/var/log/fuckipv6
install -m 0755 "${BINARY}" /usr/local/fuckipv6/fuckipv6
ln -sf /usr/local/fuckipv6/fuckipv6 /usr/local/bin/fuckipv6

install -m 0644 "${SCRIPT_DIR}/fuckipv6.plist" /Library/LaunchDaemons/cc.xlei.fuckipv6.plist
launchctl bootout system /Library/LaunchDaemons/cc.xlei.fuckipv6.plist >/dev/null 2>&1 || true
launchctl bootstrap system /Library/LaunchDaemons/cc.xlei.fuckipv6.plist

echo "macOS node installed."
echo "Start: sudo launchctl kickstart -k system/cc.xlei.fuckipv6"
echo "Stop:  sudo launchctl bootout system /Library/LaunchDaemons/cc.xlei.fuckipv6.plist"
