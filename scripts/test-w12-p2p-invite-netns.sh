#!/usr/bin/env bash
#
# W12 P2P acceptance: two independent network namespaces join one InviteOnly
# network through the production invitation flow, then exchange direct UDP P2P
# traffic and virtual IPv6 ICMP without an Exit Node.
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
NETWORK="w12-p2p-invite"
NSH="w12pinvh"
NS1="w12pinv1"
NS2="w12pinv2"
VH1="w12pi1h"
VN1="w12pi1n"
VH2="w12pi2h"
VN2="w12pi2n"
HOST_PORT=19666
RELAY_PORT=19667
WEBUI_HOST=18980
WEBUI1=18981
WEBUI2=18982
WORK="/tmp/w12-p2p-invite.$(date -u +%Y%m%dT%H%M%SZ).$$"

HPID=""
N1PID=""
N2PID=""
TCPDUMP1_PID=""
TCPDUMP2_PID=""

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
    kill_pid_checked "$N1PID" TERM
    kill_pid_checked "$N2PID" TERM
    kill_pid_checked "$HPID" TERM
    for dir in "$WORK/host" "$WORK/node1" "$WORK/node2" "$WORK/rejected"; do
        kill_runtime_locks "$dir/run"
    done
    sleep 2
    kill_pid_checked "$TCPDUMP1_PID" KILL
    kill_pid_checked "$TCPDUMP2_PID" KILL
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

ipc_command() {
    local socket_path=$1 command_json=$2
    python3 - "$socket_path" "$command_json" <<'PY'
import json
import socket
import struct
import sys

socket_path, command_json = sys.argv[1:3]
command = json.loads(command_json)
request_id = 1
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
    stream.sendall(FRAME_HEADER.pack(b"FKIP", 3, kind, 0, len(payload)))
    stream.sendall(payload)


def recv_frame(stream):
    magic, version, kind, reserved, length = FRAME_HEADER.unpack(recv_exact(stream, 12))
    if magic != b"FKIP" or version != 3 or reserved != 0:
        raise RuntimeError("invalid local IPC frame header")
    if length > 1024 * 1024:
        raise RuntimeError("oversized local IPC frame")
    return kind, json.loads(recv_exact(stream, length))


with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as stream:
    stream.settimeout(30)
    stream.connect(socket_path)
    send_frame(stream, 1, {"protocol_version": 3, "role": "cli"})
    kind, hello = recv_frame(stream)
    if kind != 2 or hello.get("protocol_version") != 3:
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
PY
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
        ns="w12pinv$i"
        vh="w12pi${i}h"
        vn="w12pi${i}n"
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

    info "starting two empty nodes in separate netns"
    "$headless_exec" "$headless_state" ip netns exec "$NS1" "${node1_env[@]}" RUST_LOG=debug "$BIN" --headless --webui-port "$WEBUI1" \
        >"$WORK/node1/node.log" 2>&1 &
    N1PID=$!
    "$headless_exec" "$headless_state" ip netns exec "$NS2" "${node2_env[@]}" RUST_LOG=debug "$BIN" --headless --webui-port "$WEBUI2" \
        >"$WORK/node2/node.log" 2>&1 &
    N2PID=$!
    NODE1_SOCK="$WORK/node1/run/node.sock"
    NODE2_SOCK="$WORK/node2/run/node.sock"
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
        --name "$NETWORK" --start --headless --webui-port 18983 \
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
    [ -n "$IP1" ] && [ -n "$IP2" ] && [ "$IP1" != "$IP2" ] || fail "nodes did not receive distinct virtual IPv6 addresses"
    info "virtual addresses: node1=$IP1 node2=$IP2"

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

    info "node1 route to node2:"
    ip netns exec "$NS1" ip -6 route get "$IP2"
    info "node2 route to node1:"
    ip netns exec "$NS2" ip -6 route get "$IP1"

    info "capturing only the private veth underlay during ping"
    ip netns exec "$NSH" tcpdump -ni "$VH1" -nn -vvv -l 'udp' >"$WORK/tcpdump1.log" 2>&1 &
    TCPDUMP1_PID=$!
    ip netns exec "$NSH" tcpdump -ni "$VH2" -nn -vvv -l 'udp' >"$WORK/tcpdump2.log" 2>&1 &
    TCPDUMP2_PID=$!
    sleep 1

    info "ping node1 -> node2 (5 packets)"
    set +e
    ip netns exec "$NS1" ping -6 -c 5 -W 2 -i 0.3 "$IP2" >"$WORK/ping12.log" 2>&1
    PING12_RC=$?
    set -e
    cat "$WORK/ping12.log"

    info "ping node2 -> node1 (5 packets)"
    set +e
    ip netns exec "$NS2" ping -6 -c 5 -W 2 -i 0.3 "$IP1" >"$WORK/ping21.log" 2>&1
    PING21_RC=$?
    set -e
    cat "$WORK/ping21.log"

    sleep 2
    kill_pid_checked "$TCPDUMP1_PID" TERM
    kill_pid_checked "$TCPDUMP2_PID" TERM
    wait "$TCPDUMP1_PID" 2>/dev/null || true
    wait "$TCPDUMP2_PID" 2>/dev/null || true
    TCPDUMP1_PID=""
    TCPDUMP2_PID=""

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
    printf '[SUMMARY node1 peer]\n'; cat "$WORK/node1-peer-summary.json"
    printf '[SUMMARY node2 peer]\n'; cat "$WORK/node2-peer-summary.json"
    printf '[SUMMARY ping12]\n'; tail -4 "$WORK/ping12.log"
    printf '[SUMMARY ping21]\n'; tail -4 "$WORK/ping21.log"
    printf '[SUMMARY direct UDP samples]\n'
    grep -E '10\.78\.[12]\.2' "$WORK/tcpdump1.log" | head -3 || true
    grep -E '10\.78\.[12]\.2' "$WORK/tcpdump2.log" | head -3 || true

    info "PASS: formal invitations admitted both nodes; direct P2P is healthy; bidirectional ICMP 5/5"
    info "result: code authorization boundary was the blocker; private veth data path is working"
}

main "$@"
