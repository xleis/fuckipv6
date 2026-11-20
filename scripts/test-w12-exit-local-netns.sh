#!/usr/bin/env bash
#
# W12-07 Exit acceptance in isolated namespaces: the network owner is also the
# Exit Node and reaches the internet through a dedicated NAT uplink, a member
# downloads a complete file with curl through that Exit, and stopping the network
# restores TUN, routes and DNS without manual repair.
#
#   member netns --veth-- owner/exit netns --veth-- this namespace --slirp--> internet
#
# The Exit data path is proven on both sides of the tunnel: the member TUN and the
# Exit TUN captures must both contain the download flow.
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
NETWORK="w12-exit"
NSH="w12exith"
NS1="w12exit1"
NS2="w12exit2"
VH1="w12ex1h"
VN1="w12ex1n"
VH2="w12ex2h"
VN2="w12ex2n"
HOST_PORT=19966
RELAY_PORT=19967
WEBUI_HOST=18960
WEBUI1=18961
WEBUI2=18962
TCP_ECHO_PORT=19391
UDP_ECHO_PORT=19392
WORK="/tmp/w12-exit-local.$(date -u +%Y%m%dT%H%M%SZ).$$"
DOWNLOAD_BYTES="${W12_DOWNLOAD_BYTES:-8388608}"

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
    ip link delete w12xu >/dev/null 2>&1 || true
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

# The Exit Node authorises a member's readiness probe only when its own local
# access policy carries an Exit contribution clause. A network created with
# `exit_contribution = not_required` leaves that clause absent, and the probe is
# rejected with `exit.contribution.policy_unavailable`.
apply_exit_contribution_policy() {
    local socket_path=$1 owner_node=$2
    local view revision local_revision now_ms expires_ms candidate command
    ipc_command "$socket_path" '{"command":"get_policy"}' >"$WORK/policy-before.json" \
        || fail "get_policy failed on the Exit Node"
    revision=$(jq -r '.data.revision // 0' "$WORK/policy-before.json")
    local_revision=$(jq -r '.data.snapshot.local_policy.revision // 0' "$WORK/policy-before.json")
    local_revision=$((local_revision + 1))
    now_ms=$(date +%s%3N)
    expires_ms=$((now_ms + 604800000))
    candidate=$(jq -cn --arg owner "$owner_node" --argjson now "$now_ms" --argjson exp "$expires_ms" \
        --argjson rev "$local_revision" \
        '{local_policy:{node_id:$owner,revision:$rev,issued_at_unix_ms:$now,expires_at_unix_ms:$exp,
           network_scope:{kind:"all"},contribution_vetoes:[],exit_contribution:{},relay_limits:{},rules:[]},
          network_acls:[]}')
    command=$(jq -cn --argjson cand "$candidate" --argjson rev "$revision" \
        '{command:"apply_policy",payload:{expected_revision:$rev,candidate:$cand,network_ids:[],
           includes_local_policy:true,confirm_privilege_expansion:true}}')
    ipc_command "$socket_path" "$command" >"$WORK/policy-apply.json" 2>"$WORK/policy-apply.err" \
        || { cat "$WORK/policy-apply.err" "$WORK/policy-apply.json" >&2 || true; fail "applying the Exit contribution policy failed"; }
    jq -e '.data.revision >= 1' "$WORK/policy-apply.json" >/dev/null \
        || fail "the Exit contribution policy did not apply"
    info "Exit contribution policy applied at revision $(jq -r '.data.revision' "$WORK/policy-apply.json")"
}

# A profile update reissues membership terms. Until the member acknowledges the
# new terms hash, the Exit contribution gate reports
# `exit.contribution.policy_confirmation_required` and the data path stays shut.
acknowledge_membership_terms() {
    local socket_path=$1 label=$2 snapshot=$3
    local terms_hash now_ms command
    snapshot_to "$socket_path" "$snapshot" || fail "snapshot failed on $label"
    if ! jq -e --arg n "$NETWORK" '[.data.memberships[]? | select(.network_name==$n) | select(.policy_confirmation_required==true)] | length >= 1' \
        "$snapshot" >/dev/null; then
        return 0
    fi
    terms_hash=$(jq -r --arg n "$NETWORK" '[.data.memberships[]? | select(.network_name==$n)][0].current_terms_hash' "$snapshot")
    [ -n "$terms_hash" ] && [ "$terms_hash" != null ] || fail "$label reported a pending policy without a terms hash"
    now_ms=$(date +%s%3N)
    command=$(jq -cn --arg id "$(jq -r --arg n "$NETWORK" '[.data.memberships[]? | select(.network_name==$n)][0].network_id' "$snapshot")" \
        --arg hash "$terms_hash" --argjson now "$now_ms" \
        '{command:"acknowledge_membership_policy",payload:{network_id:$id,expected_terms_hash:$hash,
           acknowledged_at_unix_ms:$now,exit_contribution_acknowledged:true}}')
    ipc_command "$socket_path" "$command" >"$WORK/ack-$label.json" 2>"$WORK/ack-$label.err" \
        || { cat "$WORK/ack-$label.err" "$WORK/ack-$label.json" >&2 || true; fail "$label could not acknowledge the updated policy terms"; }
    info "$label acknowledged the updated policy terms"
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
        ns="w12exit$i"
        vh="w12ex${i}h"
        vn="w12ex${i}n"
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
        --name "$NETWORK" --start --headless --webui-port 18963 \
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
    wait_for_peer "$NODE1_SOCK" "$WORK/node1-before.json" "$IP2" 90 || fail "node1 did not report node2 as a healthy P2P peer"
    wait_for_peer "$NODE2_SOCK" "$WORK/node2-before.json" "$IP1" 90 || fail "node2 did not report node1 as a healthy P2P peer"

    info "verifying the owner P2P path to the member"
    wait_for_peer "$HOST_SOCK" "$WORK/host-before.json" "$IP1" 90 || fail "the owner did not report the member as a healthy P2P peer"

    info "giving the Exit Node a dedicated NAT uplink"
    ip link add w12xu type veth peer name w12xn
    ip link set w12xn netns "$NSH"
    ip addr add 10.91.0.1/24 dev w12xu
    ip link set w12xu up
    ip netns exec "$NSH" ip addr add 10.91.0.2/24 dev w12xn
    ip netns exec "$NSH" ip link set w12xn up
    ip netns exec "$NSH" ip route add default via 10.91.0.1
    WAN_IF=$(ip route show default | awk '{print $5; exit}')
    [ -n "$WAN_IF" ] || fail "cannot determine the uplink interface for the Exit NAT"
    echo 1 >/proc/sys/net/ipv4/ip_forward
    iptables -A FORWARD -i w12xu -o "$WAN_IF" -j ACCEPT
    iptables -A FORWARD -i "$WAN_IF" -o w12xu -m state --state RELATED,ESTABLISHED -j ACCEPT
    iptables -t nat -A POSTROUTING -s 10.91.0.0/24 -o "$WAN_IF" -j MASQUERADE
    ip netns exec "$NSH" curl -sS --max-time 20 https://ifconfig.me >"$WORK/exit-egress.txt" 2>&1 \
        || { ip netns exec "$NSH" cat "$WORK/exit-egress.txt" >&2 || true; fail "the Exit Node has no internet egress"; }
    info "Exit Node egress: $(cat "$WORK/exit-egress.txt")"

    info "enabling and authorising the owner as the Exit Node"
    ip netns exec "$NSH" env HOME="$WORK/host/home" XDG_CONFIG_HOME="$WORK/host/xdg" \
        FUCKIPV6_RUNTIME_DIR="$WORK/host/run" FUCKIPV6_CONFIG_DIR="$WORK/host/config" \
        "$BIN" exit enable >"$WORK/exit-enable.txt" 2>&1 || fail "enabling the local Exit capability failed"
    role_command=$(jq -cn --arg n "$NETWORK" '{command:"set_network_role",payload:{network_name:$n,role:"exit_node",enabled:true}}')
    ipc_command "$HOST_SOCK" "$role_command" >"$WORK/exit-role.json" || fail "authorising the Exit Node role failed"

    info "publishing the administrator Exit authorization directory"
    apply_exit_contribution_policy "$HOST_SOCK" "$HOST_NODE_ID"

    profile_revision=$(jq -r '.revision' "$NETWORK_JSON")
    update_command=$(jq -cn --arg n "$NETWORK" --argjson rev "$profile_revision" \
        --argjson up "$(jq -c '{purpose,icon,language,region,tags,capability_declarations,visibility,join_mode,
            exit_contribution:"not_required",exit_contribution_scope:"not_applicable",
            exit_contribution_mode:"autonomous",
            exit_selection_policy:{mode:{mode:"fixed",exit:.owner},default_exit:.owner,
              available_exits:[.owner],denied_exits:[],groups:[],members:[],
              session_transition:"fail_closed"}}' "$NETWORK_JSON")" \
        '{command:"update_network_profile",payload:{network_name:$n,expected_revision:$rev,update:$up}}')
    ipc_command "$HOST_SOCK" "$update_command" >"$WORK/exit-directory.json" || fail "the Exit authorization directory update failed"
    jq -e '.status == "network_profile_updated"' "$WORK/exit-directory.json" >/dev/null || fail "the Exit directory update did not apply"

    info "acknowledging the reissued policy terms on every member"
    for ((i = 0; i < 30; i++)); do
        acknowledge_membership_terms "$HOST_SOCK" host "$WORK/host-terms.json"
        acknowledge_membership_terms "$NODE1_SOCK" node1 "$WORK/node1-terms.json"
        acknowledge_membership_terms "$NODE2_SOCK" node2 "$WORK/node2-terms.json"
        if ! jq -e --arg n "$NETWORK" '[.data.memberships[]? | select(.network_name==$n) | select(.policy_confirmation_required==true)] | length >= 1' \
            "$WORK/host-terms.json" >/dev/null; then
            break
        fi
        sleep 1
    done

    info "waiting for the member to see the authorised Exit"
    exit_selectable=0
    for ((i = 0; i < 60; i++)); do
        snapshot_to "$NODE1_SOCK" "$WORK/node1-exit.json" || true
        if jq -e '[.data.exit_selections[]?.resolution.selectable_exits // []] | flatten | length >= 1' \
            "$WORK/node1-exit.json" >/dev/null 2>&1; then
            exit_selectable=1
            break
        fi
        sleep 1
    done
    [ "$exit_selectable" = 1 ] || { jq -c '.data.exit_selections' "$WORK/node1-exit.json" >&2 || true; fail "the member never saw an authorised Exit"; }

    info "pointing the member at the Exit and routing its default into the TUN"
    routing_command=$(jq -cn --arg n "$NETWORK" --arg exit "$(jq -r --arg n "$NETWORK" '.data.networks[]? | select(.name==$n) | .assigned_ipv6' "$WORK/host-online.json" | head -1)" \
        '{command:"update_routing",payload:{network_name:$n,mode:"global_proxy",exit_node:$exit,is_exit_node:false}}')
    ipc_command "$NODE1_SOCK" "$routing_command" >"$WORK/node1-routing.json" || fail "update_routing was rejected"
    TUN1=$(jq -r --arg n "$NETWORK" '.data.networks[]? | select(.name==$n) | .tun_name' "$WORK/node1-exit.json" | head -1)
    [ -n "$TUN1" ] && [ "$TUN1" != null ] || fail "the member did not report a TUN name"
    ip netns exec "$NS1" ip route replace default dev "$TUN1"
    ip netns exec "$NS1" ip -6 route replace default dev "$TUN1"
    routed=0
    for ((i = 0; i < 60; i++)); do
        snapshot_to "$NODE1_SOCK" "$WORK/node1-routed.json" || true
        if jq -e '[.data.routing_summaries[]? | select(.default_action.kind == "exit")] | length >= 1' \
            "$WORK/node1-routed.json" >/dev/null 2>&1; then routed=1; break; fi
        sleep 1
    done
    [ "$routed" = 1 ] || { jq -c '.data.routing_summaries' "$WORK/node1-routed.json" >&2 || true; fail "the member never projected an Exit routing decision"; }
    jq -c '.data.routing_summaries[]?' "$WORK/node1-routed.json" >"$WORK/routing-summary.json"

    info "waiting for the Exit session to reach the protected state"
    protected=0
    for ((i = 0; i < 120; i++)); do
        snapshot_to "$NODE1_SOCK" "$WORK/node1-protected.json" || true
        if jq -e '.data.traffic_session.state == "protected"' "$WORK/node1-protected.json" >/dev/null 2>&1; then
            protected=1
            break
        fi
        sleep 1
    done
    [ "$protected" = 1 ] || { jq -c '.data.traffic_session' "$WORK/node1-protected.json" >&2 || true; fail "the Exit session never reached the protected state"; }
    jq -c '.data.traffic_session' "$WORK/node1-protected.json" >"$WORK/traffic-session.json"
    info "traffic session: $(cat "$WORK/traffic-session.json")"

    info "capturing both sides of the tunnel during the download"
    ip netns exec "$NS1" timeout 240 tcpdump -ni "$TUN1" -nn -c 20000 'tcp' >"$WORK/client-tun.txt" 2>"$WORK/client-tun.err" &
    CLIENT_CAP=$!
    TUNH=$(jq -r --arg n "$NETWORK" '.data.networks[]? | select(.name==$n) | .tun_name' "$WORK/host-online.json" | head -1)
    [ -n "$TUNH" ] && [ "$TUNH" != null ] || fail "the owner did not report a TUN name"
    ip netns exec "$NSH" timeout 240 tcpdump -ni "$TUNH" -nn -c 20000 'tcp' >"$WORK/exit-tun.txt" 2>"$WORK/exit-tun.err" &
    EXIT_CAP=$!

    ip netns exec "$NS1" cat /etc/resolv.conf >"$WORK/member-resolv-after.txt" 2>&1 || true
    ip netns exec "$NS1" ip -br addr show lo >"$WORK/member-lo.txt" 2>&1 || true
    ip netns exec "$NS1" getent hosts speed.cloudflare.com >"$WORK/member-dns-check.txt" 2>&1 || true
    info "member resolver: $(tr '\n' ' ' <"$WORK/member-resolv-after.txt")"
    info "member lo: $(cat "$WORK/member-lo.txt")"
    info "member DNS check: $(tr '\n' ' ' <"$WORK/member-dns-check.txt")"

    info "downloading $DOWNLOAD_BYTES bytes through the Exit with curl"
    # The validated Exit path covers IPv4 only; IPv6 is intentionally failed
    # closed, so pin the client to IPv4 instead of racing a reachable-family
    # fallback.
    ip netns exec "$NS1" curl -4 -sS --max-time 240 -o "$WORK/download.bin" \
        -w '{"http_code":%{http_code},"size_download":%{size_download},"speed":%{speed_download}}\n' \
        "https://speed.cloudflare.com/__down?bytes=$DOWNLOAD_BYTES" >"$WORK/download.json" 2>"$WORK/download.err" \
        || { cat "$WORK/download.err" >&2 || true; fail "the non-browser download through the Exit failed"; }
    jq -e '.http_code == 200 and .size_download > 0' "$WORK/download.json" >/dev/null || fail "the download did not complete"
    sha256sum "$WORK/download.bin" >"$WORK/download.sha256"
    got=$(stat -c %s "$WORK/download.bin")
    [ "$got" -ge "$DOWNLOAD_BYTES" ] || fail "download truncated: $got bytes"
    sleep 2
    kill_pid_checked "$CLIENT_CAP" TERM
    kill_pid_checked "$EXIT_CAP" TERM
    CLIENT_PACKETS=$(grep -c ' IP ' "$WORK/client-tun.txt" 2>/dev/null || true)
    EXIT_PACKETS=$(grep -c ' IP ' "$WORK/exit-tun.txt" 2>/dev/null || true)
    [ "${CLIENT_PACKETS:-0}" -gt 0 ] || fail "no packet entered the member TUN during the download"
    [ "${EXIT_PACKETS:-0}" -gt 0 ] || fail "the Exit TUN saw no packet: the download did not traverse the Exit"
    jq -n --argjson client "${CLIENT_PACKETS:-0}" --argjson exit_packets "${EXIT_PACKETS:-0}" \
        '{client_tun_packets:$client, exit_tun_packets:$exit_packets}' >"$WORK/tunnel-evidence.json"

    info "verifying DNS resolution inside the protected member namespace"
    ip netns exec "$NS1" getent hosts www.youtube.com >"$WORK/dns-member.txt" 2>&1 || true
    [ -s "$WORK/dns-member.txt" ] || fail "protected DNS resolution returned nothing"

    info "stopping the network through the production command bus"
    stop_command=$(jq -cn --arg n "$NETWORK" '{command:"stop_networks",payload:{networks:[$n],all:false}}')
    ipc_command "$NODE1_SOCK" "$stop_command" >"$WORK/stop-node1.json"
    ipc_command "$HOST_SOCK" "$stop_command" >"$WORK/stop-host.json"
    waited=0
    while [ "$waited" -lt 60 ]; do
        if ! ip netns exec "$NS1" ip link show | grep -q "fk-"; then break; fi
        sleep 1
        waited=$((waited + 1))
    done
    ip netns exec "$NS1" ip link show | grep -q "fk-" && fail "the member TUN survived stop_networks"
    ip netns exec "$NSH" ip link show | grep -q "fk-" && fail "the owner TUN survived stop_networks"
    ip netns exec "$NS1" ip -6 route show >"$WORK/member-routes-after.txt" 2>&1
    grep -qE 'fd00::/64|fd00::100' "$WORK/member-routes-after.txt" && fail "virtual routes survived stop_networks"

    jq -n --slurpfile dl "$WORK/download.json" --slurpfile tun "$WORK/tunnel-evidence.json" \
        --arg sha "$(cut -d' ' -f1 "$WORK/download.sha256")" \
        '{download:$dl[0], sha256:$sha, tunnel:$tun[0], tun_removed:true, routes_removed:true,
          exit_egress_path:true}' >"$WORK/SUMMARY.json"
    info "PASS: the member downloaded the file through the Exit, both TUN captures saw the flow, and stop removed TUN and routes"
    cat "$WORK/SUMMARY.json"
    info "evidence retained at $WORK"
}

main "$@"
