#!/usr/bin/env bash
#
# End-to-end Linux network namespace test for fuckipv6.
#
# This starts one coordinator in the host namespace and three clients in
# separate network namespaces. It verifies route setup, bidirectional ping,
# near-MTU ping payloads, and 3-node reachability.

set -euo pipefail

PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BUILD_DIR="$PROJECT_ROOT/target/release"
TEST_DIR="$PROJECT_ROOT/test_workspace/netns"
COORDINATOR_BIND_ADDR="${COORDINATOR_BIND_ADDR:-0.0.0.0}"
WEBUI_BIND_ADDR="${WEBUI_BIND_ADDR:-127.0.0.1}"
SIGNALING_PORT="16666"
WEBUI_PORT="18080"
CLIENT_COUNT="${CLIENT_COUNT:-3}"
PING_PAYLOAD_SIZE="${PING_PAYLOAD_SIZE:-1200}"
COORDINATOR_LOG="${COORDINATOR_LOG:-info}"
CLIENT_LOG="${CLIENT_LOG:-info}"

CLIENT_PIDS=()
NAMESPACES=()
VETH_HOSTS=()
VETH_PEERS=()
PING6_CMD=()

info() {
    printf '[INFO] %s\n' "$*"
}

fail() {
    printf '[FAIL] %s\n' "$*" >&2
    dump_logs
    exit 1
}

dump_logs() {
    if [ ! -d "$TEST_DIR" ]; then
        return
    fi

    if [ -f "$TEST_DIR/coordinator/coordinator.log" ]; then
        printf '\n[INFO] coordinator log tail\n' >&2
        tail -80 "$TEST_DIR/coordinator/coordinator.log" >&2 || true
    fi

    for ns in "${NAMESPACES[@]:-}"; do
        printf '\n[INFO] namespace %s diagnostics\n' "$ns" >&2
        ip netns exec "$ns" ip -6 addr show dev fuckipv6 >&2 || true
        ip netns exec "$ns" ip -6 route show >&2 || true
        ip netns exec "$ns" ip -6 neigh show >&2 || true
        ip netns exec "$ns" "$BUILD_DIR/fuckipv6" status >&2 || true
        ip netns exec "$ns" "$BUILD_DIR/fuckipv6" peers >&2 || true
    done

    for log in "$TEST_DIR"/client*/client.log; do
        [ -f "$log" ] || continue
        printf '\n[INFO] %s tail\n' "$log" >&2
        tail -80 "$log" >&2 || true
    done
}

cleanup() {
    set +e

    for pid in "${CLIENT_PIDS[@]:-}"; do
        kill "$pid" 2>/dev/null || true
    done

    if [ -n "${COORDINATOR_PID:-}" ]; then
        kill "$COORDINATOR_PID" 2>/dev/null || true
    fi

    sleep 1

    for pid in "${CLIENT_PIDS[@]:-}"; do
        wait "$pid" 2>/dev/null || true
    done

    if [ -n "${COORDINATOR_PID:-}" ]; then
        wait "$COORDINATOR_PID" 2>/dev/null || true
    fi

    for ns in "${NAMESPACES[@]:-}"; do
        ip netns delete "$ns" 2>/dev/null || true
    done

    for veth in "${VETH_HOSTS[@]:-}"; do
        ip link delete "$veth" 2>/dev/null || true
    done

    rm -rf "$TEST_DIR"
}

trap cleanup EXIT INT TERM

require_root() {
    if [ "$(id -u)" -ne 0 ]; then
        fail "This test needs root for network namespaces and TUN devices. Run with sudo."
    fi
}

require_tools() {
    for tool in ip cargo; do
        command -v "$tool" >/dev/null 2>&1 || fail "Missing required tool: $tool"
    done

    if command -v ping6 >/dev/null 2>&1; then
        PING6_CMD=(ping6)
    elif command -v ping >/dev/null 2>&1; then
        PING6_CMD=(ping -6)
    else
        fail "Missing required tool: ping6 or ping"
    fi
}

ensure_binaries() {
    if [ ! -x "$BUILD_DIR/fuckipv6-coordinator" ] || [ ! -x "$BUILD_DIR/fuckipv6" ]; then
        info "Release binaries not found; building them first"
        (cd "$PROJECT_ROOT" && cargo build --release)
    fi
}

create_namespace() {
    local index="$1"
    local ns="fip6test${index}"
    local host_veth="fip6h${index}"
    local peer_veth="fip6n${index}"
    local peer_ip="10.66.${index}.2/24"
    local host_ip="10.66.${index}.1/24"

    NAMESPACES+=("$ns")
    VETH_HOSTS+=("$host_veth")
    VETH_PEERS+=("$peer_veth")

    ip netns add "$ns"
    ip link add "$host_veth" type veth peer name "$peer_veth"
    ip link set "$peer_veth" netns "$ns"
    ip addr add "$host_ip" dev "$host_veth"
    ip link set "$host_veth" up

    ip netns exec "$ns" ip link set lo up
    ip netns exec "$ns" ip addr add "$peer_ip" dev "$peer_veth"
    ip netns exec "$ns" ip link set "$peer_veth" up
    ip netns exec "$ns" ip route add default via "10.66.${index}.1"
}

write_client_config() {
    local index="$1"
    local dir="$TEST_DIR/client${index}"
    local coordinator_addr="10.66.${index}.1"
    mkdir -p "$dir"

    cat >"$dir/config.toml" <<EOF
coordinator_url = "ws://${coordinator_addr}:${SIGNALING_PORT}"
private_key_path = "${dir}/identity.pem"

[network]
virtual_network = "fd00::/64"

[tun]
name = "fuckipv6"
mtu = 1280

[p2p]
listen_port = $((17000 + index))
connect_timeout = 10
heartbeat_interval = 5

[p2p.fallback_threshold]
max_rtt_ms = 200
max_loss_rate = 0.05
timeout_secs = 10

[log]
level = "info"
EOF
}

start_coordinator() {
    mkdir -p "$TEST_DIR/coordinator"
    RUST_LOG="$COORDINATOR_LOG" "$BUILD_DIR/fuckipv6-coordinator" \
        --bind "${COORDINATOR_BIND_ADDR}:${SIGNALING_PORT}" \
        --webui-bind "${WEBUI_BIND_ADDR}:${WEBUI_PORT}" \
        --ip-pool "fd00::1000/120" \
        >"$TEST_DIR/coordinator/coordinator.log" 2>&1 &
    COORDINATOR_PID="$!"

    sleep 2
    kill -0 "$COORDINATOR_PID" 2>/dev/null || {
        cat "$TEST_DIR/coordinator/coordinator.log" >&2
        fail "Coordinator failed to start"
    }
}

start_client() {
    local index="$1"
    local ns="fip6test${index}"
    local dir="$TEST_DIR/client${index}"

    ip netns exec "$ns" env RUST_LOG="$CLIENT_LOG" "$BUILD_DIR/fuckipv6" \
        start --config "$dir/config.toml" \
        >"$dir/client.log" 2>&1 &
    CLIENT_PIDS+=("$!")
}

wait_for_client_ip() {
    local index="$1"
    local ns="fip6test${index}"
    local ip_addr=""

    for _ in $(seq 1 30); do
        ip_addr="$(ip netns exec "$ns" ip -6 -o addr show dev fuckipv6 scope global 2>/dev/null \
            | awk '{print $4}' \
            | cut -d/ -f1 \
            | head -n1)"

        if [ -n "$ip_addr" ]; then
            printf '%s' "$ip_addr"
            return 0
        fi

        sleep 1
    done

    tail -50 "$TEST_DIR/client${index}/client.log" >&2 || true
    return 1
}

assert_route() {
    local index="$1"
    local ns="fip6test${index}"

    ip netns exec "$ns" ip -6 route show dev fuckipv6 | grep -q 'fd00::/64' \
        || fail "Namespace $ns does not have fd00::/64 route on fuckipv6"
}

assert_ping() {
    local src_index="$1"
    local dst_ip="$2"
    local label="$3"

    info "Ping ${label}: namespace ${src_index} -> ${dst_ip}"
    ip netns exec "fip6test${src_index}" ip -6 route get "$dst_ip" >&2 || true
    local ping_output
    if ! ping_output="$(ip netns exec "fip6test${src_index}" "${PING6_CMD[@]}" -c 3 -W 2 "$dst_ip" 2>&1)"; then
        printf '%s\n' "$ping_output" >&2
        fail "Ping failed: ${label}"
    fi
}

assert_mtu_ping() {
    local src_index="$1"
    local dst_ip="$2"

    info "Near-MTU ping: namespace ${src_index} -> ${dst_ip}, payload ${PING_PAYLOAD_SIZE}"
    local ping_output
    if ! ping_output="$(ip netns exec "fip6test${src_index}" "${PING6_CMD[@]}" -c 2 -W 3 -s "$PING_PAYLOAD_SIZE" "$dst_ip" 2>&1)"; then
        printf '%s\n' "$ping_output" >&2
        fail "Near-MTU ping failed"
    fi
}

main() {
    require_root
    require_tools
    ensure_binaries
    cleanup
    mkdir -p "$TEST_DIR"

    info "Creating ${CLIENT_COUNT} network namespaces"
    for index in $(seq 1 "$CLIENT_COUNT"); do
        create_namespace "$index"
        write_client_config "$index"
    done

    info "Starting coordinator"
    start_coordinator

    info "Starting clients"
    for index in $(seq 1 "$CLIENT_COUNT"); do
        start_client "$index"
    done

    declare -a CLIENT_IPS
    for index in $(seq 1 "$CLIENT_COUNT"); do
        CLIENT_IPS[$index]="$(wait_for_client_ip "$index")" \
            || fail "Client ${index} did not receive a virtual IP"
        info "Client ${index} virtual IP: ${CLIENT_IPS[$index]}"
        assert_route "$index"
    done

    sleep 5

    assert_ping 1 "${CLIENT_IPS[2]}" "client1 to client2"
    assert_ping 2 "${CLIENT_IPS[1]}" "client2 to client1"
    assert_mtu_ping 1 "${CLIENT_IPS[2]}"

    if [ "$CLIENT_COUNT" -ge 3 ]; then
        assert_ping 1 "${CLIENT_IPS[3]}" "client1 to client3"
        assert_ping 2 "${CLIENT_IPS[3]}" "client2 to client3"
        assert_ping 3 "${CLIENT_IPS[1]}" "client3 to client1"
    fi

    info "All namespace network tests passed"
}

main "$@"
