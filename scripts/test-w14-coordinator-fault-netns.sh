#!/usr/bin/env bash
#
# Privileged W14 acceptance test for real Coordinator candidate failures.
# Three isolated candidate network namespaces are connected to the host with
# veth pairs. The integration test starts real probe/store processes via
# `ip netns exec`, applies an nftables egress drop in a candidate namespace,
# verifies that the path fails closed, then restores it and validates recovery.

set -euo pipefail

PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SKIP_STATUS=77
NETNS_A="fip6-w14-a"
NETNS_B="fip6-w14-b"
NETNS_C="fip6-w14-c"
HOST_A="fip6-w14-ha"
HOST_B="fip6-w14-hb"
HOST_C="fip6-w14-hc"
PEER_A="fip6-w14-a0"
PEER_B="fip6-w14-b0"
PEER_C="fip6-w14-c0"
IP_A="10.240.1.10"
IP_B="10.240.2.10"
IP_C="10.240.3.10"
HOST_IP_A="10.240.1.1"
HOST_IP_B="10.240.2.1"
HOST_IP_C="10.240.3.1"
REPORT_ENV="FUCKIPV6_W14_REAL_FAULT_REPORT"
WORK_ENV="FUCKIPV6_W14_FAULT_NETNS_WORK"

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
    for netns in "$NETNS_A" "$NETNS_B" "$NETNS_C"; do
        ip netns delete "$netns" 2>/dev/null || true
    done
    for link in "$HOST_A" "$HOST_B" "$HOST_C"; do
        ip link delete "$link" 2>/dev/null || true
    done
}

cleanup() {
    set +e
    cleanup_topology
    if [ -n "${FUCKIPV6_W14_FAULT_NETNS_WORK:-}" ]; then
        rm -rf "$FUCKIPV6_W14_FAULT_NETNS_WORK"
    fi
}

require_root() {
    if [ "$(id -u)" -ne 0 ]; then
        skip "root is required to create network namespaces and apply nftables rules"
    fi
}

require_tools() {
    for tool in cargo ip nft ping timeout; do
        if ! command -v "$tool" >/dev/null 2>&1; then
            fail "missing required tool: $tool"
        fi
    done
}

create_pair() {
    local netns="$1"
    local host_link="$2"
    local peer_link="$3"
    local host_ip="$4"
    local candidate_ip="$5"

    ip netns add "$netns"
    ip link add "$host_link" type veth peer name "$peer_link"
    ip link set "$peer_link" netns "$netns"
    ip addr add "$host_ip/24" dev "$host_link"
    ip link set "$host_link" up

    ip netns exec "$netns" ip link set lo up
    ip netns exec "$netns" ip addr add "$candidate_ip/24" dev "$peer_link"
    ip netns exec "$netns" ip link set "$peer_link" up
}

create_topology() {
    cleanup_topology
    create_pair "$NETNS_A" "$HOST_A" "$PEER_A" "$HOST_IP_A" "$IP_A"
    create_pair "$NETNS_B" "$HOST_B" "$PEER_B" "$HOST_IP_B" "$IP_B"
    create_pair "$NETNS_C" "$HOST_C" "$PEER_C" "$HOST_IP_C" "$IP_C"

    for pair in "$NETNS_A:$HOST_IP_A" "$NETNS_B:$HOST_IP_B" "$NETNS_C:$HOST_IP_C"; do
        local netns="${pair%%:*}"
        local host_ip="${pair##*:}"
        if ! timeout 3 ip netns exec "$netns" ping -c 1 -W 1 "$host_ip" >/dev/null; then
            fail "candidate namespace $netns cannot reach host address $host_ip"
        fi
    done
}

run_test() {
    create_topology
    info "candidate namespaces: $NETNS_A ($IP_A), $NETNS_B ($IP_B), $NETNS_C ($IP_C)"
    FUCKIPV6_W14_NETNS_MODE=1 \
    FUCKIPV6_W14_NETNS_A="$NETNS_A" \
    FUCKIPV6_W14_NETNS_B="$NETNS_B" \
    FUCKIPV6_W14_NETNS_C="$NETNS_C" \
    FUCKIPV6_W14_IP_A="$IP_A" \
    FUCKIPV6_W14_IP_B="$IP_B" \
    FUCKIPV6_W14_IP_C="$IP_C" \
    FUCKIPV6_W14_REAL_FAULT_REPORT="${!REPORT_ENV}" \
        cargo test \
        --manifest-path "$PROJECT_ROOT/Cargo.toml" \
        -p fuckipv6-coordinator \
        --test w14_coordinator_fault_process \
        w14_netns_netfilter_fault_injection_records_real_process_evidence \
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

    if [ -z "${!WORK_ENV:-}" ]; then
        export "${WORK_ENV}=$(mktemp -d /tmp/fuckipv6-w14-fault-netns.XXXXXX)"
        export "${REPORT_ENV}=${!WORK_ENV}/report.json"
    fi
    export CARGO_TARGET_DIR="${FUCKIPV6_W14_CARGO_TARGET_DIR:-${!WORK_ENV}/target}"
    trap cleanup EXIT INT TERM

    run_test
    [ -s "${!REPORT_ENV}" ] || fail "W14 fault report was not written: ${!REPORT_ENV}"
    info "W14 Coordinator process/netns/netfilter fault checks passed"
    info "report: ${!REPORT_ENV}"
    cat "${!REPORT_ENV}"
}

main "$@"
