#!/usr/bin/env bash
#
# Privileged Linux acceptance test for W11-09 recovery failure injection.
#
# The test runs in private network and mount namespaces. It creates real TUN,
# policy-routing and DNS-bridge resources, then injects resource loss, network
# changes, a disconnected exit path and a real UDP DNS timeout. Recovery
# decisions and cleanup remain isolated from the host.

set -euo pipefail

PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SKIP_STATUS=77
INNER_ENV="FUCKIPV6_RECOVERY_NETNS_INNER"
WORK_ENV="FUCKIPV6_RECOVERY_NETNS_WORK"
RESOLV_CONF_ENV="FUCKIPV6_RECOVERY_RESOLV_CONF"
TUN_NAME="fip6recov"
UPLINK_NAME="fip6recup"

info() {
    printf '[INFO] %s\n' "$*"
}

fail() {
    printf '[FAIL] %s\n' "$*" >&2
    exit 1
}

skip() {
    printf '[SKIP] %s\n' "$*" >&2
    exit "$SKIP_STATUS"
}

require_root() {
    if [ "$(id -u)" -ne 0 ]; then
        skip "root is required for TUN, network namespaces and policy routing"
    fi
}

require_tools() {
    for tool in cargo ip mount unshare; do
        if ! command -v "$tool" >/dev/null 2>&1; then
            fail "missing required tool: $tool"
        fi
    done
    if [ ! -e /dev/net/tun ]; then
        skip "/dev/net/tun is unavailable"
    fi
}

require_netns_privileges() {
    if ! unshare --net --mount --fork --propagation private true >/dev/null 2>&1; then
        skip "network/mount namespace creation is unavailable"
    fi
}

cleanup() {
    set +e
    ip link delete "$TUN_NAME" >/dev/null 2>&1 || true
    ip link delete "$UPLINK_NAME" >/dev/null 2>&1 || true
    if [ -n "${FUCKIPV6_RECOVERY_NETNS_WORK:-}" ]; then
        rm -rf "$FUCKIPV6_RECOVERY_NETNS_WORK"
    fi
}

prepare_isolated_topology() {
    ip link set lo up
    ip link add "$UPLINK_NAME" type dummy
    ip link set "$UPLINK_NAME" up
    mount -t tmpfs tmpfs /run

    local resolv_conf="${!RESOLV_CONF_ENV}"
    printf '# isolated recovery baseline\nsearch recovery.invalid\nnameserver 203.0.113.53\n' >"$resolv_conf"
    mount --bind "$resolv_conf" /etc/resolv.conf
}

prebuild() {
    cargo test \
        --manifest-path "$PROJECT_ROOT/Cargo.toml" \
        -p fuckipv6-client \
        --no-default-features \
        --test recovery_linux \
        --no-run
}

run_test() {
    prepare_isolated_topology
    FUCKIPV6_RECOVERY_NETNS_TOPOLOGY=1 \
        cargo test \
        --manifest-path "$PROJECT_ROOT/Cargo.toml" \
        -p fuckipv6-client \
        --no-default-features \
        --test recovery_linux \
        -- \
        --ignored \
        --test-threads=1 \
        --nocapture
}

main() {
    require_root
    require_tools
    require_netns_privileges

    if [ -z "${!WORK_ENV:-}" ]; then
        export "${WORK_ENV}=$(mktemp -d /tmp/fuckipv6-recovery-netns.XXXXXX)"
        export "${RESOLV_CONF_ENV}=${!WORK_ENV}/resolv.conf"
        touch "${!RESOLV_CONF_ENV}"
    fi
    trap cleanup EXIT INT TERM

    if [ "${!INNER_ENV:-}" != "1" ]; then
        info "prebuilding the privileged recovery test"
        prebuild
        export "${INNER_ENV}=1"
        info "entering isolated network and mount namespaces"
        exec unshare --net --mount --fork --propagation private -- "$0" "$@"
    fi

    run_test
    info "all Linux recovery failure-injection checks passed"
}

main "$@"
