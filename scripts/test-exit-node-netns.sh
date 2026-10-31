#!/usr/bin/env bash
#
# Privileged Linux Exit Node acceptance test.
#
# The script re-executes itself in a private network and mount namespace, then
# builds this topology:
#
#   client netns --- fip6tun/fip6cli --- host netns --- fip6out/fip6up --- upstream netns
#      10.0.0.2                          default route              198.51.100.10
#
# The host-side fip6tun veth stands in for a TUN interface at the kernel routing
# and firewall layer. The test therefore exercises real forwarding, conntrack,
# iptables NAT, return routes, revocation and rollback, but not userspace TUN
# packet I/O or a real internet uplink.

set -euo pipefail

PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SKIP_STATUS=77
CLIENT_NETNS="fip6-exit-client"
UPSTREAM_NETNS="fip6-exit-upstream"
INNER_ENV="FUCKIPV6_EXIT_NETNS_INNER"
WORK_ENV="FUCKIPV6_EXIT_NETNS_WORK"

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
    if [ -n "${FUCKIPV6_EXIT_NETNS_WORK:-}" ]; then
        rm -rf "$FUCKIPV6_EXIT_NETNS_WORK"
    fi
}

require_root() {
    if [ "$(id -u)" -ne 0 ]; then
        skip "root is required for network namespaces, TUN-layer routing and iptables"
    fi
}

require_tools() {
    for tool in cargo ip iptables ping unshare; do
        if ! command -v "$tool" >/dev/null 2>&1; then
            fail "missing required tool: $tool"
        fi
    done
}

require_netns_privileges() {
    if ! unshare --net --mount --fork --propagation private true >/dev/null 2>&1; then
        skip "network/mount namespace creation is unavailable (missing CAP_SYS_ADMIN or kernel support)"
    fi
}

create_topology() {
    cleanup_topology

    ip netns add "$CLIENT_NETNS"
    ip netns add "$UPSTREAM_NETNS"

    # Client-side veth stands in for the node TUN device at the kernel layer.
    ip link add fip6tun type veth peer name fip6cli
    ip link set fip6cli netns "$CLIENT_NETNS"
    ip addr add 10.0.0.1/24 dev fip6tun
    ip link set fip6tun up

    ip netns exec "$CLIENT_NETNS" ip link set lo up
    ip netns exec "$CLIENT_NETNS" ip addr add 10.0.0.2/24 dev fip6cli
    ip netns exec "$CLIENT_NETNS" ip link set fip6cli up
    ip netns exec "$CLIENT_NETNS" ip route add default via 10.0.0.1 dev fip6cli

    # The host namespace obtains a deterministic default route via fip6out.
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

run_ignored_test() {
    local test_name="$1"
    info "running $test_name"
    FUCKIPV6_EXIT_NETNS_CLIENT="$CLIENT_NETNS" \
        FUCKIPV6_EXIT_NETNS_TOPOLOGY=1 \
        cargo test \
        --manifest-path "$PROJECT_ROOT/Cargo.toml" \
        -p fuckipv6-client \
        --no-default-features \
        "$test_name" \
        -- \
        --ignored \
        --exact \
        --test-threads=1 \
        --nocapture
}

run_success_scenario() {
    create_topology
    run_ignored_test \
        routing::linux_exit::tests::linux_netns_forwarding_nat_revocation_and_cleanup_are_effective
    cleanup_topology
}

run_failure_rollback_scenario() {
    create_topology

    local real_iptables fail_bin marker
    real_iptables="$(command -v iptables)"
    fail_bin="$FUCKIPV6_EXIT_NETNS_WORK/fail-bin"
    marker="$FUCKIPV6_EXIT_NETNS_WORK/iptables-failure-hit"
    mkdir -p "$fail_bin"
    cat >"$fail_bin/iptables" <<EOF
#!/usr/bin/env bash
set -euo pipefail
if [[ " \$* " == *" -t nat -I POSTROUTING 1 -j FKV6N-"* ]]; then
    printf 'injected POSTROUTING failure\n' >&2
    : >"$marker"
    exit 42
fi
exec "$real_iptables" "\$@"
EOF
    chmod 0755 "$fail_bin/iptables"

    info "running install-failure rollback with a deterministic iptables wrapper"
    PATH="$fail_bin:$PATH" run_ignored_test \
        routing::linux_exit::tests::linux_netns_install_failure_rolls_back_kernel_state

    if [ ! -f "$marker" ]; then
        fail "iptables fault injection did not reach the POSTROUTING jump"
    fi

    if ip route show default | grep -q 'dev fip6out'; then
        :
    else
        fail "temporary topology disappeared before rollback verification"
    fi
    cleanup_topology
}

main() {
    require_root
    require_tools
    require_netns_privileges

    if [ -z "${!WORK_ENV:-}" ]; then
        export "${WORK_ENV}=$(mktemp -d /tmp/fuckipv6-exit-netns.XXXXXX)"
    fi
    export CARGO_TARGET_DIR="${FUCKIPV6_EXIT_NETNS_CARGO_TARGET_DIR:-${!WORK_ENV}/target}"
    trap cleanup EXIT INT TERM

    if [ "${!INNER_ENV:-}" != "1" ]; then
        info "prebuilding the privileged test binary before network isolation"
        cargo test \
            --manifest-path "$PROJECT_ROOT/Cargo.toml" \
            -p fuckipv6-client \
            --no-default-features \
            --no-run

        export "${INNER_ENV}=1"
        info "entering an isolated network and mount namespace"
        exec unshare --net --mount --fork --propagation private -- "$0" "$@"
    fi

    if ! iptables -w 5 -t nat -L POSTROUTING -n >/dev/null 2>&1; then
        skip "the kernel iptables NAT table is unavailable in this environment"
    fi
    if ! iptables -w 5 -t filter -L FORWARD -n >/dev/null 2>&1; then
        skip "the kernel iptables filter table is unavailable in this environment"
    fi

    run_success_scenario
    run_failure_rollback_scenario
    info "all Linux Exit Node privileged netns checks passed"
}

main "$@"
