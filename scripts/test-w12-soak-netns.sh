#!/usr/bin/env bash
#
# W12-09 long-running soak: two independent network namespaces join one
# InviteOnly network through the production invitation flow, then keep the direct
# P2P data path under continuous observation for 24 hours: per-cycle snapshots,
# bidirectional ICMP, periodic TCP canaries, peer-health transitions, process RSS
# and file-descriptor growth, and temporary-file growth.
#
# The Coordinator and both peers each run in their own netns with a dedicated
# routed veth underlay. No Exit Node, remote HK egress, router OpenClash policy,
# host default route or firewall rule is used by this test. The only host
# networking changes are three private network namespaces and two 10.78.x/24
# veth pairs; all TUN, route and policy changes stay inside those namespaces.
#
# Run as root:
#   scripts/test-w12-p2p-invite-netns.sh

set -Eeuo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BIN="$ROOT/target/release/fuckipv6"
NETWORK="w12-soak"
NSH="w12soakh"
NS1="w12soak1"
NS2="w12soak2"
VH1="w12so1h"
VN1="w12so1n"
VH2="w12so2h"
VN2="w12so2n"
HOST_PORT=19866
RELAY_PORT=19867
WEBUI_HOST=18970
WEBUI1=18971
WEBUI2=18972
TCP_ECHO_PORT=19291
UDP_ECHO_PORT=19292
WORK="${W12_SOAK_WORK:-/tmp/w12-soak.$(date -u +%Y%m%dT%H%M%SZ).$$}"
SOAK_HOURS="${W12_SOAK_HOURS:-24}"
SOAK_INTERVAL="${W12_SOAK_INTERVAL:-60}"
CANARY_INTERVAL="${W12_SOAK_CANARY_INTERVAL:-300}"

HPID=""
N1PID=""
N2PID=""
TCPDUMP1_PID=""
TCPDUMP2_PID=""
TCPDUMP_TUN1_PID=""
TCPDUMP_TUN2_PID=""
TCP_ECHO1_PID=""
TCP_ECHO2_PID=""
UDP_ECHO1_PID=""
UDP_ECHO2_PID=""

info() { printf '[INFO] %s\n' "$*"; }
fail() { printf '[FAIL] %s\n' "$*" >&2; exit 1; }

require_tools() {
    local tool
    for tool in cargo git ip jq mount perl ping python3 ss tcpdump unshare; do
        command -v "$tool" >/dev/null 2>&1 || fail "missing required tool: $tool"
    done
}

kill_pid_checked() {
    local pid=$1 sig=$2
    [ -n "$pid" ] || return 0
    case "$pid" in
        *[!0-9]*) return 0 ;;
    esac
    kill -"$sig" "$pid" 2>/dev/null || true
}

kill_runtime_locks() {
    local dir=$1 lock pid
    for lock in "$dir/control.lock" "$dir/node.lock"; do
        [ -f "$lock" ] || continue
        pid=$(tr -dc '0-9' <"$lock" 2>/dev/null || true)
        [ -n "$pid" ] || continue
        kill_pid_checked "$pid" TERM
    done
}

cleanup() {
    set +e
    info "cleanup: stopping only PIDs created by this run"
    kill_pid_checked "$TCPDUMP1_PID" TERM
    kill_pid_checked "$TCPDUMP2_PID" TERM
    kill_pid_checked "$TCPDUMP_TUN1_PID" TERM
    kill_pid_checked "$TCPDUMP_TUN2_PID" TERM
    kill_pid_checked "$TCP_ECHO1_PID" TERM
    kill_pid_checked "$TCP_ECHO2_PID" TERM
    kill_pid_checked "$UDP_ECHO1_PID" TERM
    kill_pid_checked "$UDP_ECHO2_PID" TERM
    kill_pid_checked "$N1PID" TERM
    kill_pid_checked "$N2PID" TERM
    kill_pid_checked "$HPID" TERM
    for dir in "$WORK/host" "$WORK/node1" "$WORK/node2" "$WORK/rejected"; do
        kill_runtime_locks "$dir/run"
    done
    sleep 2
    kill_pid_checked "$TCPDUMP1_PID" KILL
    kill_pid_checked "$TCPDUMP2_PID" KILL
    kill_pid_checked "$TCPDUMP_TUN1_PID" KILL
    kill_pid_checked "$TCPDUMP_TUN2_PID" KILL
    kill_pid_checked "$TCP_ECHO1_PID" KILL
    kill_pid_checked "$TCP_ECHO2_PID" KILL
    kill_pid_checked "$UDP_ECHO1_PID" KILL
    kill_pid_checked "$UDP_ECHO2_PID" KILL
    kill_pid_checked "$N1PID" KILL
    kill_pid_checked "$N2PID" KILL
    kill_pid_checked "$HPID" KILL
    for dir in "$WORK/host" "$WORK/node1" "$WORK/node2" "$WORK/rejected"; do
        kill_runtime_locks "$dir/run"
    done
    ip netns delete "$NS1" >/dev/null 2>&1 || true
    ip netns delete "$NS2" >/dev/null 2>&1 || true
    ip netns delete "$NSH" >/dev/null 2>&1 || true
    info "evidence retained at $WORK"
}

# The node rejects local IPC peers from a different network namespace, so every
# CLI-side probe has to run inside the namespace that owns the socket.
declare -A SOCKET_NS=()

socket_namespace() {
    printf '%s' "${SOCKET_NS[$1]:-}"
}

ipc_command() {
    local socket_path=$1 command_json=$2 ns
    ns=$(socket_namespace "$socket_path")
    if [ -n "$ns" ]; then
        ip netns exec "$ns" python3 "$WORK/ipc_client.py" "$socket_path" "$command_json"
    else
        python3 "$WORK/ipc_client.py" "$socket_path" "$command_json"
    fi
}

write_ipc_client() {
    cat >"$WORK/ipc_client.py" <<'PYCLIENT'
import json
import socket
import struct
import sys

socket_path, command_json = sys.argv[1:3]
command = json.loads(command_json)
request_id = 1
# Kept in sync with crates/client/src/local_ipc.rs PROTOCOL_VERSION.
PROTOCOL_VERSION = 5
FRAME_HEADER = struct.Struct(">4sHBBI")


def recv_exact(stream, size):
    chunks = []
    remaining = size
    while remaining:
        chunk = stream.recv(remaining)
        if not chunk:
            raise RuntimeError("local IPC closed before a complete frame arrived")
        chunks.append(chunk)
        remaining -= len(chunk)
    return b"".join(chunks)


def send_frame(stream, kind, value):
    payload = json.dumps(value, separators=(",", ":"), ensure_ascii=False).encode("utf-8")
    stream.sendall(FRAME_HEADER.pack(b"FKIP", PROTOCOL_VERSION, kind, 0, len(payload)))
    stream.sendall(payload)


def recv_frame(stream):
    magic, version, kind, reserved, length = FRAME_HEADER.unpack(recv_exact(stream, 12))
    if magic != b"FKIP" or version != PROTOCOL_VERSION or reserved != 0:
        raise RuntimeError("invalid local IPC frame header")
    if length > 1024 * 1024:
        raise RuntimeError("oversized local IPC frame")
    return kind, json.loads(recv_exact(stream, length))


with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as stream:
    stream.settimeout(30)
    stream.connect(socket_path)
    send_frame(stream, 1, {"protocol_version": PROTOCOL_VERSION, "role": "cli"})
    kind, hello = recv_frame(stream)
    if kind != 2 or hello.get("protocol_version") != PROTOCOL_VERSION:
        raise RuntimeError(f"local IPC handshake failed: {hello}")
    send_frame(
        stream,
        3,
        {
            "request_id": request_id,
            "command": {
                "type": "execute",
                "data": {
                    "request_id": request_id,
                    "command": command,
                },
            },
        },
    )
    while True:
        kind, value = recv_frame(stream)
        if kind == 5:
            continue
        if kind == 6:
            raise RuntimeError(f"local IPC error: {value}")
        if kind != 4:
            raise RuntimeError(f"unexpected local IPC frame kind: {kind}")
        break

if value.get("type") != "node_command":
    raise RuntimeError(f"unexpected local IPC response: {value}")
result = value.get("data")
if not isinstance(result, dict):
    raise RuntimeError(f"unexpected local IPC response: {value}")
if result.get("request_id") != request_id:
    raise RuntimeError(f"local IPC request id mismatch: {result}")
outcome = result.get("outcome", {})
if outcome.get("status") == "rejected":
    print(json.dumps(outcome, indent=2, ensure_ascii=False))
    raise SystemExit(3)
print(json.dumps(outcome, indent=2, ensure_ascii=False))
PYCLIENT
}

snapshot_to() {
    local socket_path=$1 output=$2
    ipc_command "$socket_path" '{"command":"get_snapshot"}' >"$output"
}

wait_for_file() {
    local path=$1 attempts=${2:-60}
    local i
    for ((i = 0; i < attempts; i++)); do
        [ -s "$path" ] && return 0
        sleep 1
    done
    return 1
}

wait_for_socket() {
    local path=$1 attempts=${2:-60}
    local i
    for ((i = 0; i < attempts; i++)); do
        [ -S "$path" ] && return 0
        sleep 1
    done
    return 1
}

wait_for_network_online() {
    local socket_path=$1 output=$2 attempts=${3:-90}
    local i status ip
    for ((i = 0; i < attempts; i++)); do
        if snapshot_to "$socket_path" "$output"; then
            status=$(jq -r --arg n "$NETWORK" '.data.networks[]? | select(.name==$n) | .status' "$output" 2>/dev/null | head -1)
            ip=$(jq -r --arg n "$NETWORK" '.data.networks[]? | select(.name==$n) | .assigned_ipv6 // empty' "$output" 2>/dev/null | head -1)
            if [ "$status" = online ] && [ -n "$ip" ]; then
                return 0
            fi
        fi
        sleep 1
    done
    return 1
}

wait_for_peer() {
    local socket_path=$1 output=$2 peer_ip=$3 attempts=${4:-60}
    local i active healthy online
    for ((i = 0; i < attempts; i++)); do
        if snapshot_to "$socket_path" "$output"; then
            readarray -t peer_state < <(jq -r --arg ip "$peer_ip" \
                '.data.peers[]? | select(.virtual_ipv6==$ip) | [.online,.p2p_active,.p2p_healthy] | @tsv' \
                "$output" 2>/dev/null | head -1 | tr '\t' '\n')
            online=${peer_state[0]:-false}
            active=${peer_state[1]:-false}
            healthy=${peer_state[2]:-false}
            if [ "$online" = true ] && [ "$active" = true ] && [ "$healthy" = true ]; then
                return 0
            fi
        fi
        sleep 1
    done
    return 1
}

assert_target_peer() {
    local snapshot=$1 peer_ip=$2 expected_endpoint_prefix=$3 label=$4
    jq -e --arg ip "$peer_ip" --arg prefix "$expected_endpoint_prefix" '
        [.data.peers[]? | select(.virtual_ipv6==$ip)
          | select(.online==true and .p2p_active==true and .p2p_healthy==true)
          | select(any(.endpoints[]?; startswith($prefix)))] | length == 1
    ' "$snapshot" >/dev/null || fail "$label does not show a healthy direct P2P peer at $expected_endpoint_prefix"
}

assert_peer_traffic() {
    local snapshot=$1 peer_ip=$2 label=$3
    jq -e --arg ip "$peer_ip" '
        [.data.peers[]? | select(.virtual_ipv6==$ip)
          | select(.traffic.p2p_rx_packets > 0 and .traffic.p2p_tx_packets > 0)] | length == 1
    ' "$snapshot" >/dev/null || fail "$label does not show bidirectional P2P packet traffic"
}

run_tcp_echo_test() {
    local ns=$1 peer_ip=$2 label=$3 payload_bytes=${4:-262144}
    timeout 30s ip netns exec "$ns" python3 "$WORK/tcp_echo_client.py" "$peer_ip" "$TCP_ECHO_PORT" "$payload_bytes" \
        >"$WORK/$label-tcp.json"
    jq -e '.ok == true' "$WORK/$label-tcp.json" >/dev/null || fail "$label TCP echo verification failed"
}

run_udp_echo_test() {
    local ns=$1 peer_ip=$2 label=$3 datagrams=${4:-64}
    timeout 30s ip netns exec "$ns" python3 "$WORK/udp_echo_client.py" "$peer_ip" "$UDP_ECHO_PORT" "$datagrams" \
        >"$WORK/$label-udp.json"
    jq -e '.ok == true' "$WORK/$label-udp.json" >/dev/null || fail "$label UDP echo verification failed"
}

wait_for_echo_port() {
    local ns=$1 address=$2 port=$3 label=$4 attempts=${5:-30}
    local i
    for ((i = 0; i < attempts; i++)); do
        if ip netns exec "$ns" python3 - "$address" "$port" <<'PY' >/dev/null 2>&1
import socket
import sys

address, port = sys.argv[1], int(sys.argv[2])
with socket.socket(socket.AF_INET6, socket.SOCK_STREAM) as stream:
    stream.settimeout(1)
    stream.connect((address, port))
PY
        then
            return 0
        fi
        sleep 1
    done
    fail "$label echo service did not start on $address:$port"
}

# The first candidate configuration is the credential-free bootstrap write the
# Coordinator accepts before any term exists. Publishing the owner declaration
# then makes that candidate healthy, which is the Coordinator's own gate for
# membership governance. Without this step a brand-new network stays frozen.
bootstrap_coordinator_governance() {
    local socket_path=$1 network=$2 owner_node=$3
    local ENDPOINT="10.78.1.1:$HOST_PORT"
    local now_ms expires_ms command
    now_ms=$(date +%s%3N)
    expires_ms=$((now_ms + 3600000))
    command=$(jq -cn --arg n "$network" --arg id "$owner_node" --argjson exp "$expires_ms" \
        '{command:"update_coordinator_candidates",payload:{network_name:$n,expected_revision:0,candidates:[{node_id:$id,priority:10}],expires_at_unix_ms:$exp}}')
    if ! ipc_command "$socket_path" "$command" >"$WORK/bootstrap-candidates.json" 2>"$WORK/bootstrap-candidates.err"; then
        sed -e 's/\x1b\[[0-9;]*m//g' "$WORK/bootstrap-candidates.err" >&2 || true
        fail "bootstrap candidate configuration was rejected"
    fi
    # The declaration must claim a public path, otherwise the candidate is
    # projected as degraded ("coordinator.candidate.no_public_path") and the
    # Coordinator keeps governance frozen.
    command=$(jq -cn --arg n "$network" --arg endpoint "$ENDPOINT" \
        '{command:"publish_coordinator_candidate_declaration",payload:{network_name:$n,willingness:"willing",public_reachability:{public_endpoint:$endpoint,reachable_via_relay:true},resources:{cpu_cores:1,memory_bytes:268435456,max_connections:64},validity_ms:600000}}')
    if ! ipc_command "$socket_path" "$command" >"$WORK/bootstrap-declaration.json" 2>"$WORK/bootstrap-declaration.err"; then
        sed -e 's/\x1b\[[0-9;]*m//g' "$WORK/bootstrap-declaration.err" >&2 || true
        fail "bootstrap candidate declaration was rejected"
    fi
    info "bootstrap governance: candidate revision 1 and owner declaration published"
}

main() {
    [ "$(id -u)" -eq 0 ] || fail "root is required for network namespaces and TUN devices"
    require_tools
    [ -x "$BIN" ] || fail "missing release binary: $BIN (run cargo build --release)"

    if ip netns list | grep -Eq "^($NSH|$NS1|$NS2)([[:space:]]|$)"; then
        fail "refusing to replace existing netns $NSH/$NS1/$NS2"
    fi
    if ip link show "$VH1" >/dev/null 2>&1 || ip link show "$VH2" >/dev/null 2>&1; then
        fail "refusing to replace existing veth $VH1/$VH2"
    fi
    local port
    for port in "$HOST_PORT" "$RELAY_PORT" "$WEBUI_HOST" "$WEBUI1" "$WEBUI2"; do
        if ss -H -lntu | awk '{print $5}' | grep -Eq "[:.]$port$"; then
            fail "host port $port is already in use"
        fi
    done

    mkdir -p "$WORK"
    trap cleanup EXIT INT TERM
    cat >"$WORK/tcp_echo_server.py" <<'PY'
import socket
import sys
import threading

family = socket.AF_INET6 if sys.argv[1] == "6" else socket.AF_INET
address, port = sys.argv[2], int(sys.argv[3])


def serve_client(connection):
    with connection:
        while True:
            data = connection.recv(65536)
            if not data:
                return
            connection.sendall(data)


with socket.socket(family, socket.SOCK_STREAM) as listener:
    listener.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    if family == socket.AF_INET6:
        listener.setsockopt(socket.IPPROTO_IPV6, socket.IPV6_V6ONLY, 1)
    listener.bind((address, port))
    listener.listen(16)
    while True:
        connection, _ = listener.accept()
        threading.Thread(target=serve_client, args=(connection,), daemon=True).start()
PY
    cat >"$WORK/udp_echo_server.py" <<'PY'
import socket
import sys

family = socket.AF_INET6 if sys.argv[1] == "6" else socket.AF_INET
address, port = sys.argv[2], int(sys.argv[3])

with socket.socket(family, socket.SOCK_DGRAM) as listener:
    listener.bind((address, port))
    while True:
        data, peer = listener.recvfrom(65536)
        listener.sendto(data, peer)
PY
    cat >"$WORK/tcp_echo_client.py" <<'PY'
import hashlib
import json
import socket
import sys
import time

address, port, size = sys.argv[1], int(sys.argv[2]), int(sys.argv[3])
payload = bytes(range(256)) * (size // 256) + bytes(range(size % 256))
started = time.monotonic()
with socket.socket(socket.AF_INET6, socket.SOCK_STREAM) as stream:
    stream.settimeout(10)
    stream.connect((address, port))
    stream.sendall(payload)
    stream.shutdown(socket.SHUT_WR)
    chunks = []
    while True:
        chunk = stream.recv(65536)
        if not chunk:
            break
        chunks.append(chunk)
received = b"".join(chunks)
result = {
    "ok": received == payload,
    "bytes": len(received),
    "sha256": hashlib.sha256(received).hexdigest(),
    "elapsed_ms": round((time.monotonic() - started) * 1000, 3),
}
print(json.dumps(result))
raise SystemExit(0 if result["ok"] else 1)
PY
    cat >"$WORK/udp_echo_client.py" <<'PY'
import hashlib
import json
import socket
import sys
import time

address, port, count = sys.argv[1], int(sys.argv[2]), int(sys.argv[3])
with socket.socket(socket.AF_INET6, socket.SOCK_DGRAM) as stream:
    stream.settimeout(2)
    received = []
    started = time.monotonic()
    for sequence in range(count):
        payload = f"W12-UDP-{sequence:04d}-".encode() + bytes([sequence % 256]) * 1000
        stream.sendto(payload, (address, port))
        data, _ = stream.recvfrom(65536)
        if data != payload:
            raise RuntimeError(f"UDP reply {sequence} did not match")
        received.append(sequence)
result = {
    "ok": len(received) == count,
    "datagrams": len(received),
    "sha256": hashlib.sha256(bytes(received)).hexdigest(),
    "elapsed_ms": round((time.monotonic() - started) * 1000, 3),
}
print(json.dumps(result))
raise SystemExit(0 if result["ok"] else 1)
PY
    info "workdir=$WORK"
    info "binary=$(git -C "$ROOT" rev-parse HEAD) sha256=$(sha256sum "$BIN" | awk '{print $1}')"
    info "topology: Coordinator netns $NSH 10.78.1.1:$HOST_PORT; node1 netns 10.78.1.2; node2 netns 10.78.2.2"

    ip rule show >"$WORK/host-ip-rule.before"
    ip -6 rule show >"$WORK/host-ip6-rule.before"
    ip route show default >"$WORK/host-default-route.before"

    info "creating routed veth underlay and isolated Coordinator/node namespaces"
    local i ns vh vn
    ip netns add "$NSH"
    ip netns exec "$NSH" ip link set lo up
    ip netns exec "$NSH" sysctl -qw net.ipv4.ip_forward=1
    for i in 1 2; do
        ns="w12soak$i"
        vh="w12so${i}h"
        vn="w12so${i}n"
        ip netns add "$ns"
        ip link add "$vh" type veth peer name "$vn"
        ip link set "$vn" netns "$ns"
        ip link set "$vh" netns "$NSH"
        ip netns exec "$NSH" ip addr add "10.78.$i.1/24" dev "$vh"
        ip netns exec "$NSH" ip link set "$vh" up
        ip netns exec "$ns" ip link set lo up
        ip netns exec "$ns" ip addr add "10.78.$i.2/24" dev "$vn"
        ip netns exec "$ns" ip link set "$vn" up
        ip netns exec "$ns" ip route add default via "10.78.$i.1"
    done

    local common_clean_env=(env -u DISPLAY -u WAYLAND_DISPLAY -u XDG_SESSION_TYPE -u XDG_CURRENT_DESKTOP -u DESKTOP_SESSION)
    local host_env=("${common_clean_env[@]}" HOME="$WORK/host/home" XDG_CONFIG_HOME="$WORK/host/xdg"
        FUCKIPV6_RUNTIME_DIR="$WORK/host/run" FUCKIPV6_CONFIG_DIR="$WORK/host/config")
    local node1_env=("${common_clean_env[@]}" HOME="$WORK/node1/home" XDG_CONFIG_HOME="$WORK/node1/xdg"
        FUCKIPV6_RUNTIME_DIR="$WORK/node1/run" FUCKIPV6_CONFIG_DIR="$WORK/node1/config")
    local node2_env=("${common_clean_env[@]}" HOME="$WORK/node2/home" XDG_CONFIG_HOME="$WORK/node2/xdg"
        FUCKIPV6_RUNTIME_DIR="$WORK/node2/run" FUCKIPV6_CONFIG_DIR="$WORK/node2/config")
    local rejected_env=("${common_clean_env[@]}" HOME="$WORK/rejected/home" XDG_CONFIG_HOME="$WORK/rejected/xdg"
        FUCKIPV6_RUNTIME_DIR="$WORK/rejected/run" FUCKIPV6_CONFIG_DIR="$WORK/rejected/config")
    mkdir -p "$WORK"/{host,node1,node2,rejected}/{home,xdg,run,config}
    write_ipc_client
    SOCKET_NS["$WORK/host/run/node.sock"]="$NSH"
    local headless_state="$WORK/no-desktop"
    local headless_exec="$headless_state/exec-headless"
    mkdir -p "$headless_state/run-user" "$headless_state/x11"
    cat >"$headless_exec" <<'SH'
#!/usr/bin/env bash
set -Eeuo pipefail
state=$1
shift
exec unshare --mount --propagation private bash -c '
    set -Eeuo pipefail
    mount --bind "$1" /run/user
    mount --bind "$2" /tmp/.X11-unix
    shift 2
    exec "$@"
' bash "$state/run-user" "$state/x11" "$@"
SH
    chmod +x "$headless_exec"

    info "starting Coordinator in isolated netns with its owner membership active"
    "$headless_exec" "$headless_state" ip netns exec "$NSH" "${host_env[@]}" RUST_LOG=debug "$BIN" host \
        --name "$NETWORK" --port "$HOST_PORT" --relay-port "$RELAY_PORT" \
        --external-host 10.78.1.1 --headless --webui-port "$WEBUI_HOST" \
        >"$WORK/host/host.log" 2>&1 &
    HPID=$!

    : >"$WORK/ticket.txt"
    for i in $(seq 1 60); do
        perl -pe 's/\e\[[0-9;]*[A-Za-z]//g; s/[\x00-\x08\x0b-\x1f\x7f]//g' "$WORK/host/host.log" \
            | grep -oE 'fk://[^[:space:]]+' | tail -1 >"$WORK/ticket.txt" || true
        [ "$(wc -c <"$WORK/ticket.txt")" -gt 20 ] && break
        sleep 1
    done
    [ "$(wc -c <"$WORK/ticket.txt")" -gt 20 ] || { tail -160 "$WORK/host/host.log" >&2 || true; fail "Coordinator did not emit a ticket"; }
    TICKET=$(<"$WORK/ticket.txt")
    HOST_SOCK="$WORK/host/run/node.sock"
    wait_for_socket "$HOST_SOCK" 60 || { tail -160 "$WORK/host/host.log" >&2 || true; fail "host node.sock did not appear"; }
    NETWORK_JSON="$WORK/host/config/$NETWORK/network.json"
    wait_for_file "$NETWORK_JSON" 60 || fail "authoritative network record did not appear"
    NETWORK_ID=$(jq -r '.network_id // empty' "$NETWORK_JSON")
    HOST_NODE_ID=$(jq -r '.owner // empty' "$NETWORK_JSON")
    [ -n "$NETWORK_ID" ] && [ -n "$HOST_NODE_ID" ] || fail "network record lacks network_id or owner"
    [ "$(jq -r '.join_mode' "$NETWORK_JSON")" = invite_only ] || fail "acceptance network is not InviteOnly"
    cp "$NETWORK_JSON" "$WORK/network-before.json"
    info "network_id=$NETWORK_ID owner=$HOST_NODE_ID join_mode=invite_only"
    wait_for_network_online "$HOST_SOCK" "$WORK/host-online.json" 90 || {
        tail -220 "$WORK/host/host.log" >&2 || true
        fail "Coordinator owner network did not become online"
    }
    jq -e --arg n "$NETWORK" '
        .data.configuration.networks[]? | select(.network_name==$n)
        | select(.exit_node == null and .routing_mode == "mesh_only" and .exit_node_enabled == false)
    ' "$WORK/host-online.json" >/dev/null || fail "Coordinator owner is not running mesh_only without an Exit Node"
    bootstrap_coordinator_governance "$HOST_SOCK" "$NETWORK" "$HOST_NODE_ID"

    info "starting two empty nodes in separate netns"
    "$headless_exec" "$headless_state" ip netns exec "$NS1" "${node1_env[@]}" RUST_LOG=debug "$BIN" --headless --webui-port "$WEBUI1" \
        >"$WORK/node1/node.log" 2>&1 &
    N1PID=$!
    "$headless_exec" "$headless_state" ip netns exec "$NS2" "${node2_env[@]}" RUST_LOG=debug "$BIN" --headless --webui-port "$WEBUI2" \
        >"$WORK/node2/node.log" 2>&1 &
    N2PID=$!
    NODE1_SOCK="$WORK/node1/run/node.sock"
    NODE2_SOCK="$WORK/node2/run/node.sock"
    SOCKET_NS["$HOST_SOCK"]="$NSH"
    SOCKET_NS["$NODE1_SOCK"]="$NS1"
    SOCKET_NS["$NODE2_SOCK"]="$NS2"
    wait_for_socket "$NODE1_SOCK" 60 || { tail -160 "$WORK/node1/node.log" >&2 || true; fail "node1 socket did not appear"; }
    wait_for_socket "$NODE2_SOCK" 60 || { tail -160 "$WORK/node2/node.log" >&2 || true; fail "node2 socket did not appear"; }
    snapshot_to "$NODE1_SOCK" "$WORK/node1-identity.json"
    snapshot_to "$NODE2_SOCK" "$WORK/node2-identity.json"
    NODE1_ID=$(jq -r '.data.identity.node_id // empty' "$WORK/node1-identity.json")
    NODE2_ID=$(jq -r '.data.identity.node_id // empty' "$WORK/node2-identity.json")
    [ -n "$NODE1_ID" ] && [ -n "$NODE2_ID" ] && [ "$NODE1_ID" != "$NODE2_ID" ] || fail "failed to obtain two distinct node identities"
    info "node1_id=$NODE1_ID"
    info "node2_id=$NODE2_ID"

    info "negative integration check: fk:// location ticket must not authorize InviteOnly membership"
    set +e
    timeout --kill-after=5s 60s "$headless_exec" "$headless_state" ip netns exec "$NS1" "${rejected_env[@]}" RUST_LOG=debug "$BIN" join "$TICKET" \
        --name "$NETWORK" --start --headless --webui-port 18973 \
        >"$WORK/rejected/join.log" 2>&1
    REJECT_RC=$?
    set -e
    [ "$REJECT_RC" -ne 0 ] || fail "fk:// location ticket unexpectedly joined an InviteOnly network"
    (grep -q '仅接受正式邀请' "$WORK/rejected/join.log" || grep -q 'invite_only' "$WORK/rejected/join.log") \
        || { tail -160 "$WORK/rejected/join.log" >&2 || true; fail "rejection did not report invite_only semantics"; }
    cmp -s "$WORK/network-before.json" "$NETWORK_JSON" || fail "rejected fk:// join mutated authoritative membership"
    info "fk:// rejection confirmed; authoritative roster unchanged"

    info "creating a formal max_uses=2 invitation through the host node command bus"
    local create_command preview_command redeem_command
    create_command=$(jq -cn --arg network "$NETWORK_ID" \
        '{command:"create_invitation",payload:{network_id:$network,role:"member",capabilities:[],max_uses:2,expires_in_ms:3600000}}')
    ipc_command "$HOST_SOCK" "$create_command" >"$WORK/invitation-created.json"
    INVITE_CODE=$(jq -r '.data.code // empty' "$WORK/invitation-created.json")
    INVITATION_ID=$(jq -r '.data.status.invitation.invitation_id // empty' "$WORK/invitation-created.json")
    [ -n "$INVITE_CODE" ] && [ -n "$INVITATION_ID" ] || { cat "$WORK/invitation-created.json" >&2; fail "invitation creation returned no code/id"; }
    info "invitation_id=$INVITATION_ID max_uses=2"

    local coordinator_url="ws://10.78.1.1:$HOST_PORT"
    local now_ms revision preview_digest terms_hash
    info "node1: preview then redeem formal invitation"
    preview_command=$(jq -cn --arg network "$NETWORK_ID" --arg url "$coordinator_url" --arg code "$INVITE_CODE" \
        '{command:"preview_invitation",payload:{network_id:$network,coordinator_url:$url,code:$code}}')
    ipc_command "$NODE1_SOCK" "$preview_command" >"$WORK/node1-preview.json"
    revision=$(jq -r '.data.invitation_revision' "$WORK/node1-preview.json")
    preview_digest=$(jq -r '.data.preview_digest' "$WORK/node1-preview.json")
    terms_hash=$(jq -r '.data.terms_hash' "$WORK/node1-preview.json")
    jq -e '.data.can_accept == true' "$WORK/node1-preview.json" >/dev/null || fail "node1 invitation preview is not acceptable"
    now_ms=$(date +%s%3N)
    redeem_command=$(jq -cn --arg url "$coordinator_url" --arg network "$NETWORK_ID" --arg code "$INVITE_CODE" \
        --arg invitation "$INVITATION_ID" --arg digest "$preview_digest" --arg terms "$terms_hash" \
        --argjson revision "$revision" --argjson now "$now_ms" \
        '{command:"redeem_invitation",payload:{coordinator_url:$url,request:{network_id:$network,code:$code,invitation_id:$invitation,expected_invitation_revision:$revision,preview_digest:$digest,terms_hash:$terms,acknowledged_at_unix_ms:$now,exit_contribution_acknowledged:true}}}')
    ipc_command "$NODE1_SOCK" "$redeem_command" >"$WORK/node1-redeem.json"
    jq -e --arg id "$NODE1_ID" '.data.node_id == $id and .data.grants_permanent_membership == false' "$WORK/node1-redeem.json" >/dev/null \
        || fail "node1 redemption did not grant ordinary membership"

    info "node2: re-preview after node1 incremented invitation revision, then redeem"
    preview_command=$(jq -cn --arg network "$NETWORK_ID" --arg url "$coordinator_url" --arg code "$INVITE_CODE" \
        '{command:"preview_invitation",payload:{network_id:$network,coordinator_url:$url,code:$code}}')
    ipc_command "$NODE2_SOCK" "$preview_command" >"$WORK/node2-preview.json"
    revision=$(jq -r '.data.invitation_revision' "$WORK/node2-preview.json")
    preview_digest=$(jq -r '.data.preview_digest' "$WORK/node2-preview.json")
    terms_hash=$(jq -r '.data.terms_hash' "$WORK/node2-preview.json")
    jq -e '.data.can_accept == true' "$WORK/node2-preview.json" >/dev/null || fail "node2 invitation preview is not acceptable"
    now_ms=$(date +%s%3N)
    redeem_command=$(jq -cn --arg url "$coordinator_url" --arg network "$NETWORK_ID" --arg code "$INVITE_CODE" \
        --arg invitation "$INVITATION_ID" --arg digest "$preview_digest" --arg terms "$terms_hash" \
        --argjson revision "$revision" --argjson now "$now_ms" \
        '{command:"redeem_invitation",payload:{coordinator_url:$url,request:{network_id:$network,code:$code,invitation_id:$invitation,expected_invitation_revision:$revision,preview_digest:$digest,terms_hash:$terms,acknowledged_at_unix_ms:$now,exit_contribution_acknowledged:true}}}')
    ipc_command "$NODE2_SOCK" "$redeem_command" >"$WORK/node2-redeem.json"
    jq -e --arg id "$NODE2_ID" '.data.node_id == $id and .data.grants_permanent_membership == false' "$WORK/node2-redeem.json" >/dev/null \
        || fail "node2 redemption did not grant ordinary membership"

    info "waiting for active networks and authoritative roster admission"
    wait_for_network_online "$NODE1_SOCK" "$WORK/node1-online.json" 90 || {
        tail -220 "$WORK/node1/node.log" >&2 || true
        fail "node1 network did not become online"
    }
    wait_for_network_online "$NODE2_SOCK" "$WORK/node2-online.json" 90 || {
        tail -220 "$WORK/node2/node.log" >&2 || true
        fail "node2 network did not become online"
    }
    local i
    for ((i = 0; i < 60; i++)); do
        if jq -e --arg n1 "$NODE1_ID" --arg n2 "$NODE2_ID" \
            '(.members | index($n1)) != null and (.members | index($n2)) != null' "$NETWORK_JSON" >/dev/null 2>&1; then
            break
        fi
        sleep 1
    done
    jq -e --arg n1 "$NODE1_ID" --arg n2 "$NODE2_ID" \
        '(.members | index($n1)) != null and (.members | index($n2)) != null' "$NETWORK_JSON" >/dev/null \
        || { jq '{owner,administrators,members,revision}' "$NETWORK_JSON" >&2; fail "authoritative roster did not admit both invitees"; }
    cp "$NETWORK_JSON" "$WORK/network-after-admission.json"
    jq '{owner,administrators,members,revision}' "$NETWORK_JSON" >"$WORK/network-roster.json"
    info "authoritative roster after formal redemption:"
    cat "$WORK/network-roster.json"

    IP1=$(jq -r --arg n "$NETWORK" '.data.networks[]? | select(.name==$n) | .assigned_ipv6 // empty' "$WORK/node1-online.json" | head -1)
    IP2=$(jq -r --arg n "$NETWORK" '.data.networks[]? | select(.name==$n) | .assigned_ipv6 // empty' "$WORK/node2-online.json" | head -1)
    IP1_V4=$(jq -r --arg n "$NETWORK" '.data.networks[]? | select(.name==$n) | .assigned_ipv4 // empty' "$WORK/node1-online.json" | head -1)
    IP2_V4=$(jq -r --arg n "$NETWORK" '.data.networks[]? | select(.name==$n) | .assigned_ipv4 // empty' "$WORK/node2-online.json" | head -1)
    TUN1=$(jq -r --arg n "$NETWORK" '.data.networks[]? | select(.name==$n) | .tun_name // empty' "$WORK/node1-online.json" | head -1)
    TUN2=$(jq -r --arg n "$NETWORK" '.data.networks[]? | select(.name==$n) | .tun_name // empty' "$WORK/node2-online.json" | head -1)
    [ -n "$IP1" ] && [ -n "$IP2" ] && [ "$IP1" != "$IP2" ] || fail "nodes did not receive distinct virtual IPv6 addresses"
    [ -n "$IP1_V4" ] && [ -n "$IP2_V4" ] && [ "$IP1_V4" != "$IP2_V4" ] || fail "nodes did not receive distinct virtual IPv4 addresses"
    [ -n "$TUN1" ] && [ -n "$TUN2" ] || fail "nodes did not expose their TUN interface names"
    info "virtual addresses: node1=$IP1/$IP1_V4 node2=$IP2/$IP2_V4 tun=$TUN1/$TUN2"

    jq -e --arg n "$NETWORK" '
        .data.configuration.networks[]? | select(.network_name==$n)
        | select(.exit_node == null and .routing_mode == "mesh_only" and .exit_node_enabled == false)
    ' "$WORK/node1-online.json" >/dev/null || fail "node1 is not running mesh_only without an Exit Node"
    jq -e --arg n "$NETWORK" '
        .data.configuration.networks[]? | select(.network_name==$n)
        | select(.exit_node == null and .routing_mode == "mesh_only" and .exit_node_enabled == false)
    ' "$WORK/node2-online.json" >/dev/null || fail "node2 is not running mesh_only without an Exit Node"

    cat >"$WORK/soak_loop.py" <<'PYSOAK'

import argparse
import json
import os
import pathlib
import statistics
import subprocess
import time
from datetime import datetime, timezone


def utc_now():
    return datetime.now(timezone.utc).isoformat().replace("+00:00", "Z")


def run(cmd, timeout=30):
    try:
        result = subprocess.run(cmd, capture_output=True, text=True, timeout=timeout)
    except subprocess.TimeoutExpired:
        return 124, "", "timeout"
    return result.returncode, result.stdout, result.stderr


def snapshot(ns, sock, client):
    code, out, _ = run(["ip", "netns", "exec", ns, "python3", client, sock,
                        '{"command":"get_snapshot"}'], timeout=30)
    if code != 0 or not out.strip():
        return None
    try:
        return json.loads(out)
    except json.JSONDecodeError:
        return None


def peer_view(snap, peer_ip):
    if not snap:
        return None
    for peer in snap.get("data", {}).get("peers", []):
        if peer.get("virtual_ipv6") == peer_ip:
            return {
                "online": peer.get("online"),
                "p2p_active": peer.get("p2p_active"),
                "p2p_healthy": peer.get("p2p_healthy"),
                "loss_rate": peer.get("loss_rate"),
                "traffic": peer.get("traffic", {}),
            }
    return None


def ping(ns, target, count=5):
    code, out, _ = run(["ip", "netns", "exec", ns, "env", "LC_ALL=C", "ping", "-6",
                        "-c", str(count), "-W", "2", "-i", "0.2", target], timeout=40)
    received = 0
    rtt = None
    for line in out.splitlines():
        if "packets transmitted" in line and "received" in line:
            parts = line.split(",")
            for part in parts:
                if "received" in part:
                    received = int(part.strip().split()[0])
        if "min/avg/max" in line:
            rtt = float(line.split("=")[1].split("/")[1].strip())
    return {"ok": code == 0 and received == count, "received": received, "count": count, "avg_rtt_ms": rtt}


def canary(ns, target, port, client, payload=65536):
    code, out, _ = run(["ip", "netns", "exec", ns, "python3", client, target, str(port), str(payload)], timeout=45)
    if code == 0:
        try:
            data = json.loads(out)
        except json.JSONDecodeError:
            data = {}
        return {"ok": bool(data.get("ok")), "elapsed_ms": data.get("elapsed_ms")}
    return {"ok": False, "error": (out or "")[-200:]}


def process_metrics(pids):
    metrics = {}
    for label, pid in pids.items():
        if not pid:
            continue
        rss = None
        fds = None
        try:
            with open(f"/proc/{pid}/status") as handle:
                for line in handle:
                    if line.startswith("VmRSS:"):
                        rss = int(line.split()[1])
        except OSError:
            pass
        try:
            fds = len(os.listdir(f"/proc/{pid}/fd"))
        except OSError:
            pass
        metrics[label] = {"pid": pid, "rss_kib": rss, "fd_count": fds}
    return metrics


def tree_files(directory):
    total = 0
    errors = 0
    for _, _, files in os.walk(directory):
        total += len(files)
    return {"files": total, "errors": errors}


def lock_pid(path):
    try:
        value = open(path).read().strip()
    except OSError:
        return None
    try:
        return int(value)
    except ValueError:
        return None


def node_pids(run_dir):
    return {
        "node": lock_pid(f"{run_dir}/node.lock"),
        "control": lock_pid(f"{run_dir}/control.lock"),
    }


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--workdir", required=True)
    parser.add_argument("--hours", type=float, default=24)
    parser.add_argument("--interval", type=int, default=60)
    parser.add_argument("--canary-interval", type=int, default=300)
    parser.add_argument("--ns1", required=True)
    parser.add_argument("--ns2", required=True)
    parser.add_argument("--sock1", required=True)
    parser.add_argument("--sock2", required=True)
    parser.add_argument("--ip1", required=True)
    parser.add_argument("--ip2", required=True)
    parser.add_argument("--tcp-port", type=int, required=True)
    parser.add_argument("--network", required=True)
    parser.add_argument("--ipc-client", required=True)
    parser.add_argument("--tcp-client", required=True)
    parser.add_argument("--node1-run", required=True)
    parser.add_argument("--node2-run", required=True)
    args = parser.parse_args()

    work = pathlib.Path(args.workdir)
    evidence = work / "soak"
    evidence.mkdir(parents=True, exist_ok=True)
    metrics_path = evidence / "metrics.jsonl"
    events_path = evidence / "events.jsonl"
    planned_seconds = args.hours * 3600
    started = time.monotonic()
    cycle = 0
    transitions = {}
    rss_series = {}
    failures = 0
    canaries_total = 0
    canaries_failed = 0

    (evidence / "run.json").write_text(json.dumps({
        "started_at": utc_now(),
        "planned_seconds": planned_seconds,
        "interval_seconds": args.interval,
        "canary_interval_seconds": args.canary_interval,
        "network": args.network,
        "nodes": [args.ip1, args.ip2],
        "scope": "Direct mesh-only P2P in private network namespaces; no Exit Node.",
    }, indent=2) + "\n")

    def event(kind, detail):
        with events_path.open("a") as handle:
            handle.write(json.dumps({"at": utc_now(), "kind": kind, "detail": detail}) + "\n")

    def transition(key, healthy, detail):
        previous = transitions.get(key)
        if previous != healthy:
            transitions[key] = healthy
            event("transition", {"key": key, "healthy": healthy, "detail": detail})

    try:
        while True:
            cycle += 1
            cycle_start = time.monotonic()
            record = {"cycle": cycle, "at": utc_now(), "elapsed_seconds": round(cycle_start - started, 3)}

            snap1 = snapshot(args.ns1, args.sock1, args.ipc_client)
            snap2 = snapshot(args.ns2, args.sock2, args.ipc_client)
            record["node1_peer"] = peer_view(snap1, args.ip2)
            record["node2_peer"] = peer_view(snap2, args.ip1)
            for label, view in (("node1_peer", record["node1_peer"]), ("node2_peer", record["node2_peer"])):
                healthy = bool(view and view["online"] and view["p2p_active"] and view["p2p_healthy"])
                transition(label, healthy, view)
                if not healthy:
                    failures += 1

            record["ping_1_to_2"] = ping(args.ns1, args.ip2)
            record["ping_2_to_1"] = ping(args.ns2, args.ip1)
            transition("ping_1_to_2", record["ping_1_to_2"]["ok"], record["ping_1_to_2"])
            transition("ping_2_to_1", record["ping_2_to_1"]["ok"], record["ping_2_to_1"])

            if cycle == 1 or (cycle - 1) * args.interval % args.canary_interval == 0:
                record["canary_1_to_2"] = canary(args.ns1, args.ip2, args.tcp_port, args.tcp_client)
                record["canary_2_to_1"] = canary(args.ns2, args.ip1, args.tcp_port, args.tcp_client)
                canaries_total += 2
                canaries_failed += 0 if record["canary_1_to_2"]["ok"] else 1
                canaries_failed += 0 if record["canary_2_to_1"]["ok"] else 1
                transition("canary_1_to_2", record["canary_1_to_2"]["ok"], record["canary_1_to_2"])
                transition("canary_2_to_1", record["canary_2_to_1"]["ok"], record["canary_2_to_1"])

            pids1 = node_pids(args.node1_run)
            pids2 = node_pids(args.node2_run)
            record["processes"] = {
                "node1": process_metrics({"node": pids1.get("node"), "control": pids1.get("control")}),
                "node2": process_metrics({"node": pids2.get("node"), "control": pids2.get("control")}),
            }
            record["runtime_files"] = {
                "node1": tree_files(f"{args.node1_run}/.."),
                "node2": tree_files(f"{args.node2_run}/.."),
            }
            for label, metrics in record["processes"].items():
                node = metrics.get("node") or {}
                if node.get("rss_kib"):
                    rss_series.setdefault(label, []).append(node["rss_kib"])

            with metrics_path.open("a") as handle:
                handle.write(json.dumps(record) + "\n")
            (evidence / "run-state.json").write_text(json.dumps({
                "cycle": cycle,
                "last_heartbeat": utc_now(),
                "elapsed_seconds": round(time.monotonic() - started, 3),
                "planned_seconds": planned_seconds,
            }, indent=2) + "\n")
            print(json.dumps({"event": "cycle", "cycle": cycle,
                              "elapsed_seconds": round(time.monotonic() - started, 3)}), flush=True)

            if time.monotonic() - started >= planned_seconds:
                break
            sleep_for = args.interval - (time.monotonic() - cycle_start)
            if sleep_for > 0:
                time.sleep(sleep_for)
    finally:
        growth = {}
        for label, series in rss_series.items():
            if len(series) > 1:
                growth[label] = {
                    "first_kib": series[0],
                    "last_kib": series[-1],
                    "max_kib": max(series),
                    "delta_kib": series[-1] - series[0],
                }
        summary = {
            "cycles": cycle,
            "elapsed_seconds": round(time.monotonic() - started, 3),
            "planned_seconds": planned_seconds,
            "failures": failures,
            "canaries": {"total": canaries_total, "failed": canaries_failed},
            "rss_growth": growth,
            "transitions": transitions,
            "verdict": "ok" if failures == 0 and canaries_failed == 0 else "review_required",
            "finished_at": utc_now(),
        }
        (evidence / "summary.json").write_text(json.dumps(summary, indent=2, sort_keys=True) + "\n")
        print(json.dumps({"event": "stopped", "summary": summary}, ensure_ascii=False), flush=True)


if __name__ == "__main__":
    main()
PYSOAK

    info "waiting for direct P2P discovery and health"
    wait_for_peer "$NODE1_SOCK" "$WORK/node1-before.json" "$IP2" 90 || fail "node1 did not report node2 as a healthy P2P peer"
    wait_for_peer "$NODE2_SOCK" "$WORK/node2-before.json" "$IP1" 90 || fail "node2 did not report node1 as a healthy P2P peer"

    info "starting TCP echo service on both virtual interfaces"
    ip netns exec "$NS1" python3 "$WORK/tcp_echo_server.py" 6 "$IP1" "$TCP_ECHO_PORT" >"$WORK/tcp-echo1.log" 2>&1 &
    TCP_ECHO1_PID=$!
    ip netns exec "$NS2" python3 "$WORK/tcp_echo_server.py" 6 "$IP2" "$TCP_ECHO_PORT" >"$WORK/tcp-echo2.log" 2>&1 &
    TCP_ECHO2_PID=$!
    wait_for_echo_port "$NS2" "$IP2" "$TCP_ECHO_PORT" node2
    wait_for_echo_port "$NS1" "$IP1" "$TCP_ECHO_PORT" node1

    info "soak: ${SOAK_HOURS}h, interval ${SOAK_INTERVAL}s, TCP canary every ${CANARY_INTERVAL}s"
    SOAK_ARGS=(
        --workdir "$WORK"
        --hours "$SOAK_HOURS"
        --interval "$SOAK_INTERVAL"
        --canary-interval "$CANARY_INTERVAL"
        --ns1 "$NS1" --ns2 "$NS2"
        --sock1 "$NODE1_SOCK" --sock2 "$NODE2_SOCK"
        --ip1 "$IP1" --ip2 "$IP2"
        --tcp-port "$TCP_ECHO_PORT"
        --network "$NETWORK"
        --ipc-client "$WORK/ipc_client.py"
        --tcp-client "$WORK/tcp_echo_client.py"
        --node1-run "$WORK/node1/run" --node2-run "$WORK/node2/run"
    )
    python3 "$WORK/soak_loop.py" "${SOAK_ARGS[@]}" || fail "soak loop reported a failure verdict"
    info "soak finished; evidence retained at $WORK"
}

main "$@"
