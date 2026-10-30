#!/usr/bin/env bash
#
# End-to-end Linux network namespace test for fuckipv6.
#
# This starts one host node in the host namespace and member nodes in separate
# network namespaces. It verifies route setup, bidirectional ping, near-MTU
# ping payloads, and multi-node reachability.

set -euo pipefail

PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BUILD_DIR="$PROJECT_ROOT/target/release"
TEST_DIR="$PROJECT_ROOT/test_workspace/netns"
SIGNALING_PORT="16666"
RELAY_PORT="16667"
CLIENT_COUNT="${CLIENT_COUNT:-3}"
PING_PAYLOAD_SIZE="${PING_PAYLOAD_SIZE:-1200}"
HOST_LOG="${HOST_LOG:-info}"
NODE_LOG="${NODE_LOG:-info}"

NODE_PIDS=()
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

    if [ -f "$TEST_DIR/host/host.log" ]; then
        printf '\n[INFO] host node log tail\n' >&2
        tail -80 "$TEST_DIR/host/host.log" >&2 || true
    fi

    for ns in "${NAMESPACES[@]:-}"; do
        printf '\n[INFO] namespace %s diagnostics\n' "$ns" >&2
        ip netns exec "$ns" ip -6 addr show dev fuckipv6 >&2 || true
        ip netns exec "$ns" ip -6 route show >&2 || true
        ip netns exec "$ns" ip -6 neigh show >&2 || true
        ip netns exec "$ns" "$BUILD_DIR/fuckipv6" status >&2 || true
        ip netns exec "$ns" "$BUILD_DIR/fuckipv6" peers >&2 || true
    done

    for log in "$TEST_DIR"/node*/node.log; do
        [ -f "$log" ] || continue
        printf '\n[INFO] %s tail\n' "$log" >&2
        tail -80 "$log" >&2 || true
    done
}

cleanup() {
    set +e

    for pid in "${NODE_PIDS[@]:-}"; do
        kill "$pid" 2>/dev/null || true
    done

    if [ -n "${HOST_PID:-}" ]; then
        kill "$HOST_PID" 2>/dev/null || true
    fi

    sleep 1

    for pid in "${NODE_PIDS[@]:-}"; do
        wait "$pid" 2>/dev/null || true
    done

    if [ -n "${HOST_PID:-}" ]; then
        wait "$HOST_PID" 2>/dev/null || true
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

ensure_binary() {
    if [ ! -x "$BUILD_DIR/fuckipv6" ]; then
        info "Release node binary not found; building it first"
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

write_node_config() {
    local index="$1"
    local dir="$TEST_DIR/node${index}"
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

start_host() {
    mkdir -p "$TEST_DIR/host"
    RUST_LOG="$HOST_LOG" "$BUILD_DIR/fuckipv6" host \
        --port "$SIGNALING_PORT" \
        --relay-port "$RELAY_PORT" \
        >"$TEST_DIR/host/host.log" 2>&1 &
    HOST_PID="$!"

    sleep 2
    kill -0 "$HOST_PID" 2>/dev/null || {
        cat "$TEST_DIR/host/host.log" >&2
        fail "Host node failed to start"
    }
}

start_node() {
    local index="$1"
    local ns="fip6test${index}"
    local dir="$TEST_DIR/node${index}"

    ip netns exec "$ns" env RUST_LOG="$NODE_LOG" "$BUILD_DIR/fuckipv6" \
        start --config "$dir/config.toml" \
        >"$dir/node.log" 2>&1 &
    NODE_PIDS+=("$!")
}

wait_for_node_ip() {
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

    tail -50 "$TEST_DIR/node${index}/node.log" >&2 || true
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
    ensure_binary
    cleanup
    mkdir -p "$TEST_DIR"

    info "Creating ${CLIENT_COUNT} network namespaces"
    for index in $(seq 1 "$CLIENT_COUNT"); do
        create_namespace "$index"
        write_node_config "$index"
    done

    info "Starting host node"
    start_host

    info "Starting member nodes"
    for index in $(seq 1 "$CLIENT_COUNT"); do
        start_node "$index"
    done

    declare -a NODE_IPS
    for index in $(seq 1 "$CLIENT_COUNT"); do
        NODE_IPS[$index]="$(wait_for_node_ip "$index")" \
            || fail "Node ${index} did not receive a virtual IP"
        info "Node ${index} virtual IP: ${NODE_IPS[$index]}"
        assert_route "$index"
    done

    sleep 5

    assert_ping 1 "${NODE_IPS[2]}" "node1 to node2"
    assert_ping 2 "${NODE_IPS[1]}" "node2 to node1"
    assert_mtu_ping 1 "${NODE_IPS[2]}"

    if [ "$CLIENT_COUNT" -ge 3 ]; then
        assert_ping 1 "${NODE_IPS[3]}" "node1 to node3"
        assert_ping 2 "${NODE_IPS[3]}" "node2 to node3"
        assert_ping 3 "${NODE_IPS[1]}" "node3 to node1"
    fi

    info "All namespace network tests passed"
}

main "$@"
