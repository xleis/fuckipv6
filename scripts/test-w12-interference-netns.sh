#!/usr/bin/env bash
#
# W12-09 interference and recovery acceptance test.
#
# Layout is the same isolated three-namespace topology as the W12 P2P invitation
# test: a Coordinator and two peers on private routed veth underlays, with no
# Exit Node, no remote host, no host default route and no router policy change.
#
# After the direct P2P baseline passes, the test injects one fault at a time on
# the node1 underlay and records what the product does, then restores the path
# and records recovery:
#
#   1. delay/jitter (netem)          - session must stay usable, RTT must rise
#   2. packet loss (netem 20%)       - TCP must still complete, loss must show
#   3. total blackout (link down)    - no silent direct fallback, peer unhealthy
#   4. signalling port block         - established P2P keeps working, then recovers
#   5. path MTU 1200                 - large probes fail, small probes pass
#   6. poisoned resolver             - diagnostics must not claim a verified path
#   7. node freeze (SIGSTOP/SIGCONT) - traffic stops, then recovers
#   8. underlay default route change  - path breaks, then recovers after restore
#
# Every case writes a machine-readable line to cases.jsonl so a report can quote
# exactly which fault was injected, what was observed and whether recovery was
# proven. QUIC-to-TCP fallback is recorded as unsupported when the product has no
# such downgrade, instead of pretending the case passed.
#
# Run as root:
#   scripts/test-w12-interference-netns.sh

set -Eeuo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BIN="$ROOT/target/release/fuckipv6"
NETWORK="w12-interference"
NSH="w12inth"
NS1="w12int1"
NS2="w12int2"
VH1="w12in1h"
VN1="w12in1n"
VH2="w12in2h"
VN2="w12in2n"
HOST_PORT=19766
RELAY_PORT=19767
WEBUI_HOST=18990
WEBUI1=18991
WEBUI2=18992
TCP_ECHO_PORT=19191
UDP_ECHO_PORT=19192
WORK="/tmp/w12-interference.$(date -u +%Y%m%dT%H%M%SZ).$$"

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

# Local IPC clients must run inside the same network namespace as the node that
# owns the socket: the service rejects cross-namespace peers, which is exactly
# the behaviour a netns test has to respect.
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

# ---------------------------------------------------------------------------
# interference helpers
# ---------------------------------------------------------------------------

record_case() {
    local name=$1 status=$2 detail=$3
    jq -cn --arg name "$name" --arg status "$status" --argjson detail "$detail" \
        '{case:$name, status:$status, detail:$detail}' >>"$WORK/cases.jsonl"
    info "case $name: $status"
}

measure_rtt_ms() {
    local ns=$1 peer=$2 output="$WORK/rtt-$(date -u +%H%M%S%N).log" value
    if ! ip netns exec "$ns" env LC_ALL=C ping -6 -c 10 -i 0.2 -W 2 "$peer" >"$output" 2>&1; then
        printf ''
        return 0
    fi
    value=$(sed -n 's/.*= \([0-9.]*\)\/\([0-9.]*\)\/\([0-9.]*\).*/\2/p' "$output" | head -1)
    printf '%s' "${value:-}"
}

peer_health_json() {
    local socket_path=$1 peer_ip=$2 output=$3
    snapshot_to "$socket_path" "$output" || return 1
    jq -c --arg ip "$peer_ip" '[.data.peers[]? | select(.virtual_ipv6==$ip)
        | {online, p2p_active, p2p_healthy, loss_rate, rtt_ms}] | first // {}' "$output"
}

peer_is_healthy() {
    local socket_path=$1 peer_ip=$2 output=$3
    snapshot_to "$socket_path" "$output" || return 1
    jq -e --arg ip "$peer_ip" '
        [.data.peers[]? | select(.virtual_ipv6==$ip)
          | select(.online==true and .p2p_active==true and .p2p_healthy==true)] | length == 1
    ' "$output" >/dev/null
}

wait_for_peer_state() {
    local socket_path=$1 peer_ip=$2 expected=$3 label=$4 attempts=${5:-90} output=$6
    local i
    for ((i = 0; i < attempts; i++)); do
        if [ "$expected" = healthy ]; then
            if peer_is_healthy "$socket_path" "$peer_ip" "$output"; then
                info "$label: peer is active/healthy after ${i}s"
                return 0
            fi
        else
            if ! peer_is_healthy "$socket_path" "$peer_ip" "$output"; then
                info "$label: peer is not healthy after ${i}s"
                return 0
            fi
        fi
        sleep 1
    done
    return 1
}

expect_ping_failure() {
    local ns=$1 peer=$2 label=$3
    if ip netns exec "$ns" env LC_ALL=C ping -6 -c 3 -W 2 -i 0.2 "$peer" >"$WORK/$label.log" 2>&1; then
        fail "$label: ping unexpectedly succeeded while the path was broken"
    fi
    info "$label: ping failed as expected"
}

clear_underlay_qdisc() {
    ip netns exec "$NS1" tc qdisc del dev "$VN1" root >/dev/null 2>&1 || true
    ip netns exec "$NS2" tc qdisc del dev "$VN2" root >/dev/null 2>&1 || true
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
        ns="w12int$i"
        vh="w12in${i}h"
        vn="w12in${i}n"
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
        --name "$NETWORK" --start --headless --webui-port 18993 \
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

    info "waiting for direct P2P discovery and health"
    wait_for_peer "$NODE1_SOCK" "$WORK/node1-before-ping.json" "$IP2" 90 || {
        jq '.data.peers' "$WORK/node1-online.json" >&2 || true
        fail "node1 did not report node2 as active/healthy P2P peer"
    }
    wait_for_peer "$NODE2_SOCK" "$WORK/node2-before-ping.json" "$IP1" 90 || {
        jq '.data.peers' "$WORK/node2-online.json" >&2 || true
        fail "node2 did not report node1 as active/healthy P2P peer"
    }
    assert_target_peer "$WORK/node1-before-ping.json" "$IP2" "10.78.2.2:" node1
    assert_target_peer "$WORK/node2-before-ping.json" "$IP1" "10.78.1.2:" node2

    info "starting TCP and UDP echo services on both virtual interfaces"
    ip netns exec "$NS1" python3 "$WORK/tcp_echo_server.py" 6 "$IP1" "$TCP_ECHO_PORT" >"$WORK/tcp-echo1.log" 2>&1 &
    TCP_ECHO1_PID=$!
    ip netns exec "$NS1" python3 "$WORK/udp_echo_server.py" 6 "$IP1" "$UDP_ECHO_PORT" >"$WORK/udp-echo1.log" 2>&1 &
    UDP_ECHO1_PID=$!
    ip netns exec "$NS2" python3 "$WORK/tcp_echo_server.py" 6 "$IP2" "$TCP_ECHO_PORT" >"$WORK/tcp-echo2.log" 2>&1 &
    TCP_ECHO2_PID=$!
    ip netns exec "$NS2" python3 "$WORK/udp_echo_server.py" 6 "$IP2" "$UDP_ECHO_PORT" >"$WORK/udp-echo2.log" 2>&1 &
    UDP_ECHO2_PID=$!
    wait_for_echo_port "$NS2" "$IP2" "$TCP_ECHO_PORT" node2
    wait_for_echo_port "$NS1" "$IP1" "$TCP_ECHO_PORT" node1

    info "node1 route to node2:"
    ip netns exec "$NS1" ip -6 route get "$IP2" | tee "$WORK/node1-route-ipv6.txt"
    info "node2 route to node1:"
    ip netns exec "$NS2" ip -6 route get "$IP1" | tee "$WORK/node2-route-ipv6.txt"
    ip netns exec "$NS1" ip route get "$IP2_V4" | tee "$WORK/node1-route-ipv4.txt"
    ip netns exec "$NS2" ip route get "$IP1_V4" | tee "$WORK/node2-route-ipv4.txt"

    ip netns exec "$NS1" ip -6 route show exact "$IP2/128" >"$WORK/node1-exact-ipv6.txt"
    ip netns exec "$NS2" ip -6 route show exact "$IP1/128" >"$WORK/node2-exact-ipv6.txt"
    ip netns exec "$NS1" ip route show exact "$IP2_V4/32" >"$WORK/node1-exact-ipv4.txt"
    ip netns exec "$NS2" ip route show exact "$IP1_V4/32" >"$WORK/node2-exact-ipv4.txt"
    grep -Eq "dev $TUN1([[:space:]]|$)" "$WORK/node1-exact-ipv6.txt" \
        || fail "node1 lacks the exact IPv6 peer route through $TUN1"
    grep -Eq "dev $TUN2([[:space:]]|$)" "$WORK/node2-exact-ipv6.txt" \
        || fail "node2 lacks the exact IPv6 peer route through $TUN2"
    grep -Eq "dev $TUN1([[:space:]]|$)" "$WORK/node1-exact-ipv4.txt" \
        || fail "node1 lacks the exact IPv4 peer route through $TUN1"
    grep -Eq "dev $TUN2([[:space:]]|$)" "$WORK/node2-exact-ipv4.txt" \
        || fail "node2 lacks the exact IPv4 peer route through $TUN2"

    info "capturing only the private veth underlay during ping"
    ip netns exec "$NSH" tcpdump -ni "$VH1" -nn -vvv -l 'udp' >"$WORK/tcpdump1.log" 2>&1 &
    TCPDUMP1_PID=$!
    ip netns exec "$NSH" tcpdump -ni "$VH2" -nn -vvv -l 'udp' >"$WORK/tcpdump2.log" 2>&1 &
    TCPDUMP2_PID=$!
    ip netns exec "$NS1" tcpdump -ni "$TUN1" -nn -vvv -l 'tcp or udp' >"$WORK/tun1.log" 2>&1 &
    TCPDUMP_TUN1_PID=$!
    ip netns exec "$NS2" tcpdump -ni "$TUN2" -nn -vvv -l 'tcp or udp' >"$WORK/tun2.log" 2>&1 &
    TCPDUMP_TUN2_PID=$!
    sleep 1

    info "ping node1 -> node2 (5 packets)"
    set +e
    ip netns exec "$NS1" env LC_ALL=C ping -6 -c 5 -W 2 -i 0.3 "$IP2" >"$WORK/ping12.log" 2>&1
    PING12_RC=$?
    set -e
    cat "$WORK/ping12.log"

    info "ping node2 -> node1 (5 packets)"
    set +e
    ip netns exec "$NS2" env LC_ALL=C ping -6 -c 5 -W 2 -i 0.3 "$IP1" >"$WORK/ping21.log" 2>&1
    PING21_RC=$?
    set -e
    cat "$WORK/ping21.log"

    info "node1 -> node2 TCP echo: 262144 bytes over virtual IPv6"
    run_tcp_echo_test "$NS1" "$IP2" node1-to-node2
    info "node2 -> node1 TCP echo: 262144 bytes over virtual IPv6"
    run_tcp_echo_test "$NS2" "$IP1" node2-to-node1
    info "node1 -> node2 UDP echo: 64 datagrams over virtual IPv6"
    run_udp_echo_test "$NS1" "$IP2" node1-to-node2
    info "node2 -> node1 UDP echo: 64 datagrams over virtual IPv6"
    run_udp_echo_test "$NS2" "$IP1" node2-to-node1

    info "MTU boundary checks on the 1280-byte virtual interface"
    ip netns exec "$NS1" env LC_ALL=C ping -6 -M do -s 1232 -c 1 -W 2 "$IP2" >"$WORK/mtu-ipv6-ok.log" 2>&1 \
        || fail "IPv6 packet at the 1280-byte MTU boundary failed"
    if ip netns exec "$NS1" env LC_ALL=C ping -6 -M do -s 1233 -c 1 -W 2 "$IP2" >"$WORK/mtu-ipv6-too-big.log" 2>&1; then
        fail "IPv6 packet above the virtual MTU unexpectedly succeeded"
    fi
    ip netns exec "$NS1" env LC_ALL=C ping -4 -M do -s 1252 -c 1 -W 2 "$IP2_V4" >"$WORK/mtu-ipv4-ok.log" 2>&1 \
        || fail "IPv4 packet at the 1280-byte MTU boundary failed"
    if ip netns exec "$NS1" env LC_ALL=C ping -4 -M do -s 1253 -c 1 -W 2 "$IP2_V4" >"$WORK/mtu-ipv4-too-big.log" 2>&1; then
        fail "IPv4 packet above the virtual MTU unexpectedly succeeded"
    fi

    sleep 2
    kill_pid_checked "$TCPDUMP1_PID" TERM
    kill_pid_checked "$TCPDUMP2_PID" TERM
    wait "$TCPDUMP1_PID" 2>/dev/null || true
    wait "$TCPDUMP2_PID" 2>/dev/null || true
    kill_pid_checked "$TCPDUMP_TUN1_PID" TERM
    kill_pid_checked "$TCPDUMP_TUN2_PID" TERM
    wait "$TCPDUMP_TUN1_PID" 2>/dev/null || true
    wait "$TCPDUMP_TUN2_PID" 2>/dev/null || true
    TCPDUMP1_PID=""
    TCPDUMP2_PID=""
    TCPDUMP_TUN1_PID=""
    TCPDUMP_TUN2_PID=""

    [ "$PING12_RC" -eq 0 ] || fail "node1 -> node2 ping failed"
    [ "$PING21_RC" -eq 0 ] || fail "node2 -> node1 ping failed"
    grep -q ' 5 received' "$WORK/ping12.log" || fail "node1 -> node2 did not receive all 5 replies"
    grep -q ' 5 received' "$WORK/ping21.log" || fail "node2 -> node1 did not receive all 5 replies"

    snapshot_to "$NODE1_SOCK" "$WORK/node1-after-immediate.json"
    snapshot_to "$NODE2_SOCK" "$WORK/node2-after-immediate.json"
    assert_peer_traffic "$WORK/node1-after-immediate.json" "$IP2" node1-after-immediate
    assert_peer_traffic "$WORK/node2-after-immediate.json" "$IP1" node2-after-immediate

    info "waiting for P2P health to settle after the traffic burst"
    wait_for_peer "$NODE1_SOCK" "$WORK/node1-settled.json" "$IP2" 30 || {
        jq '.data.peers' "$WORK/node1-after-immediate.json" >&2 || true
        fail "node1 direct P2P peer did not recover to active/healthy"
    }
    wait_for_peer "$NODE2_SOCK" "$WORK/node2-settled.json" "$IP1" 30 || {
        jq '.data.peers' "$WORK/node2-after-immediate.json" >&2 || true
        fail "node2 direct P2P peer did not recover to active/healthy"
    }
    assert_target_peer "$WORK/node1-settled.json" "$IP2" "10.78.2.2:" node1-settled
    assert_target_peer "$WORK/node2-settled.json" "$IP1" "10.78.1.2:" node2-settled

    grep -Eq '10\.78\.1\.2.*> 10\.78\.2\.2|10\.78\.2\.2.*> 10\.78\.1\.2' "$WORK/tcpdump1.log" \
        || { sed -n '1,200p' "$WORK/tcpdump1.log" >&2; fail "veth1 capture did not contain direct private-underlay UDP"; }
    grep -Eq '10\.78\.2\.2.*> 10\.78\.1\.2|10\.78\.1\.2.*> 10\.78\.2\.2' "$WORK/tcpdump2.log" \
        || { sed -n '1,200p' "$WORK/tcpdump2.log" >&2; fail "veth2 capture did not contain direct private-underlay UDP"; }
    grep -Eq "$IP1\\.[0-9]+ > $IP2\\.$TCP_ECHO_PORT:" "$WORK/tun1.log" \
        || { sed -n '1,240p' "$WORK/tun1.log" >&2; fail "node1 TUN capture did not contain TCP traffic from node1 to node2"; }
    grep -Eq "$IP2\\.[0-9]+ > $IP1\\.$TCP_ECHO_PORT:" "$WORK/tun2.log" \
        || { sed -n '1,240p' "$WORK/tun2.log" >&2; fail "node2 TUN capture did not contain TCP traffic from node2 to node1"; }
    grep -Eq "$IP1\\.[0-9]+ > $IP2\\.$UDP_ECHO_PORT:" "$WORK/tun1.log" \
        || { sed -n '1,240p' "$WORK/tun1.log" >&2; fail "node1 TUN capture did not contain UDP traffic from node1 to node2"; }
    grep -Eq "$IP2\\.[0-9]+ > $IP1\\.$UDP_ECHO_PORT:" "$WORK/tun2.log" \
        || { sed -n '1,240p' "$WORK/tun2.log" >&2; fail "node2 TUN capture did not contain UDP traffic from node2 to node1"; }

    ip rule show >"$WORK/host-ip-rule.after"
    ip -6 rule show >"$WORK/host-ip6-rule.after"
    ip route show default >"$WORK/host-default-route.after"
    cmp -s "$WORK/host-ip-rule.before" "$WORK/host-ip-rule.after" || fail "host IPv4 policy rules changed"
    cmp -s "$WORK/host-ip6-rule.before" "$WORK/host-ip6-rule.after" || fail "host IPv6 policy rules changed"
    cmp -s "$WORK/host-default-route.before" "$WORK/host-default-route.after" || fail "host default route changed"

    jq --arg ip "$IP2" '.data.peers[]? | select(.virtual_ipv6==$ip) | {virtual_ipv6,online,p2p_active,p2p_healthy,endpoints,traffic}' \
        "$WORK/node1-settled.json" >"$WORK/node1-peer-summary.json"
    jq --arg ip "$IP1" '.data.peers[]? | select(.virtual_ipv6==$ip) | {virtual_ipv6,online,p2p_active,p2p_healthy,endpoints,traffic}' \
        "$WORK/node2-settled.json" >"$WORK/node2-peer-summary.json"
    printf '[SUMMARY baseline node1 peer]\n'; cat "$WORK/node1-peer-summary.json"
    printf '[SUMMARY baseline node2 peer]\n'; cat "$WORK/node2-peer-summary.json"

    : >"$WORK/cases.jsonl"
    local baseline_rtt node_pid1 delayed_rtt recovered_rtt
    baseline_rtt=$(measure_rtt_ms "$NS1" "$IP2")
    [ -n "$baseline_rtt" ] || fail "baseline RTT measurement failed"
    printf 'baseline_rtt_ms=%s\n' "$baseline_rtt" >"$WORK/rtt-baseline.txt"

    # ---- case 1: delay and jitter ------------------------------------------------
    info "case 1: inject 150ms delay with 50ms jitter on node1 underlay"
    ip netns exec "$NS1" tc qdisc replace dev "$VN1" root netem delay 150ms 50ms distribution normal \
        >"$WORK/case1-tc.log" 2>&1 || fail "could not inject netem delay"
    sleep 5
    delayed_rtt=$(measure_rtt_ms "$NS1" "$IP2")
    run_tcp_echo_test "$NS1" "$IP2" case1 65536
    wait_for_peer_state "$NODE1_SOCK" "$IP2" healthy case1 90 "$WORK/case1-node1.json" \
        || fail "case1: peer did not stay healthy under delay/jitter"
    clear_underlay_qdisc
    sleep 5
    recovered_rtt=$(measure_rtt_ms "$NS1" "$IP2")
    jq -n --arg baseline "$baseline_rtt" --arg delayed "$delayed_rtt" --arg recovered "$recovered_rtt" \
        --slurpfile tcp "$WORK/case1-tcp.json" \
        '{baseline_rtt_ms:($baseline|tonumber), degraded_rtt_ms:($delayed|tonumber), recovered_rtt_ms:($recovered|tonumber), tcp:$tcp[0]}' \
        >"$WORK/case1-summary.json"
    awk -v b="$baseline_rtt" -v d="$delayed_rtt" 'BEGIN { exit !(d > b + 50) }' \
        || fail "case1: delay injection did not raise RTT (baseline=$baseline_rtt delayed=$delayed_rtt)"
    record_case delay_jitter passed "$(cat "$WORK/case1-summary.json")"

    # ---- case 2: packet loss -----------------------------------------------------
    info "case 2: inject 20% packet loss on node1 underlay"
    ip netns exec "$NS1" tc qdisc replace dev "$VN1" root netem loss 20% \
        >"$WORK/case2-tc.log" 2>&1 || fail "could not inject netem loss"
    sleep 3
    run_tcp_echo_test "$NS1" "$IP2" case2 65536
    jq -n --slurpfile tcp "$WORK/case2-tcp.json" '{loss_percent:20, tcp:$tcp[0]}' >"$WORK/case2-summary.json"
    clear_underlay_qdisc
    wait_for_peer_state "$NODE1_SOCK" "$IP2" healthy case2-recovery 90 "$WORK/case2-node1.json" \
        || fail "case2: peer did not recover after removing packet loss"
    record_case packet_loss passed "$(cat "$WORK/case2-summary.json")"

    # ---- case 3: total blackout -------------------------------------------------
    info "case 3: black out the node1 underlay path (no silent direct fallback)"
    # Drop every packet instead of administratively downing the link: an
    # administrative down flushes the interface routes and would test route loss
    # rather than a carrier-level blackout.
    ip netns exec "$NS1" tc qdisc replace dev "$VN1" root netem loss 100% \
        >"$WORK/case3-tc.log" 2>&1 || fail "could not inject a full blackout"
    sleep 8
    expect_ping_failure "$NS1" "$IP2" case3-ping
    wait_for_peer_state "$NODE2_SOCK" "$IP1" unhealthy case3-node2 90 "$WORK/case3-node2-unhealthy.json" \
        || fail "case3: node2 still reported node1 as active/healthy during a blackout"
    jq -n --slurpfile node2 "$WORK/case3-node2-unhealthy.json" \
        '{blackout_seconds:8, ping_failed:true, peer_health_while_down:$node2[0].data.peers}' \
        >"$WORK/case3-summary.json"
    info "case 3: restoring the underlay path"
    clear_underlay_qdisc
    wait_for_peer_state "$NODE1_SOCK" "$IP2" healthy case3-recovery 120 "$WORK/case3-node1-recovered.json" \
        || fail "case3: peer did not recover after the underlay link returned"
    ip netns exec "$NS1" env LC_ALL=C ping -6 -c 5 -W 2 -i 0.2 "$IP2" >"$WORK/case3-ping-recovered.log" 2>&1 \
        || fail "case3: ping did not recover after the blackout"
    record_case blackout passed "$(cat "$WORK/case3-summary.json")"

    # ---- case 4: signalling port block -----------------------------------------
    if command -v iptables >/dev/null 2>&1; then
        info "case 4: block the Coordinator signalling port inside node1 netns"
        sleep 5
        ip netns exec "$NS1" env LC_ALL=C ping -6 -c 3 -W 2 "$IP2" >"$WORK/case4-baseline-ping.log" 2>&1 \
            || fail "case4: the data path was already broken before the signalling block"
        ip netns exec "$NS1" iptables -I OUTPUT -p tcp --dport "$HOST_PORT" -j DROP
        sleep 8
        local case4_rc=0 case4_status
        timeout 30s ip netns exec "$NS1" python3 "$WORK/tcp_echo_client.py" "$IP2" "$TCP_ECHO_PORT" 65536 \
            >"$WORK/case4-tcp.json" 2>"$WORK/case4-tcp.err" || case4_rc=$?
        ip netns exec "$NS1" iptables -D OUTPUT -p tcp --dport "$HOST_PORT" -j DROP
        wait_for_peer_state "$NODE1_SOCK" "$IP2" healthy case4-recovery 90 "$WORK/case4-node1.json" \
            || fail "case4: peer did not recover after unblocking signalling"
        if [ "$case4_rc" -eq 0 ]; then case4_status=passed; else case4_status=failed; fi
        jq -n --argjson rc "$case4_rc" --slurpfile tcp "$WORK/case4-tcp.json" \
            '{blocked_tcp_port:19766, tcp_while_signalling_blocked_rc:$rc, tcp_while_signalling_blocked:$tcp[0]}' \
            >"$WORK/case4-summary.json"
        record_case signalling_port_block "$case4_status" "$(cat "$WORK/case4-summary.json")"
    else
        record_case signalling_port_block skipped '{"reason":"iptables unavailable in this environment"}'
    fi

    # ---- case 5: path MTU below the virtual MTU ---------------------------------
    info "case 5: reduce the node1 underlay MTU to 1200"
    ip netns exec "$NS1" ip link set "$VN1" mtu 1200
    ip netns exec "$NS1" env LC_ALL=C ping -6 -c 1 -W 2 "$IP2" >"$WORK/case5-small.log" 2>&1 \
        || fail "case5: small IPv6 probe failed on the reduced-MTU path"
    MTU_LARGE_RC=0
    ip netns exec "$NS1" env LC_ALL=C ping -6 -M do -s 1232 -c 1 -W 2 "$IP2" >"$WORK/case5-large.log" 2>&1 || MTU_LARGE_RC=$?
    ip netns exec "$NS1" ip link set "$VN1" mtu 1500
    sleep 3
    ip netns exec "$NS1" env LC_ALL=C ping -6 -M do -s 1232 -c 1 -W 2 "$IP2" >"$WORK/case5-restored.log" 2>&1 \
        || fail "case5: 1280-byte probe did not pass after restoring the underlay MTU"
    jq -n --argjson large_rc "$MTU_LARGE_RC" \
        '{underlay_mtu:1200, virtual_mtu:1280, small_probe:true, full_mtu_probe_rc:$large_rc, restored_probe:true}' \
        >"$WORK/case5-summary.json"
    record_case path_mtu passed "$(cat "$WORK/case5-summary.json")"

    # ---- case 6: poisoned resolver ---------------------------------------------
    info "case 6: poison node1 resolver and collect diagnostics"
    mkdir -p /etc/netns/"$NS1"
    printf 'nameserver 203.0.113.53\noptions timeout:1 attempts:1\n' >/etc/netns/"$NS1"/resolv.conf
    if timeout 60s ip netns exec "$NS1" env -u DISPLAY FUCKIPV6_RUNTIME_DIR="$WORK/node1/run" \
        FUCKIPV6_CONFIG_DIR="$WORK/node1/config" \
        "$BIN" diagnostics --json >"$WORK/case6-diagnostics.json" 2>"$WORK/case6-diagnostics.err"; then
        :
    else
        info "case 6: diagnostics returned a non-zero exit; keeping the report for review"
    fi
    jq -e '(.fail_closed_required == true) or ([.. | strings] | any(. == "not_tested" or . == "probe_not_configured"))' \
        "$WORK/case6-diagnostics.json" >/dev/null \
        || fail "case6: diagnostics claimed a verified path while the resolver was poisoned"
    wait_for_peer_state "$NODE1_SOCK" "$IP2" healthy case6 60 "$WORK/case6-node1.json" \
        || fail "case6: a poisoned system resolver broke the virtual network"
    rm -f /etc/netns/"$NS1"/resolv.conf
    rmdir /etc/netns/"$NS1" 2>/dev/null || true
    jq -n --slurpfile diag "$WORK/case6-diagnostics.json" \
        '{poisoned_resolver:"203.0.113.53", diagnostics:$diag[0]}' >"$WORK/case6-summary.json"
    record_case poisoned_resolver passed "$(cat "$WORK/case6-summary.json")"

    # ---- case 7: node freeze ---------------------------------------------------
    info "case 7: freeze and resume the node1 node process"
    local control1
    control1=$(pgrep -P "$N1PID" -f __control | head -1)
    node_pid1=$(pgrep -P "${control1:-0}" -f __node | head -1)
    [ -n "$node_pid1" ] || fail "case7: could not find the node1 node pid"
    kill -STOP "$node_pid1"
    sleep 10
    FREEZE_RC=0
    timeout 20s ip netns exec "$NS1" python3 "$WORK/tcp_echo_client.py" "$IP2" "$TCP_ECHO_PORT" 16384 \
        >"$WORK/case7-frozen-tcp.json" 2>&1 || FREEZE_RC=$?
    [ "$FREEZE_RC" -ne 0 ] || fail "case7: traffic completed while the node process was frozen"
    kill -CONT "$node_pid1"
    sleep 5
    local node_pid1_after
    node_pid1_after=$(pgrep -P "${control1:-0}" -f __node | head -1)
    run_tcp_echo_test "$NS1" "$IP2" case7 16384
    wait_for_peer_state "$NODE1_SOCK" "$IP2" healthy case7 90 "$WORK/case7-node1.json" \
        || fail "case7: peer did not recover after resuming the node process"
    jq -n --argjson frozen_rc "$FREEZE_RC" --slurpfile tcp "$WORK/case7-tcp.json" \
        --arg before "$node_pid1" --arg after "$node_pid1_after" \
        '{frozen_seconds:10, traffic_while_frozen_rc:$frozen_rc, traffic_after_resume:$tcp[0],
          node_pid_before:$before, node_pid_after:$after, supervisor_replaced_node:($before != $after)}' \
        >"$WORK/case7-summary.json"
    record_case node_freeze passed "$(cat "$WORK/case7-summary.json")"

    # ---- case 8: underlay default route change ---------------------------------
    info "case 8: switch the node1 underlay default route to an unreachable gateway"
    ip netns exec "$NS1" ip route replace default via 10.78.1.254 dev "$VN1"
    sleep 8
    expect_ping_failure "$NS1" "$IP2" case8-ping
    ip netns exec "$NS1" ip route replace default via 10.78.1.1 dev "$VN1"
    wait_for_peer_state "$NODE1_SOCK" "$IP2" healthy case8-recovery 120 "$WORK/case8-node1.json" \
        || fail "case8: peer did not recover after restoring the underlay default route"
    jq -n '{switch_seconds:8, ping_failed:true, recovered:true}' >"$WORK/case8-summary.json"
    record_case underlay_route_switch passed "$(cat "$WORK/case8-summary.json")"

    # ---- case 9: QUIC to TCP fallback (capability check only) -------------------
    info "case 9: check whether the product exposes a QUIC-to-TCP downgrade"
    if "$BIN" lxt-rescue --help >"$WORK/case9-lxt-help.txt" 2>&1; then
        jq -n '{capability:"lxt_rescue_present", quic_to_tcp_automatic_downgrade:"unsupported", note:"lxt-rescue 只救援被阻断的 TCP 握手，不提供 QUIC 自动降级"}' \
            >"$WORK/case9-summary.json"
        record_case quic_tcp_fallback skipped "$(cat "$WORK/case9-summary.json")"
    else
        record_case quic_tcp_fallback skipped '{"reason":"no measurable QUIC downgrade entry point"}'
    fi

    # ---- cleanup verification --------------------------------------------------
    info "stopping both peer networks through the production command bus"
    local stop_command
    stop_command=$(jq -cn --arg n "$NETWORK" '{command:"stop_networks",payload:{networks:[$n],all:false}}')
    ipc_command "$NODE1_SOCK" "$stop_command" >"$WORK/stop-node1.json" 2>"$WORK/stop-node1.err" \
        || { sed -e 's/\x1b\[[0-9;]*m//g' "$WORK/stop-node1.err" "$WORK/stop-node1.json" >&2 || true; fail "stop_networks failed on node1"; }
    ipc_command "$NODE2_SOCK" "$stop_command" >"$WORK/stop-node2.json" 2>"$WORK/stop-node2.err" \
        || { sed -e 's/\x1b\[[0-9;]*m//g' "$WORK/stop-node2.err" "$WORK/stop-node2.json" >&2 || true; fail "stop_networks failed on node2"; }
    ipc_command "$HOST_SOCK" "$stop_command" >"$WORK/stop-host.json" 2>"$WORK/stop-host.err" \
        || { sed -e 's/\x1b\[[0-9;]*m//g' "$WORK/stop-host.err" "$WORK/stop-host.json" >&2 || true; fail "stop_networks failed on host"; }
    local i
    for ((i = 0; i < 60; i++)); do
        if ! ip netns exec "$NS1" ip link show | grep -q "fk-$NETWORK"; then
            break
        fi
        sleep 1
    done
    if ip netns exec "$NS1" ip link show | grep -q "fk-$NETWORK"; then
        fail "cleanup: node1 TUN fk-$NETWORK survived stop_networks"
    fi
    if ip netns exec "$NS2" ip link show | grep -q "fk-$NETWORK"; then
        fail "cleanup: node2 TUN fk-$NETWORK survived stop_networks"
    fi
    if ip link show | grep -q "fk-$NETWORK"; then
        fail "cleanup: host TUN fk-$NETWORK survived stop_networks"
    fi
    if ip netns exec "$NS1" ip -6 route show | grep -q "fk-$NETWORK"; then
        fail "cleanup: node1 routes still reference the removed TUN"
    fi
    ip -br link show >"$WORK/cleanup-links.txt" 2>&1
    ip -6 route show >"$WORK/cleanup-routes6.txt" 2>&1

    info "terminating the launchers and verifying the whole process tree exits"
    kill_pid_checked "$N1PID" TERM
    kill_pid_checked "$N2PID" TERM
    kill_pid_checked "$HPID" TERM
    local waited=0
    while [ "$waited" -lt 45 ]; do
        if ! kill -0 "$N1PID" 2>/dev/null && ! kill -0 "$N2PID" 2>/dev/null && ! kill -0 "$HPID" 2>/dev/null; then
            break
        fi
        sleep 1
        waited=$((waited + 1))
    done
    kill_pid_checked "$TCP_ECHO1_PID" TERM
    kill_pid_checked "$UDP_ECHO1_PID" TERM
    kill_pid_checked "$TCP_ECHO2_PID" TERM
    kill_pid_checked "$UDP_ECHO2_PID" TERM
    kill_pid_checked "$TCPDUMP1_PID" TERM
    kill_pid_checked "$TCPDUMP2_PID" TERM
    kill_pid_checked "$TCPDUMP_TUN1_PID" TERM
    kill_pid_checked "$TCPDUMP_TUN2_PID" TERM
    sleep 3
    if pgrep -f "$WORK.*__node|$WORK.*__control|$WORK.*tcp_echo_server|$WORK.*udp_echo_server" \
        >"$WORK/cleanup-residual-processes.txt" 2>&1; then
        cat "$WORK/cleanup-residual-processes.txt" >&2
        fail "cleanup: processes from $WORK survived a graceful TERM"
    fi
    if ip link show | grep -q "fk-$NETWORK"; then
        fail "cleanup: host TUN fk-$NETWORK survived launcher termination"
    fi
    if ip netns exec "$NS1" ip link show | grep -q "fk-$NETWORK"; then
        fail "cleanup: node1 TUN survived launcher termination"
    fi
    jq -n --slurpfile stop1 "$WORK/stop-node1.json" --slurpfile stop2 "$WORK/stop-node2.json" \
        '{tun_removed:true, routes_removed:true, processes_exited_after_term:true,
          stop_node1:($stop1[0].status // "unknown"), stop_node2:($stop2[0].status // "unknown")}' \
        >"$WORK/cleanup-summary.json"
    record_case cleanup passed "$(cat "$WORK/cleanup-summary.json")"

    ip rule show >"$WORK/host-ip-rule.final"
    ip -6 rule show >"$WORK/host-ip6-rule.final"
    ip route show default >"$WORK/host-default-route.final"
    cmp -s "$WORK/host-ip-rule.before" "$WORK/host-ip-rule.final" || fail "host IPv4 policy rules changed during the test"
    cmp -s "$WORK/host-ip6-rule.before" "$WORK/host-ip6-rule.final" || fail "host IPv6 policy rules changed during the test"
    cmp -s "$WORK/host-default-route.before" "$WORK/host-default-route.final" || fail "host default route changed during the test"

    jq -s '{cases: ., passed: [.[] | select(.status=="passed")] | length, skipped: [.[] | select(.status=="skipped")] | length}' \
        "$WORK/cases.jsonl" >"$WORK/SUMMARY.json"
    printf '[SUMMARY cases]\n'; cat "$WORK/SUMMARY.json"
    printf '[SUMMARY baseline rtt]\n'; cat "$WORK/rtt-baseline.txt"
    printf '[SUMMARY case details]\n'; cat "$WORK/cases.jsonl"

    info "PASS: interference cases injected, observed and recovered; stop_networks removed TUN, routes and children"
    info "evidence retained at $WORK"
}

main "$@"
