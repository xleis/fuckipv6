#!/usr/bin/env bash
#
# Privileged W13 acceptance test for simultaneous Relay, Coordinator and Exit
# failure. It runs in private network and mount namespaces, then builds:
#
#   client netns --- fip6tun/fip6cli --- host netns --- fip6out/fip6up --- upstream netns
#      10.0.0.2                          default route              198.51.100.10
#
# SignalingServer, RelayServer and LinuxExitForwarder are real instances. The
# test proves their individual paths before failure, stops all three together,
# verifies bounded fail-closed cleanup with no direct fallback, then restarts a
# fresh stack and verifies Relay forwarding and Exit NAT recover.

set -euo pipefail

PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SKIP_STATUS=77
CLIENT_NETNS="fip6-w13-client"
UPSTREAM_NETNS="fip6-w13-upstream"
INNER_ENV="FUCKIPV6_W13_FAILURE_NETNS_INNER"
WORK_ENV="FUCKIPV6_W13_FAILURE_NETNS_WORK"
REPORT_ENV="FUCKIPV6_W13_FAILURE_REPORT"

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

cleanup_topology() {
    ip netns delete "$CLIENT_NETNS" 2>/dev/null || true
    ip netns delete "$UPSTREAM_NETNS" 2>/dev/null || true
    ip link delete fip6tun 2>/dev/null || true
    ip link delete fip6out 2>/dev/null || true
}

cleanup() {
    set +e
    cleanup_topology
    if [ -n "${FUCKIPV6_W13_FAILURE_NETNS_WORK:-}" ]; then
        rm -rf "$FUCKIPV6_W13_FAILURE_NETNS_WORK"
    fi
}

require_root() {
    if [ "$(id -u)" -ne 0 ]; then
        skip "root is required for network namespaces, forwarding and iptables"
    fi
}

require_tools() {
    for tool in cargo ip iptables ip6tables ping unshare; do
        if ! command -v "$tool" >/dev/null 2>&1; then
            fail "missing required tool: $tool"
        fi
    done
}

require_netns_privileges() {
    if ! unshare --net --mount --fork --propagation private true >/dev/null 2>&1; then
        skip "network/mount namespace creation is unavailable"
    fi
}

create_topology() {
    cleanup_topology

    ip netns add "$CLIENT_NETNS"
    ip netns add "$UPSTREAM_NETNS"

    ip link set lo up
    ip link add fip6tun type veth peer name fip6cli
    ip link set fip6cli netns "$CLIENT_NETNS"
    ip addr add 10.0.0.1/24 dev fip6tun
    ip link set fip6tun up

    ip netns exec "$CLIENT_NETNS" ip link set lo up
    ip netns exec "$CLIENT_NETNS" ip addr add 10.0.0.2/24 dev fip6cli
    ip netns exec "$CLIENT_NETNS" ip link set fip6cli up
    ip netns exec "$CLIENT_NETNS" ip route add default via 10.0.0.1 dev fip6cli

    ip link add fip6out type veth peer name fip6up
    ip link set fip6up netns "$UPSTREAM_NETNS"
    ip addr add 192.0.2.1/30 dev fip6out
    ip link set fip6out up
    ip route add default via 192.0.2.2 dev fip6out

    ip netns exec "$UPSTREAM_NETNS" ip link set lo up
    ip netns exec "$UPSTREAM_NETNS" ip addr add 192.0.2.2/30 dev fip6up
    ip netns exec "$UPSTREAM_NETNS" ip link set fip6up up
    ip netns exec "$UPSTREAM_NETNS" ip addr add 198.51.100.10/32 dev lo

    sysctl -q -w net.ipv4.ip_forward=0

    if [ "$(cat /proc/sys/net/ipv4/ip_forward)" != "0" ]; then
        fail "could not establish a clean IPv4 forwarding baseline"
    fi
    if ! ip route show default | grep -q 'dev fip6out'; then
        fail "the namespace default route does not use fip6out"
    fi
}

run_test() {
    create_topology
    FUCKIPV6_W13_FAILURE_NETNS_CLIENT="$CLIENT_NETNS" \
    FUCKIPV6_W13_FAILURE_NETNS_TOPOLOGY=1 \
    FUCKIPV6_W13_FAILURE_REPORT="${!REPORT_ENV}" \
        cargo test \
        --manifest-path "$PROJECT_ROOT/Cargo.toml" \
        -p fuckipv6-client \
        --no-default-features \
        --test w13_relay_failure_netns \
        -- \
        --ignored \
        --exact \
        --test-threads=1 \
        --nocapture
    cleanup_topology
}

main() {
    require_root
    require_tools
    require_netns_privileges

    if [ -z "${!WORK_ENV:-}" ]; then
        export "${WORK_ENV}=$(mktemp -d /tmp/fuckipv6-w13-failure-netns.XXXXXX)"
        export "${REPORT_ENV}=${!WORK_ENV}/report.json"
    fi
    export CARGO_TARGET_DIR="${FUCKIPV6_W13_FAILURE_CARGO_TARGET_DIR:-${!WORK_ENV}/target}"
    trap cleanup EXIT INT TERM

    if [ "${!INNER_ENV:-}" != "1" ]; then
        info "prebuilding the privileged W13 failure test"
        cargo test \
            --manifest-path "$PROJECT_ROOT/Cargo.toml" \
            -p fuckipv6-client \
            --no-default-features \
            --test w13_relay_failure_netns \
            --no-run
        export "${INNER_ENV}=1"
        info "entering an isolated network and mount namespace"
        exec unshare --net --mount --fork --propagation private -- "$0" "$@"
    fi

    if ! iptables -w 5 -t nat -L POSTROUTING -n >/dev/null 2>&1; then
        skip "the kernel iptables NAT table is unavailable"
    fi
    if ! iptables -w 5 -t filter -L FORWARD -n >/dev/null 2>&1; then
        skip "the kernel iptables filter table is unavailable"
    fi

    run_test
    info "W13 simultaneous Relay/Coordinator/Exit failure checks passed"
    info "report: ${!REPORT_ENV}"
    cat "${!REPORT_ENV}"
}

main "$@"
