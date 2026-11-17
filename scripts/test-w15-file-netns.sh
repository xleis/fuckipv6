#!/usr/bin/env bash
#
# W15 File acceptance: two independent member nodes use the production
# invitation, signed network-profile, P2P and File data planes to transfer
# files through one owner in isolated network namespaces.
#
# This test also exercises pause/resume across a veth outage and a complete
# node-service SIGKILL/restart, cancellation cleanup, live ACL revocation, live
# network File-capability revocation, path-free observability and resource
# cleanup. It also saturates all four File transfer slots with real P2P uploads
# while probing the coordinator, owner and member control planes. All network
# changes remain inside three private namespaces and two 10.79.x/24 veth pairs.
# The host default route, policy rules, DNS, OpenClash and firewall are never
# modified.
#
# Run as root:
#   sudo -n scripts/test-w15-file-netns.sh

set -Eeuo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BIN="$ROOT/target/release/fuckipv6"
NETWORK="w15-file-netns"
NSH="w15fileh"
NS1="w15file1"
NS2="w15file2"
VH1="w15fi1h"
VN1="w15fi1n"
VH2="w15fi2h"
VN2="w15fi2n"
HOST_PORT=19766
RELAY_PORT=19767
WEBUI_HOST=18991
WEBUI1=18992
WEBUI2=18993
WORK="/tmp/w15-file.$(date -u +%Y%m%dT%H%M%SZ).$$"

HPID=""
N1PID=""
N2PID=""
HOST_NODE_PID=""
NODE1_NODE_PID=""
NODE2_NODE_PID=""
STARTED_TRANSFER_ID=""
STARTED_OPERATION_ID=""
CLEANED=0

info() { printf '[INFO] %s\n' "$*"; }
fail() { printf '[FAIL] %s\n' "$*" >&2; exit 1; }

require_tools() {
    local tool
    for tool in base64 cmp find git grep ip jq mount openssl perl python3 rustc sed sha256sum ss stat timeout tr unshare; do
        command -v "$tool" >/dev/null 2>&1 || fail "missing required tool: $tool"
    done
}

kill_pid_checked() {
    local pid=${1:-} sig=${2:-TERM}
    [ -n "$pid" ] || return 0
    case "$pid" in
        *[!0-9]*) return 0 ;;
    esac
    kill -"$sig" "$pid" 2>/dev/null || true
}

lock_pid() {
    local lock=$1
    [ -f "$lock" ] || return 1
    tr -dc '0-9' <"$lock" 2>/dev/null || true
}

kill_runtime_locks() {
    local dir=$1 lock pid
    for lock in "$dir/control.lock" "$dir/node.lock"; do
        pid=$(lock_pid "$lock" || true)
        [ -n "$pid" ] || continue
        kill_pid_checked "$pid" TERM
    done
}

cleanup() {
    [ "$CLEANED" -eq 1 ] && return 0
    CLEANED=1
    set +e
    info "cleaning up tracked processes and test namespaces"
    kill_pid_checked "$N1PID" TERM
    kill_pid_checked "$N2PID" TERM
    kill_pid_checked "$HPID" TERM
    for dir in "$WORK/host" "$WORK/node1" "$WORK/node2"; do
        kill_runtime_locks "$dir/run"
    done
    sleep 2
    kill_pid_checked "$N1PID" KILL
    kill_pid_checked "$N2PID" KILL
    kill_pid_checked "$HPID" KILL
    for dir in "$WORK/host" "$WORK/node1" "$WORK/node2"; do
        kill_runtime_locks "$dir/run"
    done
    ip netns delete "$NS1" >/dev/null 2>&1 || true
    ip netns delete "$NS2" >/dev/null 2>&1 || true
    ip netns delete "$NSH" >/dev/null 2>&1 || true
    if [ -d "${WORK:-}" ]; then
        info "evidence retained at $WORK"
    fi
}

trap cleanup EXIT INT TERM

ipc_command() {
    local socket_path=$1 command_json=$2 ns=""
    local -a runner=(python3)
    case "$socket_path" in
        "$WORK/host/"*) ns=$NSH ;;
        "$WORK/node1/"*) ns=$NS1 ;;
        "$WORK/node2/"*) ns=$NS2 ;;
    esac
    [ -z "$ns" ] || runner=(ip netns exec "$ns" python3)
    "${runner[@]}" - "$socket_path" "$command_json" <<'PY'
import json
import socket
import struct
import sys

socket_path, command_json = sys.argv[1:3]
command = json.loads(command_json)
request_id = 1
IPC_VERSION = 5
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
    stream.sendall(FRAME_HEADER.pack(b"FKIP", IPC_VERSION, kind, 0, len(payload)))
    stream.sendall(payload)


def recv_frame(stream):
    magic, version, kind, reserved, length = FRAME_HEADER.unpack(recv_exact(stream, 12))
    if magic != b"FKIP" or version != IPC_VERSION or reserved != 0:
        raise RuntimeError("invalid local IPC frame header")
    if length > 1024 * 1024:
        raise RuntimeError("oversized local IPC frame")
    return kind, json.loads(recv_exact(stream, length))


with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as stream:
    stream.settimeout(180)
    stream.connect(socket_path)
    send_frame(stream, 1, {"protocol_version": IPC_VERSION, "role": "cli"})
    kind, hello = recv_frame(stream)
    if kind != 2 or hello.get("protocol_version") != IPC_VERSION:
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

ipc_local_command() {
    local socket_path=$1 command_json=$2 ns=""
    local -a runner=(python3)
    case "$socket_path" in
        "$WORK/host/"*) ns=$NSH ;;
        "$WORK/node1/"*) ns=$NS1 ;;
        "$WORK/node2/"*) ns=$NS2 ;;
    esac
    [ -z "$ns" ] || runner=(ip netns exec "$ns" python3)
    "${runner[@]}" - "$socket_path" "$command_json" <<'PY'
import json
import socket
import struct
import sys

socket_path, request_json = sys.argv[1:3]
request = json.loads(request_json)
if not isinstance(request, dict) or not isinstance(request.get("command"), dict):
    raise RuntimeError("local IPC request must contain an object-valued command")
request_id = request.get("request_id")
if not isinstance(request_id, int) or isinstance(request_id, bool) or request_id < 1:
    raise RuntimeError("local IPC request must contain a positive integer request_id")
IPC_VERSION = 5
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
    stream.sendall(FRAME_HEADER.pack(b"FKIP", IPC_VERSION, kind, 0, len(payload)))
    stream.sendall(payload)


def recv_frame(stream):
    magic, version, kind, reserved, length = FRAME_HEADER.unpack(recv_exact(stream, 12))
    if magic != b"FKIP" or version != IPC_VERSION or reserved != 0:
        raise RuntimeError("invalid local IPC frame header")
    if length > 1024 * 1024:
        raise RuntimeError("oversized local IPC frame")
    return kind, json.loads(recv_exact(stream, length))


with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as stream:
    stream.settimeout(180)
    stream.connect(socket_path)
    send_frame(stream, 1, {"protocol_version": IPC_VERSION, "role": "cli"})
    kind, hello = recv_frame(stream)
    if kind != 2 or hello.get("protocol_version") != IPC_VERSION:
        raise RuntimeError(f"local IPC handshake failed: {hello}")
    send_frame(stream, 3, request)
    while True:
        kind, value = recv_frame(stream)
        if kind == 5:
            continue
        if kind == 6:
            print(json.dumps(value, indent=2, ensure_ascii=False), file=sys.stderr)
            raise SystemExit(3)
        if kind != 4:
            raise RuntimeError(f"unexpected local IPC frame kind: {kind}")
        break

print(json.dumps(value, indent=2, ensure_ascii=False))
PY
}

snapshot_to() {
    local socket_path=$1 output=$2
    ipc_command "$socket_path" '{"command":"get_snapshot"}' >"$output" 2>>"${WORK:-/tmp}/ipc-client.err"
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

wait_for_live_socket() {
    local socket_path=$1 output=$2 attempts=${3:-60}
    local i
    for ((i = 0; i < attempts; i++)); do
        if WORK="$WORK" NSH="$NSH" NS1="$NS1" NS2="$NS2" \
            timeout 5s bash -c 'source /dev/stdin' <<EOF >/dev/null 2>&1
$(declare -f ipc_command snapshot_to)
snapshot_to "$socket_path" "$output"
EOF
        then
            return 0
        fi
        sleep 1
    done
    return 1
}

ready_node_pid() {
    local log=$1
    python3 - "$log" <<'PY'
import json
import sys

last = None
with open(sys.argv[1], "r", encoding="utf-8", errors="replace") as handle:
    for line in handle:
        try:
            record = json.loads(line)
        except json.JSONDecodeError:
            continue
        if record.get("role") == "node" and record.get("state") == "ready":
            pid = record.get("pid")
            if isinstance(pid, int) and pid > 0:
                last = pid
if last is not None:
    print(last)
PY
}

wait_for_ready_node_pid() {
    local log=$1 attempts=${2:-120} previous_pid=${3:-} i pid
    for ((i = 0; i < attempts; i++)); do
        pid=$(ready_node_pid "$log" 2>/dev/null || true)
        if [ -n "$pid" ] && [ "$pid" != "$previous_pid" ] && kill -0 "$pid" 2>/dev/null; then
            printf '%s\n' "$pid"
            return 0
        fi
        sleep 0.1
    done
    return 1
}

wait_for_pid_gone() {
    local pid=$1 attempts=${2:-300} i
    for ((i = 0; i < attempts; i++)); do
        ! kill -0 "$pid" 2>/dev/null && return 0
        sleep 0.1
    done
    return 1
}

wait_for_network_online() {
    local socket_path=$1 output=$2 attempts=${3:-90}
    local i status ip
    for ((i = 0; i < attempts; i++)); do
        if snapshot_to "$socket_path" "$output" 2>/dev/null; then
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
    local i online active healthy
    for ((i = 0; i < attempts; i++)); do
        if snapshot_to "$socket_path" "$output" 2>/dev/null; then
            read -r online active healthy < <(jq -r --arg ip "$peer_ip" '
                .data.peers[]? | select(.virtual_ipv6==$ip) | [.online,.p2p_active,.p2p_healthy] | @tsv
            ' "$output" 2>/dev/null | head -1)
            if [ "$online" = true ] && [ "$active" = true ] && [ "$healthy" = true ]; then
                return 0
            fi
        fi
        sleep 1
    done
    return 1
}

profile_file_capability_matches() {
    local snapshot=$1 expected=$2 now_ms
    now_ms=$(date +%s%3N)
    if [ "$expected" = yes ]; then
        jq -e --arg n "$NETWORK" --argjson now "$now_ms" '
            [.data.network_profiles[]? | select(.name==$n)
             | select(.capability_declarations | index("file"))
             | select(.signed_profile.issued_at_unix_ms > 0)
             | select(.signed_profile.expires_at_unix_ms > $now)] | length == 1
        ' "$snapshot" >/dev/null
    else
        jq -e --arg n "$NETWORK" '
            [.data.network_profiles[]? | select(.name==$n)
             | select((.capability_declarations | index("file")) == null)] | length == 1
        ' "$snapshot" >/dev/null
    fi
}

wait_for_profile_file_capability() {
    local socket_path=$1 output=$2 expected=$3 label=$4 attempts=${5:-90}
    local i
    for ((i = 0; i < attempts; i++)); do
        if snapshot_to "$socket_path" "$output" 2>/dev/null \
            && profile_file_capability_matches "$output" "$expected"; then
            return 0
        fi
        sleep 1
    done
    jq --arg n "$NETWORK" '.data.network_profiles[]? | select(.name==$n)' "$output" >&2 || true
    if [ "$expected" = yes ]; then
        fail "$label lacks a current signed File capability declaration"
    else
        fail "$label still declares File capability"
    fi
}

wait_for_file_state() {
    local socket_path=$1 output=$2 expected=$3 attempts=${4:-60}
    local i state active
    for ((i = 0; i < attempts; i++)); do
        if snapshot_to "$socket_path" "$output" 2>/dev/null; then
            state=$(jq -r '.data.file_capability.state' "$output" 2>/dev/null)
            active=$(jq -r '.data.file_capability.active_transfers' "$output" 2>/dev/null)
            if [ "$state" = "$expected" ] && [ "$active" = 0 ]; then
                return 0
            fi
        fi
        sleep 1
    done
    jq '.data.file_capability' "$output" >&2 || true
    return 1
}

wait_for_file_active_transfers() {
    local socket_path=$1 output=$2 expected=$3 attempts=${4:-60}
    local i state active
    for ((i = 0; i < attempts; i++)); do
        if snapshot_to "$socket_path" "$output" 2>/dev/null; then
            state=$(jq -r '.data.file_capability.state' "$output" 2>/dev/null)
            active=$(jq -r '.data.file_capability.active_transfers' "$output" 2>/dev/null)
            if [ "$state" = enabled ] && [ "$active" = "$expected" ]; then
                return 0
            fi
        fi
        sleep 0.2
    done
    jq '.data.file_capability' "$output" >&2 || true
    return 1
}

control_snapshot_probe() {
    local socket_path=$1 output=$2 label=$3 round=$4 expected_active=$5
    local started ended latency active
    started=$(date +%s%3N)
    snapshot_to "$socket_path" "$output" || fail "$label control snapshot failed under File load"
    ended=$(date +%s%3N)
    latency=$((ended - started))
    jq -e '.data.identity.node_id != null and (.data.networks | type == "array")' "$output" >/dev/null \
        || fail "$label control snapshot omitted basic node/network state under File load"
    active=$(jq -r '.data.file_capability.active_transfers // empty' "$output")
    if [ -n "$expected_active" ]; then
        [ "$active" = "$expected_active" ] \
            || fail "$label active transfer count changed during control probe ($expected_active -> ${active:-missing})"
    fi
    [ "$latency" -le 2000 ] || fail "$label control snapshot exceeded 2s under File load: ${latency}ms"
    printf '%s\t%s\t%s\t%s\n' "$round" "$label" "$latency" "$active" >>"$WORK/control-probes.tsv"
}

wait_for_journal_entry() {
    local journal=$1 transfer_id=$2 attempts=${3:-60}
    local i
    for ((i = 0; i < attempts; i++)); do
        if [ -f "$journal" ] && jq -e --arg t "$transfer_id" '.transfers | any(.transfer_id == $t)' "$journal" >/dev/null 2>&1; then
            return 0
        fi
        sleep 1
    done
    return 1
}

wait_for_journal_absent() {
    local journal=$1 attempts=${2:-60}
    local i
    for ((i = 0; i < attempts; i++)); do
        if [ ! -e "$journal" ] || jq -e '.transfers | length == 0' "$journal" >/dev/null 2>&1; then
            return 0
        fi
        sleep 1
    done
    return 1
}

wait_for_log_error() {
    local log=$1 transfer_id=$2 expected_code=$3 attempts=${4:-60}
    local i
    for ((i = 0; i < attempts; i++)); do
        if grep -F "$transfer_id" "$log" 2>/dev/null | grep -F "传输已终止" | grep -F "$expected_code" >/dev/null 2>&1; then
            return 0
        fi
        sleep 1
    done
    return 1
}

wait_for_target_hash() {
    local target=$1 expected_hash=$2 attempts=${3:-120}
    local i actual
    for ((i = 0; i < attempts; i++)); do
        if [ -f "$target" ]; then
            actual=$("$WORK/blake3sum" "$target")
            [ "$actual" = "$expected_hash" ] && return 0
        fi
        sleep 1
    done
    if [ -f "$target" ]; then
        info "target hash mismatch: expected=$expected_hash actual=$("$WORK/blake3sum" "$target")"
    fi
    return 1
}

wait_for_completion_log() {
    local log=$1 transfer_id=$2 attempts=${3:-120}
    local i
    for ((i = 0; i < attempts; i++)); do
        if grep -F "$transfer_id" "$log" 2>/dev/null | grep -F "传输已完成并校验" >/dev/null 2>&1; then
            return 0
        fi
        sleep 1
    done
    return 1
}

wait_for_path_absent() {
    local path=$1 attempts=${2:-60}
    local i
    for ((i = 0; i < attempts; i++)); do
        [ ! -e "$path" ] && return 0
        sleep 1
    done
    return 1
}

wait_for_staging_absent() {
    local directory=$1 attempts=${2:-60}
    local i
    for ((i = 0; i < attempts; i++)); do
        if ! find "$directory" -maxdepth 1 -type f -name '.fkfile-download-*' -print -quit 2>/dev/null | grep -q .; then
            return 0
        fi
        sleep 1
    done
    return 1
}

wait_for_upload_staging() {
    local size=$1 attempts=${2:-60}
    local i
    for ((i = 0; i < attempts; i++)); do
        STAGING_PATH=$(find "$SHARE_ROOT/.fuckipv6-tmp" -maxdepth 1 -type f -name 'up-*' -print -quit 2>/dev/null || true)
        if [ -n "$STAGING_PATH" ] && [ -f "$STAGING_PATH" ]; then
            STAGING_BYTES=$(stat -c %s "$STAGING_PATH")
            if [ "$STAGING_BYTES" -gt 0 ] && [ "$STAGING_BYTES" -lt "$size" ]; then
                printf '%s\n' "$STAGING_PATH" >>"$WORK/private-staging-paths.txt"
                return 0
            fi
        fi
        sleep 1
    done
    return 1
}

wait_for_download_staging() {
    local directory=$1 attempts=${2:-60}
    local i
    # Small downloads can become visible and complete between one-second
    # polling ticks. Keep the overall timeout in seconds while sampling more
    # frequently so the pause/cancel/restart checks reliably observe staging.
    for ((i = 0; i < attempts * 10; i++)); do
        STAGING_PATH=$(find "$directory" -maxdepth 1 -type f -name '.fkfile-download-*' -print -quit 2>/dev/null || true)
        if [ -n "$STAGING_PATH" ] && [ -f "$STAGING_PATH" ] && [ "$(stat -c %s "$STAGING_PATH")" -gt 0 ]; then
            printf '%s\n' "$STAGING_PATH" >>"$WORK/private-staging-paths.txt"
            return 0
        fi
        sleep 0.1
    done
    return 1
}

wait_for_upload_staging_absent() {
    local attempts=${1:-60}
    local i
    for ((i = 0; i < attempts; i++)); do
        if ! find "$SHARE_ROOT/.fuckipv6-tmp" -maxdepth 1 -type f -name 'up-*' -print -quit 2>/dev/null | grep -q .; then
            return 0
        fi
        sleep 1
    done
    return 1
}

make_id() {
    local prefix=$1 seed=$2 suffix
    suffix="$(printf '%s' "$seed" | openssl dgst -sha256 -binary | base64 | tr '+/' '-_' | tr -d '=\n')"
    [ "${#suffix}" -eq 43 ] || fail "generated ID suffix is not 43 characters"
    printf '%s_%s\n' "$prefix" "$suffix"
}

new_transfer_ids() {
    local seed=$1 transfer_id operation_id
    transfer_id=$(make_id transfer "$seed-transfer")
    operation_id=$(make_id op "$seed-operation")
    [[ "$transfer_id" =~ ^transfer_[A-Za-z0-9_-]{43}$ ]] || fail "invalid transfer ID"
    [[ "$operation_id" =~ ^op_[A-Za-z0-9_-]{43}$ ]] || fail "invalid operation ID"
    printf '%s\n%s\n' "$transfer_id" "$operation_id"
}

start_transfer() {
    local socket_path=$1 label=$2 peer=$3 direction=$4 share_id=$5 path=$6 local_path=$7 chunk_size=$8
    local ids transfer_id operation_id command response
    ids="$(new_transfer_ids "$label")" || fail "failed to generate transfer IDs for $label"
    mapfile -t ids <<<"$ids"
    [ "${#ids[@]}" -eq 2 ] || fail "invalid transfer ID output for $label"
    transfer_id=${ids[0]}
    operation_id=${ids[1]}
    command=$(jq -cn --arg network "$NETWORK_ID" --arg peer "$peer" --arg direction "$direction" --arg share "$share_id" \
        --arg transfer "$transfer_id" --arg operation "$operation_id" --arg path "$path" \
        --arg local "$local_path" --argjson chunk "$chunk_size" '
        {request_id:1,command:{type:"file_transfer",data:{action:"start",data:{
          peer:$peer,network_id:$network,direction:$direction,share_id:$share,transfer_id:$transfer,
          operation_id:$operation,path:$path,local_path:$local,chunk_size:$chunk
        }}}}')
    response="$WORK/$label-start.json"
    ipc_local_command "$socket_path" "$command" >"$response"
    jq -e --arg t "$transfer_id" --arg o "$operation_id" \
        '.type == "file_transfer_started" and .data.transfer_id == $t and .data.operation_id == $o' \
        "$response" >/dev/null || fail "$label did not return a matching transfer handle"
    STARTED_TRANSFER_ID=$transfer_id
    STARTED_OPERATION_ID=$operation_id
}

file_control() {
    local socket_path=$1 action=$2 transfer_id=$3 operation_id=$4 label=$5
    local command response
    command=$(jq -cn --arg action "$action" --arg transfer "$transfer_id" --arg operation "$operation_id" '
        {request_id:1,command:{type:"file_transfer",data:{action:$action,data:{
          transfer_id:$transfer,operation_id:$operation
        }}}}')
    response="$WORK/$label-$action.json"
    ipc_local_command "$socket_path" "$command" >"$response"
    jq -e '.type == "file_transfer_accepted"' "$response" >/dev/null \
        || fail "$label $action was not accepted"
}

write_pattern_file() {
    local path=$1 size=$2 seed=$3
    python3 - "$path" "$size" "$seed" <<'PY'
import pathlib
import sys

path = pathlib.Path(sys.argv[1])
size = int(sys.argv[2])
seed = sys.argv[3].encode()
path.parent.mkdir(parents=True, exist_ok=True)
chunk = bytearray()
value = 0
for byte in seed:
    value = (value * 131 + byte) & 0xFFFFFFFF
for index in range(size):
    value = (value * 1103515245 + 12345 + index) & 0xFFFFFFFF
    chunk.append((value >> 16) & 0xFF)
path.write_bytes(bytes(chunk))
PY
}

install_owner_relay_policy() {
    local current="$WORK/policy-current.json"
    local candidate="$WORK/policy-relay-candidate.json"
    local applied="$WORK/policy-relay-applied.json"
    local expected_revision owner_rule_id network_rule_id issued_at expires_at command

    ipc_command "$HOST_SOCK" '{"command":"get_policy"}' >"$current"
    jq -e '.status == "policy"' "$current" >/dev/null \
        || fail "owner policy transaction could not be read"
    expected_revision=$(jq -r '.data.revision // empty' "$current")
    [ -n "$expected_revision" ] || fail "owner policy transaction did not report a revision"

    # SecurityRuleId is a 32-byte URL-safe digest. Reusing the canonical
    # NetworkId/NodeId digest suffix keeps these deterministic fixtures valid
    # without introducing a second identifier implementation into the script.
    owner_rule_id="srule_${HOST_NODE_ID#node_}"
    network_rule_id="srule_${NETWORK_ID#net_}"
    issued_at=$(($(date +%s%3N) - 60000))
    expires_at=$(($(date +%s%3N) + 86400000))

    jq -n \
        --arg network "$NETWORK_ID" \
        --arg owner "$HOST_NODE_ID" \
        --arg owner_rule "$owner_rule_id" \
        --arg network_rule "$network_rule_id" \
        --argjson issued "$issued_at" \
        --argjson expires "$expires_at" \
        --slurpfile current "$current" '
        ($current[0].data.snapshot) as $snapshot |
        {
          local_policy:{
            node_id:$owner,
            revision:1,
            issued_at_unix_ms:$issued,
            expires_at_unix_ms:$expires,
            network_scope:{kind:"only",network_ids:[$network]},
            contribution_vetoes:[],
            relay_limits:{},
            rules:[{
              rule_id:$owner_rule,
              effect:"allow",
              subjects:[{kind:"role",value:"owner"}],
              resources:["relay"],
              actions:["use"],
              conditions:[{kind:"membership_active"}]
            }]
          },
          network_acls:(
            [$snapshot.network_acls[]? | select(.network_id != $network)] + [{
              network_id:$network,
              revision:1,
              issued_at_unix_ms:$issued,
              expires_at_unix_ms:$expires,
              rules:[{
                rule_id:$network_rule,
                effect:"allow",
                subjects:[{kind:"any_member"}],
                resources:["relay"],
                actions:["use"],
                conditions:[{kind:"membership_active"}]
              }],
              grants:[],
              relay_limits:{}
            }]
          )
        }' >"$candidate"

    command=$(jq -cn \
        --argjson expected "$expected_revision" \
        --arg network "$NETWORK_ID" \
        --slurpfile candidate "$candidate" '
        {command:"apply_policy",payload:{
          expected_revision:$expected,
          candidate:$candidate[0],
          network_ids:[$network],
          includes_local_policy:true,
          confirm_privilege_expansion:true
        }}')
    ipc_command "$HOST_SOCK" "$command" >"$applied"
    jq -e --arg network "$NETWORK_ID" --arg owner "$HOST_NODE_ID" '
      .status == "policy_applied"
      and .data.previous_revision >= 0
      and .data.snapshot.local_policy.node_id == $owner
      and ([.data.snapshot.local_policy.rules[]
            | select(.resources | index("relay"))
            | select(.actions | index("use"))] | length == 1)
      and ([.data.snapshot.network_acls[]
            | select(.network_id == $network)
            | .rules[]
            | select(.resources | index("relay"))
            | select(.actions | index("use"))] | length == 1)
    ' "$applied" >/dev/null || {
        jq . "$applied" >&2 || true
        fail "owner Relay policy transaction was not applied"
    }
    info "owner Relay local policy and network ACL are durably authorized"
}

profile_update_command() {
    local capabilities_json=$1 network_json=$2
    jq -cn --arg network "$NETWORK" --argjson capabilities "$capabilities_json" \
        --slurpfile record "$network_json" '
        ($record[0]) as $r |
        {
          command:"update_network_profile",
          payload:{
            network_name:$network,
            expected_revision:$r.revision,
            update:{
              purpose:$r.purpose,
              icon:$r.icon,
              language:$r.language,
              region:$r.region,
              tags:$r.tags,
              capability_declarations:$capabilities,
              visibility:$r.visibility,
              join_mode:$r.join_mode,
              exit_contribution:$r.exit_contribution,
              exit_contribution_scope:$r.exit_contribution_scope,
              exit_contribution_mode:$r.exit_contribution_mode,
              exit_selection_policy:$r.exit_selection_policy
            }
          }
        }'
}

configure_share() {
    local configuration_file=$1 label=$2 response="$WORK/$label.json" command
    command=$(jq -cn --arg root "$SHARE_ROOT" --slurpfile configuration "$configuration_file" '
        {request_id:1,command:{type:"configure_file_share",data:{
          root_reference:$root,configuration:$configuration[0]
        }}}')
    ipc_local_command "$HOST_SOCK" "$command" >"$response"
    jq -e '.type == "file_share_configured" and .data.state == "enabled"' "$response" >/dev/null \
        || fail "$label did not enable the File capability"
}

make_share_configuration() {
    local output=$1 rule_mode=$2 bandwidth=$3
    if [ "$rule_mode" = full ]; then
        jq -n --arg network "$NETWORK_ID" --argjson bandwidth "$bandwidth" '
        {
          network_id:$network,
          display_name:"W15 files",
          mode:"read_write",
          network_acl:{rules:[{
            id:"members-rw",
            subjects:[{kind:"any_member"}],
            path_prefix:"",
            actions:["list","read_metadata","download","upload"],
            effect:"allow"
          }]},
          local_acl:{rules:[{
            id:"members-rw",
            subjects:[{kind:"any_member"}],
            path_prefix:"",
            actions:["list","read_metadata","download","upload"],
            effect:"allow"
          }]},
          quota:{
            total_bytes:536870912,
            per_member_bytes:268435456,
            max_file_bytes:134217728,
            max_entries:10000,
            reserve_bytes:16777216,
            max_concurrent_transfers:4,
            max_concurrent_transfers_per_member:2,
            max_concurrent_transfers_per_directory:2,
            max_bandwidth_kbps:$bandwidth
          },
          symlink_policy:"reject",
          concurrent_name_policy:"reject",
          delete_policy:{require_version_match:true,allow_recursive_directory:false}
        }' >"$output"
    elif [ "$rule_mode" = revoked ]; then
        jq -n --arg network "$NETWORK_ID" --argjson bandwidth "$bandwidth" '
        {
          network_id:$network,
          display_name:"W15 files",
          mode:"read_write",
          network_acl:{rules:[]},
          local_acl:{rules:[]},
          quota:{
            total_bytes:536870912,
            per_member_bytes:268435456,
            max_file_bytes:134217728,
            max_entries:10000,
            reserve_bytes:16777216,
            max_concurrent_transfers:4,
            max_concurrent_transfers_per_member:2,
            max_concurrent_transfers_per_directory:2,
            max_bandwidth_kbps:$bandwidth
          },
          symlink_policy:"reject",
          concurrent_name_policy:"reject",
          delete_policy:{require_version_match:true,allow_recursive_directory:false}
        }' >"$output"
    elif [ "$rule_mode" = readonly ]; then
        jq -n --arg network "$NETWORK_ID" --argjson bandwidth "$bandwidth" '
        {
          network_id:$network,
          display_name:"W15 files",
          mode:"read_only",
          network_acl:{rules:[{
            id:"members-readonly",
            subjects:[{kind:"any_member"}],
            path_prefix:"",
            actions:["list","read_metadata","download"],
            effect:"allow"
          }]},
          local_acl:{rules:[{
            id:"members-readonly",
            subjects:[{kind:"any_member"}],
            path_prefix:"",
            actions:["list","read_metadata","download"],
            effect:"allow"
          }]},
          quota:{
            total_bytes:536870912,
            per_member_bytes:268435456,
            max_file_bytes:134217728,
            max_entries:10000,
            reserve_bytes:16777216,
            max_concurrent_transfers:4,
            max_concurrent_transfers_per_member:2,
            max_concurrent_transfers_per_directory:2,
            max_bandwidth_kbps:$bandwidth
          },
          symlink_policy:"reject",
          concurrent_name_policy:"reject",
          delete_policy:{require_version_match:true,allow_recursive_directory:false}
        }' >"$output"
    else
        fail "unknown share rule mode: $rule_mode"
    fi
}

record_runtime_baseline() {
    local label=$1 pid=$2
    kill -0 "$pid" 2>/dev/null || fail "$label process is unavailable for resource baseline"
    {
        printf 'fd=%s\n' "$(find "/proc/$pid/fd" -mindepth 1 -maxdepth 1 2>/dev/null | wc -l)"
        printf 'threads=%s\n' "$(find "/proc/$pid/task" -mindepth 1 -maxdepth 1 2>/dev/null | wc -l)"
    } >"$WORK/resource-$label-before.txt"
}

assert_runtime_stable() {
    local label=$1 pid=$2 before_fd before_threads after_fd after_threads
    [ -n "$pid" ] && kill -0 "$pid" 2>/dev/null || fail "$label process is not running at resource check time"
    before_fd=$(sed -n 's/^fd=//p' "$WORK/resource-$label-before.txt")
    before_threads=$(sed -n 's/^threads=//p' "$WORK/resource-$label-before.txt")
    after_fd=$(find "/proc/$pid/fd" -mindepth 1 -maxdepth 1 2>/dev/null | wc -l)
    after_threads=$(find "/proc/$pid/task" -mindepth 1 -maxdepth 1 2>/dev/null | wc -l)
    printf 'fd=%s\nthreads=%s\n' "$after_fd" "$after_threads" >"$WORK/resource-$label-after.txt"
    [ "$after_fd" -le $((before_fd + 8)) ] || fail "$label retained too many file descriptors ($before_fd -> $after_fd)"
    [ "$after_threads" -le $((before_threads + 8)) ] || fail "$label retained too many threads ($before_threads -> $after_threads)"
}

record_stress_baseline() {
    local label=$1 pid=$2 rss
    kill -0 "$pid" 2>/dev/null || fail "$label process is unavailable for File stress baseline"
    rss=$(awk '/^VmRSS:/ {print $2}' "/proc/$pid/status")
    {
        printf 'fd=%s\n' "$(find "/proc/$pid/fd" -mindepth 1 -maxdepth 1 2>/dev/null | wc -l)"
        printf 'threads=%s\n' "$(find "/proc/$pid/task" -mindepth 1 -maxdepth 1 2>/dev/null | wc -l)"
        printf 'rss_kib=%s\n' "${rss:-0}"
    } >"$WORK/resource-stress-$label-before.txt"
}

assert_stress_runtime_stable() {
    local label=$1 pid=$2 before_fd before_threads before_rss after_fd after_threads after_rss
    [ -n "$pid" ] && kill -0 "$pid" 2>/dev/null || fail "$label process is unavailable after File stress"
    before_fd=$(sed -n 's/^fd=//p' "$WORK/resource-stress-$label-before.txt")
    before_threads=$(sed -n 's/^threads=//p' "$WORK/resource-stress-$label-before.txt")
    before_rss=$(sed -n 's/^rss_kib=//p' "$WORK/resource-stress-$label-before.txt")
    after_fd=$(find "/proc/$pid/fd" -mindepth 1 -maxdepth 1 2>/dev/null | wc -l)
    after_threads=$(find "/proc/$pid/task" -mindepth 1 -maxdepth 1 2>/dev/null | wc -l)
    after_rss=$(awk '/^VmRSS:/ {print $2}' "/proc/$pid/status")
    {
        printf 'fd=%s\n' "$after_fd"
        printf 'threads=%s\n' "$after_threads"
        printf 'rss_kib=%s\n' "${after_rss:-0}"
    } >"$WORK/resource-stress-$label-after.txt"
    [ "$after_fd" -le $((before_fd + 8)) ] \
        || fail "$label retained too many file descriptors after File stress ($before_fd -> $after_fd)"
    [ "$after_threads" -le $((before_threads + 8)) ] \
        || fail "$label retained too many threads after File stress ($before_threads -> $after_threads)"
    [ "${after_rss:-0}" -le $((before_rss + 16384)) ] \
        || fail "$label retained too much RSS after File stress ($before_rss -> $after_rss KiB)"
}

assert_pid_gone() {
    local pid=$1 label=$2
    [ -z "$pid" ] || ! kill -0 "$pid" 2>/dev/null || fail "$label process $pid is still running"
}

assert_port_free() {
    local port=$1
    ! ss -H -lntu | awk '{print $5}' | grep -Eq "[:.]$port$" || fail "port $port remains in use"
}

assert_namespace_absent() {
    local ns=$1
    ! ip netns list | grep -Eq "^$ns([[:space:]]|$)" || fail "network namespace $ns remains"
}

wait_for_network_revision() {
    local minimum=$1 attempts=${2:-60}
    local i revision
    for ((i = 0; i < attempts; i++)); do
        revision=$(jq -r '.revision // 0' "$NETWORK_JSON" 2>/dev/null || printf '0')
        if [ "$revision" -ge "$minimum" ]; then
            return 0
        fi
        sleep 1
    done
    return 1
}

set_network_file_capability() {
    local expected=$1 label=$2 capabilities old_revision command
    if [ "$expected" = yes ]; then
        capabilities='["file"]'
    else
        capabilities='[]'
    fi
    old_revision=$(jq -r '.revision // 0' "$NETWORK_JSON")
    command=$(profile_update_command "$capabilities" "$NETWORK_JSON")
    ipc_command "$HOST_SOCK" "$command" >"$WORK/$label-command.json"
    wait_for_network_revision "$((old_revision + 1))" 60 \
        || fail "$label did not advance the authoritative network revision"
    # Network profiles are owner-local configuration records. Members consume
    # the signed grant through invitations and file authorization, but do not
    # persist a copy in their ordinary local-network configuration.
    wait_for_profile_file_capability "$HOST_SOCK" "$WORK/$label-host.json" "$expected" "$label host" 90
}

wait_for_terminal_log() {
    local log=$1 transfer_id=$2 code_pattern=$3 attempts=${4:-180}
    local i line
    for ((i = 0; i < attempts; i++)); do
        line=$(grep -F "$transfer_id" "$log" 2>/dev/null | grep -F "传输已终止" | grep -E "$code_pattern" | tail -1 || true)
        if [ -n "$line" ]; then
            printf '%s\n' "$line"
            return 0
        fi
        sleep 1
    done
    return 1
}

assert_no_private_path_leaks() {
    local path file failed=0
    local -a scanned=()
    mapfile -t scanned < <(
        find "$WORK" -path '*/config' -prune -o -type f \( -name '*.log' -o -name '*.json' \) -print
    )
    [ "${#scanned[@]}" -gt 0 ] || fail "path-leak scan found no observability artifacts"
    while IFS= read -r path; do
        [ -n "$path" ] || continue
        for file in "${scanned[@]}"; do
            if grep -F -q -- "$path" "$file" 2>/dev/null; then
                printf '[FAIL] private path leaked into %s: %s\n' "$file" "$path" >&2
                failed=1
            fi
        done
    done < <(
        printf '%s\n' "$SHARE_ROOT" "$NODE1_UPLOAD_SOURCE" "$NODE1_DOWNLOAD_TARGET" \
            "$NODE1_RESTART_TARGET" "$NODE2_DOWNLOAD_TARGET" "$NODE2_RESTART_TARGET" \
            "$NODE1_CANCEL_UPLOAD_TARGET" "$NODE1_CANCEL_UPLOAD_SOURCE" \
            "$NODE2_CANCEL_DOWNLOAD_TARGET" "$NODE1_ACL_UPLOAD_SOURCE" \
            "$NODE1_NETWORK_REVOKE_SOURCE"
        cat "$WORK/private-staging-paths.txt" 2>/dev/null || true
    )
    [ "$failed" -eq 0 ] || fail "private absolute paths appeared in IPC/snapshot/node/owner observability"
    printf 'scanned_files=%s\n' "${#scanned[@]}" >"$WORK/path-leak-scan.txt"
}

assert_share_staging_empty() {
    if find "$SHARE_ROOT/.fuckipv6-tmp" -maxdepth 1 -type f -print -quit 2>/dev/null | grep -q .; then
        fail "owner upload staging still contains files"
    fi
}

build_blake3_helper() {
    local blake3_rlib
    blake3_rlib=$(find "$ROOT/target/release/deps" -maxdepth 1 -name 'libblake3-*.rlib' -print -quit)
    [ -n "$blake3_rlib" ] || fail "missing release blake3 rlib"
    cat >"$WORK/blake3sum.rs" <<'RS'
use std::env;
use std::fs::File;
use std::io::{self, Read};

fn main() -> io::Result<()> {
    let path = env::args().nth(1).expect("path");
    let mut file = File::open(path)?;
    let mut hasher = blake3::Hasher::new();
    let mut buffer = vec![0_u8; 1024 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    println!("{}", hasher.finalize().to_hex());
    Ok(())
}
RS
    rustc --edition=2021 -L dependency="$ROOT/target/release/deps" \
        --extern blake3="$blake3_rlib" "$WORK/blake3sum.rs" -o "$WORK/blake3sum"
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
        assert_port_free "$port"
    done

    mkdir -p "$WORK"/{host,node1,node2}/{home,run,config}
    mkdir -p "$WORK"/{host,node1,node2}/xdg
    SHARE_ROOT="$WORK/host/share"
    NODE1_WORK="$WORK/node1/work"
    NODE2_WORK="$WORK/node2/work"
    NODE1_DOWNLOADS="$NODE1_WORK/downloads"
    NODE2_DOWNLOADS="$NODE2_WORK/downloads"
    mkdir -p "$SHARE_ROOT" "$NODE1_DOWNLOADS" "$NODE2_DOWNLOADS"
    : >"$WORK/private-staging-paths.txt"
    build_blake3_helper
    info "workdir=$WORK"
    info "binary=$(git -C "$ROOT" rev-parse HEAD) sha256=$(sha256sum "$BIN" | awk '{print $1}')"
    info "topology: coordinator netns $NSH 10.79.1.1:$HOST_PORT; node1 10.79.1.2; node2 10.79.2.2"

    ip rule show >"$WORK/host-ip-rule.before"
    ip -6 rule show >"$WORK/host-ip6-rule.before"
    ip route show default >"$WORK/host-default-route.before"

    info "creating isolated routed veth underlay"
    local i ns vh vn
    ip netns add "$NSH"
    ip netns exec "$NSH" ip link set lo up
    ip netns exec "$NSH" sysctl -qw net.ipv4.ip_forward=1
    for i in 1 2; do
        if [ "$i" -eq 1 ]; then ns=$NS1; vh=$VH1; vn=$VN1; else ns=$NS2; vh=$VH2; vn=$VN2; fi
        ip netns add "$ns"
        ip link add "$vh" type veth peer name "$vn"
        ip link set "$vn" netns "$ns"
        ip link set "$vh" netns "$NSH"
        ip netns exec "$NSH" ip addr add "10.79.$i.1/24" dev "$vh"
        ip netns exec "$NSH" ip link set "$vh" up
        ip netns exec "$ns" ip link set lo up
        ip netns exec "$ns" ip addr add "10.79.$i.2/24" dev "$vn"
        ip netns exec "$ns" ip link set "$vn" up
        ip netns exec "$ns" ip route add default via "10.79.$i.1"
    done

    local common_clean_env=(env -u DISPLAY -u WAYLAND_DISPLAY -u XDG_SESSION_TYPE -u XDG_CURRENT_DESKTOP -u DESKTOP_SESSION)
    local host_env=("${common_clean_env[@]}" HOME="$WORK/host/home" XDG_CONFIG_HOME="$WORK/host/xdg"
        FUCKIPV6_RUNTIME_DIR="$WORK/host/run" FUCKIPV6_CONFIG_DIR="$WORK/host/config")
    local node1_env=("${common_clean_env[@]}" HOME="$WORK/node1/home" XDG_CONFIG_HOME="$WORK/node1/xdg"
        FUCKIPV6_RUNTIME_DIR="$WORK/node1/run" FUCKIPV6_CONFIG_DIR="$WORK/node1/config")
    local node2_env=("${common_clean_env[@]}" HOME="$WORK/node2/home" XDG_CONFIG_HOME="$WORK/node2/xdg"
        FUCKIPV6_RUNTIME_DIR="$WORK/node2/run" FUCKIPV6_CONFIG_DIR="$WORK/node2/config")
    local headless_state="$WORK/no-desktop"
    local headless_exec="$headless_state/exec-headless"
    mkdir -p "$headless_state"
    cat >"$headless_exec" <<'SH'
#!/usr/bin/env bash
set -Eeuo pipefail
state=$1
shift
runtime_state=$(mktemp -d "$state/runtime.XXXXXX")
mkdir -p "$runtime_state/run/user" "$runtime_state/x11" "$runtime_state/etc"
cp /etc/resolv.conf "$runtime_state/etc/resolv.conf"
cp /etc/nsswitch.conf "$runtime_state/etc/nsswitch.conf"
exec unshare --mount --propagation private bash -c '
    set -Eeuo pipefail
    mount --bind "$1" /run
    mount --bind "$2" /tmp/.X11-unix
    mount --bind "$3" /etc/resolv.conf
    mount --bind "$4" /etc/nsswitch.conf
    shift 4
    exec "$@"
' bash "$runtime_state/run" "$runtime_state/x11" \
    "$runtime_state/etc/resolv.conf" "$runtime_state/etc/nsswitch.conf" "$@"
SH
    chmod +x "$headless_exec"

    info "starting Coordinator with its owner membership active"
    ip netns exec "$NSH" "$headless_exec" "$headless_state" "${host_env[@]}" RUST_LOG=debug "$BIN" host \
        --name "$NETWORK" --port "$HOST_PORT" --relay-port "$RELAY_PORT" \
        --external-host 10.79.1.1 --headless --webui-port "$WEBUI_HOST" \
        >"$WORK/host/host.log" 2>&1 &
    HPID=$!
    HOST_SOCK="$WORK/host/run/node.sock"
    wait_for_socket "$HOST_SOCK" 90 || { tail -200 "$WORK/host/host.log" >&2 || true; fail "host node.sock did not appear"; }
    NETWORK_JSON="$WORK/host/config/$NETWORK/network.json"
    wait_for_file "$NETWORK_JSON" 60 || { tail -200 "$WORK/host/host.log" >&2 || true; fail "authoritative network record did not appear"; }
    NETWORK_ID=$(jq -r '.network_id // empty' "$NETWORK_JSON")
    HOST_NODE_ID=$(jq -r '.owner // empty' "$NETWORK_JSON")
    [ -n "$NETWORK_ID" ] && [ -n "$HOST_NODE_ID" ] || fail "network record lacks network_id or owner"
    [ "$(jq -r '.join_mode' "$NETWORK_JSON")" = invite_only ] || fail "acceptance network is not InviteOnly"
    install_owner_relay_policy
    wait_for_network_online "$HOST_SOCK" "$WORK/host-online.json" 90 || {
        tail -240 "$WORK/host/host.log" >&2 || true
        fail "Coordinator owner network did not become online"
    }
    HOST_NODE_PID=$(wait_for_ready_node_pid "$WORK/host/host.log" 60) \
        || fail "Coordinator node readiness PID was not reported"
    info "network_id=$NETWORK_ID owner=$HOST_NODE_ID join_mode=invite_only"

    # Invitation capability policy is checked against the current signed network
    # profile at issuance time, so File must be declared before accepting members.
    info "declaring the signed network File capability before invitation issuance"
    set_network_file_capability yes grant-file

    info "starting two empty member nodes"
    ip netns exec "$NS1" "$headless_exec" "$headless_state" "${node1_env[@]}" RUST_LOG=debug "$BIN" --headless --webui-port "$WEBUI1" \
        >"$WORK/node1/node.log" 2>&1 &
    N1PID=$!
    ip netns exec "$NS2" "$headless_exec" "$headless_state" "${node2_env[@]}" RUST_LOG=debug "$BIN" --headless --webui-port "$WEBUI2" \
        >"$WORK/node2/node.log" 2>&1 &
    N2PID=$!
    NODE1_SOCK="$WORK/node1/run/node.sock"
    NODE2_SOCK="$WORK/node2/run/node.sock"
    wait_for_socket "$NODE1_SOCK" 60 || { tail -160 "$WORK/node1/node.log" >&2 || true; fail "node1 socket did not appear"; }
    wait_for_socket "$NODE2_SOCK" 60 || { tail -160 "$WORK/node2/node.log" >&2 || true; fail "node2 socket did not appear"; }
    NODE1_NODE_PID=$(wait_for_ready_node_pid "$WORK/node1/node.log" 60) \
        || fail "node1 readiness PID was not reported"
    NODE2_NODE_PID=$(wait_for_ready_node_pid "$WORK/node2/node.log" 60) \
        || fail "node2 readiness PID was not reported"
    snapshot_to "$NODE1_SOCK" "$WORK/node1-identity.json"
    snapshot_to "$NODE2_SOCK" "$WORK/node2-identity.json"
    NODE1_ID=$(jq -r '.data.identity.node_id // empty' "$WORK/node1-identity.json")
    NODE2_ID=$(jq -r '.data.identity.node_id // empty' "$WORK/node2-identity.json")
    [ -n "$NODE1_ID" ] && [ -n "$NODE2_ID" ] && [ "$NODE1_ID" != "$NODE2_ID" ] \
        || fail "failed to obtain two distinct node identities"

    info "creating and redeeming the formal max_uses=2 invitation"
    local create_command preview_command redeem_command coordinator_url now_ms revision preview_digest terms_hash
    create_command=$(jq -cn --arg network "$NETWORK_ID" \
        '{command:"create_invitation",payload:{network_id:$network,role:"member",capabilities:["file"],max_uses:2,expires_in_ms:3600000}}')
    ipc_command "$HOST_SOCK" "$create_command" >"$WORK/invitation-created.json"
    INVITE_CODE=$(jq -r '.data.code // empty' "$WORK/invitation-created.json")
    INVITATION_ID=$(jq -r '.data.status.invitation.invitation_id // empty' "$WORK/invitation-created.json")
    [ -n "$INVITE_CODE" ] && [ -n "$INVITATION_ID" ] || fail "invitation creation returned no code/id"
    jq -e '.data.status.invitation.capabilities == ["file"]' "$WORK/invitation-created.json" >/dev/null \
        || fail "formal invitation was not scoped to File capability"
    coordinator_url="ws://10.79.1.1:$HOST_PORT"

    for ns in 1 2; do
        if [ "$ns" -eq 1 ]; then sock=$NODE1_SOCK; node_id=$NODE1_ID; label=node1; else sock=$NODE2_SOCK; node_id=$NODE2_ID; label=node2; fi
        preview_command=$(jq -cn --arg network "$NETWORK_ID" --arg url "$coordinator_url" --arg code "$INVITE_CODE" \
            '{command:"preview_invitation",payload:{network_id:$network,coordinator_url:$url,code:$code}}')
        ipc_command "$sock" "$preview_command" >"$WORK/$label-preview.json"
        revision=$(jq -r '.data.invitation_revision' "$WORK/$label-preview.json")
        preview_digest=$(jq -r '.data.preview_digest' "$WORK/$label-preview.json")
        terms_hash=$(jq -r '.data.terms_hash' "$WORK/$label-preview.json")
        jq -e '.data.can_accept == true and .data.capabilities == ["file"]' "$WORK/$label-preview.json" >/dev/null \
            || fail "$label invitation preview is not an acceptable File-scoped invitation"
        now_ms=$(date +%s%3N)
        redeem_command=$(jq -cn --arg url "$coordinator_url" --arg network "$NETWORK_ID" --arg code "$INVITE_CODE" \
            --arg invitation "$INVITATION_ID" --arg digest "$preview_digest" --arg terms "$terms_hash" \
            --argjson revision "$revision" --argjson now "$now_ms" \
            '{command:"redeem_invitation",payload:{coordinator_url:$url,request:{network_id:$network,code:$code,invitation_id:$invitation,expected_invitation_revision:$revision,preview_digest:$digest,terms_hash:$terms,acknowledged_at_unix_ms:$now,exit_contribution_acknowledged:true}}}')
        ipc_command "$sock" "$redeem_command" >"$WORK/$label-redeem.json"
        jq -e --arg id "$node_id" \
            '.data.node_id == $id and .data.capabilities == ["file"] and .data.grants_permanent_membership == false' \
            "$WORK/$label-redeem.json" >/dev/null \
            || fail "$label redemption did not grant ordinary membership with File capability"
    done

    wait_for_network_online "$NODE1_SOCK" "$WORK/node1-online.json" 120 || {
        tail -240 "$WORK/node1/node.log" >&2 || true
        fail "node1 network did not become online"
    }
    wait_for_network_online "$NODE2_SOCK" "$WORK/node2-online.json" 120 || {
        tail -240 "$WORK/node2/node.log" >&2 || true
        fail "node2 network did not become online"
    }
    local roster_ok=0
    for ((i = 0; i < 90; i++)); do
        if jq -e --arg n1 "$NODE1_ID" --arg n2 "$NODE2_ID" \
            '(.members | index($n1)) != null and (.members | index($n2)) != null' "$NETWORK_JSON" >/dev/null 2>&1; then
            roster_ok=1
            break
        fi
        sleep 1
    done
    [ "$roster_ok" -eq 1 ] || { jq '{owner,administrators,members,revision}' "$NETWORK_JSON" >&2; fail "authoritative roster did not admit both invitees"; }
    IP1=$(jq -r --arg n "$NETWORK" '.data.networks[]? | select(.name==$n) | .assigned_ipv6 // empty' "$WORK/node1-online.json" | head -1)
    IP2=$(jq -r --arg n "$NETWORK" '.data.networks[]? | select(.name==$n) | .assigned_ipv6 // empty' "$WORK/node2-online.json" | head -1)
    [ -n "$IP1" ] && [ -n "$IP2" ] && [ "$IP1" != "$IP2" ] || fail "nodes did not receive distinct virtual IPv6 addresses"
    info "virtual addresses: node1=$IP1 node2=$IP2"

    wait_for_peer "$NODE1_SOCK" "$WORK/node1-p2p.json" "$IP2" 120 || {
        jq '.data.peers' "$WORK/node1-online.json" >&2 || true
        fail "node1 did not report node2 as active/healthy P2P peer"
    }
    wait_for_peer "$NODE2_SOCK" "$WORK/node2-p2p.json" "$IP1" 120 || {
        jq '.data.peers' "$WORK/node2-online.json" >&2 || true
        fail "node2 did not report node1 as active/healthy P2P peer"
    }
    record_runtime_baseline host "$HOST_NODE_PID"
    record_runtime_baseline node1 "$NODE1_NODE_PID"
    record_runtime_baseline node2 "$NODE2_NODE_PID"

    info "validating member-authorized File scope from the joined-node directory"
    member_directory_command=$(jq -cn --arg network "$NETWORK_ID" \
        '{command:"list_network_members",payload:{network_id:$network}}')
    ipc_command "$NODE1_SOCK" "$member_directory_command" >"$WORK/node1-member-directory.json"
    jq -e --arg owner "$HOST_NODE_ID" --arg node1 "$NODE1_ID" '
        .status == "network_members"
        and (.data.capability_declarations | index("file")) != null
        and ([.data.members[] | select(.node_id == $owner)] | length == 1)
        and ([.data.members[] | select(.node_id == $owner and .role == "owner")] | length == 1)
        and ([.data.members[] | select(.node_id == $node1 and .role == "member")] | length == 1)
        and ([.data.members[] | select(.node_id == $node1 and ((.capabilities // []) | index("file")) != null)] | length == 1)
    ' "$WORK/node1-member-directory.json" >/dev/null || {
        jq '{status, data:{network_revision:.data.network_revision,capability_declarations:.data.capability_declarations,viewer_role:.data.viewer_role,members:[.data.members[] | {node_id,role,capabilities,lifecycle_state,access_epoch}]}}' "$WORK/node1-member-directory.json" >&2 || true
        fail "joined-node member directory does not authorize the signed File scope"
    }

    info "owner-authoritative network File profile is current and signed; File members were admitted under it"

    NODE1_UPLOAD_SOURCE="$NODE1_WORK/node1-upload.bin"
    NODE1_CANCEL_UPLOAD_SOURCE="$NODE1_WORK/cancel-upload-source.bin"
    NODE1_ACL_UPLOAD_SOURCE="$NODE1_WORK/acl-revoke-source.bin"
    NODE1_NETWORK_REVOKE_SOURCE="$NODE1_WORK/network-revoke-source.bin"
    NODE1_DOWNLOAD_TARGET="$NODE1_DOWNLOADS/node1-download.bin"
    NODE1_RESTART_TARGET="$NODE1_WORK/outage-resume.bin"
    NODE1_CANCEL_UPLOAD_TARGET="$SHARE_ROOT/cancel-upload.bin"
    NODE2_DOWNLOAD_TARGET="$NODE2_DOWNLOADS/node2-download.bin"
    NODE2_RESTART_TARGET="$NODE2_DOWNLOADS/restart-resume.bin"
    NODE2_CANCEL_DOWNLOAD_TARGET="$NODE2_DOWNLOADS/cancel-download.bin"
    OWNER_SOURCE="$SHARE_ROOT/owner-source.bin"
    OUTAGE_UPLOAD_TARGET="$SHARE_ROOT/outage-resume.bin"
    ACL_REVOKE_TARGET="$SHARE_ROOT/acl-revoke.bin"
    NETWORK_REVOKE_TARGET="$SHARE_ROOT/network-revoke.bin"

    write_pattern_file "$OWNER_SOURCE" 8388608 owner-source
    write_pattern_file "$NODE1_UPLOAD_SOURCE" 2097152 node1-upload
    write_pattern_file "$NODE1_CANCEL_UPLOAD_SOURCE" 2097152 cancel-upload
    write_pattern_file "$NODE1_ACL_UPLOAD_SOURCE" 2097152 acl-revoke
    write_pattern_file "$NODE1_NETWORK_REVOKE_SOURCE" 2097152 network-revoke
    OWNER_HASH=$("$WORK/blake3sum" "$OWNER_SOURCE")
    NODE1_UPLOAD_HASH=$("$WORK/blake3sum" "$NODE1_UPLOAD_SOURCE")
    NODE1_CANCEL_HASH=$("$WORK/blake3sum" "$NODE1_CANCEL_UPLOAD_SOURCE")
    NODE1_ACL_HASH=$("$WORK/blake3sum" "$NODE1_ACL_UPLOAD_SOURCE")
    NODE1_NETWORK_REVOKE_HASH=$("$WORK/blake3sum" "$NODE1_NETWORK_REVOKE_SOURCE")

    info "previewing the owner share with ACL, quota and directory accounting"
    local config_full="$WORK/share-full.json" preview_command_json
    make_share_configuration "$config_full" full 8192
    preview_command_json=$(jq -cn --arg root "$SHARE_ROOT" --slurpfile configuration "$config_full" \
        '{request_id:1,command:{type:"preview_file_share",data:{root_reference:$root,configuration:$configuration[0]}}}')
    ipc_local_command "$HOST_SOCK" "$preview_command_json" >"$WORK/share-preview.json"
    jq -e --arg network "$NETWORK_ID" '
        .type == "file_share_preview"
        and .data.network_id == $network
        and .data.mode == "read_write"
        and .data.network_acl_rule_count == 1
        and .data.local_acl_rule_count == 1
        and .data.audit_enabled == true
        and .data.deny_by_default == false
        and .data.available_bytes > 0
        and .data.directory_bytes >= 1048576
        and .data.directory_entries >= 1
        and .data.symlink_policy == "reject"
        and .data.quota.max_bandwidth_kbps == 8192
    ' "$WORK/share-preview.json" >/dev/null || {
        jq . "$WORK/share-preview.json" >&2 || true
        fail "owner File preview omitted required ACL/quota/directory facts"
    }

    info "enabling the read/write owner share"
    configure_share "$config_full" enable-share
    wait_for_file_state "$HOST_SOCK" "$WORK/host-file-ready.json" enabled 60 \
        || fail "owner File capability did not become enabled"
    wait_for_file "$WORK/host/config/file_share.json" 30 || fail "persisted File share was not written"
    SHARE_ID=$(jq -r '.share.id // empty' "$WORK/host/config/file_share.json")
    [ -n "$SHARE_ID" ] || fail "persisted File share has no stable share id"
    info "share_id=$SHARE_ID"

    local transfer_id operation_id terminal_line

    info "node1 uploads a 2 MiB file and publishes it atomically"
    start_transfer "$NODE1_SOCK" success-upload "$HOST_NODE_ID" upload "$SHARE_ID" uploaded-by-node1.bin "$NODE1_UPLOAD_SOURCE" 262144
    transfer_id=$STARTED_TRANSFER_ID
    operation_id=$STARTED_OPERATION_ID
    wait_for_target_hash "$SHARE_ROOT/uploaded-by-node1.bin" "$NODE1_UPLOAD_HASH" 180 \
        || fail "owner upload target did not reach the expected BLAKE3 hash"
    wait_for_completion_log "$WORK/node1/node.log" "$transfer_id" 120 \
        || { tail -200 "$WORK/node1/node.log" >&2 || true; fail "node1 upload did not log verified completion"; }
    [ -f "$NODE1_UPLOAD_SOURCE" ] || fail "node1 upload source disappeared"
    info "upload verified: $transfer_id"

    info "node1 and node2 each download the uploaded file"
    for ns in 1 2; do
        if [ "$ns" -eq 1 ]; then sock=$NODE1_SOCK; label=node1-download; target=$NODE1_DOWNLOAD_TARGET; log=$WORK/node1/node.log; else sock=$NODE2_SOCK; label=node2-download; target=$NODE2_DOWNLOAD_TARGET; log=$WORK/node2/node.log; fi
        start_transfer "$sock" "$label" "$HOST_NODE_ID" download "$SHARE_ID" uploaded-by-node1.bin "$target" 262144
        transfer_id=$STARTED_TRANSFER_ID
        operation_id=$STARTED_OPERATION_ID
        wait_for_target_hash "$target" "$NODE1_UPLOAD_HASH" 180 || fail "$label hash mismatch"
        wait_for_completion_log "$log" "$transfer_id" 120 || fail "$label did not log verified completion"
    done
    info "bidirectional requester download verification passed on both member nodes"

    info "reconfiguring File to a low-bandwidth share for pause/outage recovery"
    local config_low="$WORK/share-low.json"
    make_share_configuration "$config_low" full 512
    configure_share "$config_low" enable-low-bandwidth
    wait_for_file_state "$HOST_SOCK" "$WORK/host-file-low.json" enabled 60 \
        || fail "low-bandwidth File reconfigure did not settle"

    info "saturating all four File slots while probing coordinator/owner/member control planes"
    local -a stress_sockets=("$NODE1_SOCK" "$NODE1_SOCK" "$NODE2_SOCK" "$NODE2_SOCK")
    local -a stress_labels=(stress-node1-a stress-node1-b stress-node2-a stress-node2-b)
    local -a stress_paths=(isolation/node1-a/file.bin isolation/node1-b/file.bin isolation/node2-a/file.bin isolation/node2-b/file.bin)
    local -a stress_transfer_ids=() stress_operation_ids=()
    local stress_index stress_source stress_target
    for stress_index in 0 1 2 3; do
        stress_source="$WORK/${stress_labels[$stress_index]}-source.bin"
        stress_target="$SHARE_ROOT/${stress_paths[$stress_index]}"
        mkdir -p "$(dirname "$stress_target")"
        write_pattern_file "$stress_source" 4194304 "${stress_labels[$stress_index]}"
    done
    record_stress_baseline host "$HOST_NODE_PID"
    record_stress_baseline node1 "$NODE1_NODE_PID"
    record_stress_baseline node2 "$NODE2_NODE_PID"
    for stress_index in 0 1 2 3; do
        stress_source="$WORK/${stress_labels[$stress_index]}-source.bin"
        start_transfer "${stress_sockets[$stress_index]}" "${stress_labels[$stress_index]}" "$HOST_NODE_ID" upload \
            "$SHARE_ID" "${stress_paths[$stress_index]}" "$stress_source" 262144
        stress_transfer_ids+=("$STARTED_TRANSFER_ID")
        stress_operation_ids+=("$STARTED_OPERATION_ID")
    done
    wait_for_file_active_transfers "$HOST_SOCK" "$WORK/host-file-saturated.json" 4 120 \
        || { tail -200 "$WORK/host/host.log" >&2 || true; fail "File did not reach the four-transfer global saturation point"; }
    : >"$WORK/control-probes.tsv"
    local -a probe_sockets=("$HOST_SOCK" "$NODE1_SOCK" "$NODE2_SOCK")
    local -a probe_labels=(coordinator-owner node1 node2)
    local -a probe_expected=(4 '' '')
    local probe_round probe_index
    for ((probe_round = 1; probe_round <= 8; probe_round++)); do
        for probe_index in 0 1 2; do
            control_snapshot_probe "${probe_sockets[$probe_index]}" "$WORK/control-${probe_round}-${probe_labels[$probe_index]}.json" \
                "${probe_labels[$probe_index]}" "$probe_round" "${probe_expected[$probe_index]}"
        done
        sleep 0.1
    done
    [ "$(jq -r '.data.file_capability.active_transfers' "$WORK/control-8-coordinator-owner.json")" = 4 ] \
        || fail "File slots did not remain saturated through the control-plane probes"
    info "control probes passed at full File saturation; cancelling all four uploads"
    for stress_index in 0 1 2 3; do
        file_control "${stress_sockets[$stress_index]}" cancel "${stress_transfer_ids[$stress_index]}" \
            "${stress_operation_ids[$stress_index]}" "${stress_labels[$stress_index]}"
    done
    wait_for_file_state "$HOST_SOCK" "$WORK/host-file-stress-drained.json" enabled 120 \
        || fail "File did not return to zero active transfers after the saturation test"
    wait_for_upload_staging_absent 120 || fail "saturation test left owner upload staging behind"
    for stress_index in 0 1 2 3; do
        wait_for_path_absent "$SHARE_ROOT/${stress_paths[$stress_index]}" 30 \
            || fail "cancelled saturation upload was published: ${stress_paths[$stress_index]}"
    done
    wait_for_journal_absent "$WORK/node1/config/file_origin/$NETWORK_ID-file_origin_transfers.json" 120 \
        || fail "node1 retained cancelled saturation transfer journal entries"
    wait_for_journal_absent "$WORK/node2/config/file_origin/$NETWORK_ID-file_origin_transfers.json" 120 \
        || fail "node2 retained cancelled saturation transfer journal entries"
    assert_share_staging_empty
    assert_stress_runtime_stable host "$HOST_NODE_PID"
    assert_stress_runtime_stable node1 "$NODE1_NODE_PID"
    assert_stress_runtime_stable node2 "$NODE2_NODE_PID"
    local probe_count max_control_latency_ms
    probe_count=$(wc -l <"$WORK/control-probes.tsv" | tr -d '[:space:]')
    max_control_latency_ms=$(awk -F '\t' 'BEGIN { max = 0 } { if ($3 > max) max = $3 } END { print max + 0 }' "$WORK/control-probes.tsv")
    jq -n \
        --arg commit "$(git -C "$ROOT" rev-parse HEAD)" \
        --argjson probe_count "$probe_count" \
        --argjson rounds 8 \
        --argjson max_control_latency_ms "$max_control_latency_ms" \
        --argjson host_before_fd "$(sed -n 's/^fd=//p' "$WORK/resource-stress-host-before.txt")" \
        --argjson host_before_threads "$(sed -n 's/^threads=//p' "$WORK/resource-stress-host-before.txt")" \
        --argjson host_before_rss_kib "$(sed -n 's/^rss_kib=//p' "$WORK/resource-stress-host-before.txt")" \
        --argjson host_after_fd "$(sed -n 's/^fd=//p' "$WORK/resource-stress-host-after.txt")" \
        --argjson host_after_threads "$(sed -n 's/^threads=//p' "$WORK/resource-stress-host-after.txt")" \
        --argjson host_after_rss_kib "$(sed -n 's/^rss_kib=//p' "$WORK/resource-stress-host-after.txt")" \
        --argjson node1_before_fd "$(sed -n 's/^fd=//p' "$WORK/resource-stress-node1-before.txt")" \
        --argjson node1_before_threads "$(sed -n 's/^threads=//p' "$WORK/resource-stress-node1-before.txt")" \
        --argjson node1_before_rss_kib "$(sed -n 's/^rss_kib=//p' "$WORK/resource-stress-node1-before.txt")" \
        --argjson node1_after_fd "$(sed -n 's/^fd=//p' "$WORK/resource-stress-node1-after.txt")" \
        --argjson node1_after_threads "$(sed -n 's/^threads=//p' "$WORK/resource-stress-node1-after.txt")" \
        --argjson node1_after_rss_kib "$(sed -n 's/^rss_kib=//p' "$WORK/resource-stress-node1-after.txt")" \
        --argjson node2_before_fd "$(sed -n 's/^fd=//p' "$WORK/resource-stress-node2-before.txt")" \
        --argjson node2_before_threads "$(sed -n 's/^threads=//p' "$WORK/resource-stress-node2-before.txt")" \
        --argjson node2_before_rss_kib "$(sed -n 's/^rss_kib=//p' "$WORK/resource-stress-node2-before.txt")" \
        --argjson node2_after_fd "$(sed -n 's/^fd=//p' "$WORK/resource-stress-node2-after.txt")" \
        --argjson node2_after_threads "$(sed -n 's/^threads=//p' "$WORK/resource-stress-node2-after.txt")" \
        --argjson node2_after_rss_kib "$(sed -n 's/^rss_kib=//p' "$WORK/resource-stress-node2-after.txt")" \
        '{
          schema:"fuckipv6.w15.node-control-isolation.v1",
          commit:$commit,
          status:"pass",
          active_transfers:4,
          global_transfer_limit:4,
          control_probe:{operation:"get_snapshot",rounds:$rounds,count:$probe_count,max_latency_ms:$max_control_latency_ms,latency_limit_ms:2000},
          cleanup:{active_transfers:0,staging_empty:true,cancelled_targets_absent:true,journals_empty:true},
          runtime:{
            coordinator_owner:{before:{fd:$host_before_fd,threads:$host_before_threads,rss_kib:$host_before_rss_kib},after:{fd:$host_after_fd,threads:$host_after_threads,rss_kib:$host_after_rss_kib}},
            node1:{before:{fd:$node1_before_fd,threads:$node1_before_threads,rss_kib:$node1_before_rss_kib},after:{fd:$node1_after_fd,threads:$node1_after_threads,rss_kib:$node1_after_rss_kib}},
            node2:{before:{fd:$node2_before_fd,threads:$node2_before_threads,rss_kib:$node2_before_rss_kib},after:{fd:$node2_after_fd,threads:$node2_after_threads,rss_kib:$node2_after_rss_kib}}
          }
        }' >"$WORK/node-control-isolation.json"
    info "File saturation isolation passed: probes=$probe_count max_control_latency_ms=$max_control_latency_ms"

    start_transfer "$NODE1_SOCK" outage-resume "$HOST_NODE_ID" upload "$SHARE_ID" outage-resume.bin "$NODE1_ACL_UPLOAD_SOURCE" 262144
    transfer_id=$STARTED_TRANSFER_ID
    operation_id=$STARTED_OPERATION_ID
    wait_for_upload_staging 2097152 90 || fail "outage upload never exposed partial owner staging"
    UPLOAD_OUTAGE_STAGING=$STAGING_PATH
    file_control "$NODE1_SOCK" pause "$transfer_id" "$operation_id" outage-upload
    NODE1_JOURNAL="$WORK/node1/config/file_origin/$NETWORK_ID-file_origin_transfers.json"
    wait_for_journal_entry "$NODE1_JOURNAL" "$transfer_id" 90 || fail "outage upload did not persist paused journal state"
    [ -f "$UPLOAD_OUTAGE_STAGING" ] || fail "owner staging vanished before the simulated outage"
    info "paused upload survived the first safe request boundary"
    ip netns exec "$NSH" ip link set "$VH1" down
    sleep 3
    ip netns exec "$NSH" ip link set "$VH1" up
    wait_for_peer "$NODE1_SOCK" "$WORK/node1-after-veth.json" "$IP2" 120 || fail "node1 P2P did not recover after veth outage"
    wait_for_peer "$NODE2_SOCK" "$WORK/node2-after-veth.json" "$IP1" 120 || fail "node2 P2P did not recover after veth outage"
    wait_for_journal_entry "$NODE1_JOURNAL" "$transfer_id" 30 || fail "paused journal disappeared during veth outage"
    file_control "$NODE1_SOCK" resume "$transfer_id" "$operation_id" outage-upload
    wait_for_target_hash "$OUTAGE_UPLOAD_TARGET" "$NODE1_ACL_HASH" 240 || fail "outage/resume upload hash mismatch"
    wait_for_completion_log "$WORK/node1/node.log" "$transfer_id" 180 || fail "outage/resume upload did not complete"
    info "pause/veth outage/resume passed"

    info "reconfiguring to 8 MiB/s for a larger download pause and node restart"
    make_share_configuration "$config_full" full 8192
    configure_share "$config_full" enable-restart-rate
    wait_for_file_state "$HOST_SOCK" "$WORK/host-file-restart.json" enabled 60 || fail "restart-rate reconfigure did not settle"
    start_transfer "$NODE2_SOCK" restart-resume "$HOST_NODE_ID" download "$SHARE_ID" owner-source.bin "$NODE2_RESTART_TARGET" 262144
    transfer_id=$STARTED_TRANSFER_ID
    operation_id=$STARTED_OPERATION_ID
    wait_for_download_staging "$NODE2_DOWNLOADS" 120 || fail "restart download never exposed partial staging"
    DOWNLOAD_RESTART_STAGING=$STAGING_PATH
    file_control "$NODE2_SOCK" pause "$transfer_id" "$operation_id" restart-download
    NODE2_JOURNAL="$WORK/node2/config/file_origin/$NETWORK_ID-file_origin_transfers.json"
    wait_for_journal_entry "$NODE2_JOURNAL" "$transfer_id" 90 || fail "restart download did not persist paused journal state"
    [ -f "$DOWNLOAD_RESTART_STAGING" ] || fail "download staging vanished before SIGKILL"

    info "SIGKILLing node2 and waiting for its supervisor to restart the complete node data plane"
    OLD_NODE2_NODE_PID=$NODE2_NODE_PID
    kill -KILL "$OLD_NODE2_NODE_PID"
    wait_for_pid_gone "$OLD_NODE2_NODE_PID" 300 || fail "node2 process did not exit after SIGKILL"
    assert_pid_gone "$OLD_NODE2_NODE_PID" node2-before-restart
    NODE2_NODE_PID=$(wait_for_ready_node_pid "$WORK/node2/node.log" 120 "$OLD_NODE2_NODE_PID") \
        || fail "node2 supervisor did not report a restarted process"
    wait_for_live_socket "$NODE2_SOCK" "$WORK/node2-restarted.json" 120 || {
        tail -240 "$WORK/node2/node.log" >&2 || true
        fail "restarted node2 socket did not become live"
    }
    wait_for_network_online "$NODE2_SOCK" "$WORK/node2-restarted-online.json" 120 || fail "restarted node2 network did not become online"
    wait_for_peer "$NODE2_SOCK" "$WORK/node2-restarted-p2p.json" "$IP1" 120 || fail "restarted node2 did not recover healthy P2P"
    wait_for_journal_entry "$NODE2_JOURNAL" "$transfer_id" 90 || fail "restarted node2 did not restore paused transfer journal"
    [ -f "$DOWNLOAD_RESTART_STAGING" ] || fail "restarted node2 lost durable download staging"
    record_runtime_baseline node2 "$NODE2_NODE_PID"
    file_control "$NODE2_SOCK" resume "$transfer_id" "$operation_id" restart-download
    wait_for_target_hash "$NODE2_RESTART_TARGET" "$OWNER_HASH" 240 || fail "post-SIGKILL download hash mismatch"
    wait_for_completion_log "$WORK/node2/node.log" "$transfer_id" 180 || fail "post-SIGKILL download did not complete"
    info "SIGKILL/restart/journal resume passed"

    info "cancelling an in-flight upload and proving staging/target cleanup"
    make_share_configuration "$config_low" full 512
    configure_share "$config_low" enable-cancel-rate
    wait_for_file_state "$HOST_SOCK" "$WORK/host-file-cancel.json" enabled 60 || fail "cancel-rate reconfigure did not settle"
    start_transfer "$NODE1_SOCK" cancel-upload "$HOST_NODE_ID" upload "$SHARE_ID" cancel-upload.bin "$NODE1_CANCEL_UPLOAD_SOURCE" 262144
    transfer_id=$STARTED_TRANSFER_ID
    operation_id=$STARTED_OPERATION_ID
    wait_for_upload_staging 2097152 90 || fail "cancel upload never exposed owner staging"
    CANCEL_UPLOAD_STAGING=$STAGING_PATH
    file_control "$NODE1_SOCK" cancel "$transfer_id" "$operation_id" cancel-upload
    wait_for_upload_staging_absent 90 || fail "cancelled upload staging was not cleaned"
    wait_for_path_absent "$NODE1_CANCEL_UPLOAD_TARGET" 30 || fail "cancelled upload unexpectedly published its target"
    [ "$("$WORK/blake3sum" "$NODE1_CANCEL_UPLOAD_SOURCE")" = "$NODE1_CANCEL_HASH" ] || fail "cancelled upload mutated its local source"
    wait_for_journal_absent "$NODE1_JOURNAL" 60 || fail "cancelled upload remained in the origin journal"

    info "cancelling an in-flight download and proving staging/target cleanup"
    make_share_configuration "$config_full" full 8192
    configure_share "$config_full" enable-download-cancel-rate
    wait_for_file_state "$HOST_SOCK" "$WORK/host-file-download-cancel.json" enabled 60 || fail "download-cancel reconfigure did not settle"
    start_transfer "$NODE2_SOCK" cancel-download "$HOST_NODE_ID" download "$SHARE_ID" owner-source.bin "$NODE2_CANCEL_DOWNLOAD_TARGET" 262144
    transfer_id=$STARTED_TRANSFER_ID
    operation_id=$STARTED_OPERATION_ID
    wait_for_download_staging "$NODE2_DOWNLOADS" 120 || fail "cancel download never exposed partial staging"
    CANCEL_DOWNLOAD_STAGING=$STAGING_PATH
    file_control "$NODE2_SOCK" cancel "$transfer_id" "$operation_id" cancel-download
    wait_for_staging_absent "$NODE2_DOWNLOADS" 90 || fail "cancelled download staging was not cleaned"
    wait_for_path_absent "$NODE2_CANCEL_DOWNLOAD_TARGET" 30 || fail "cancelled download unexpectedly published its target"
    [ -f "$OWNER_SOURCE" ] || fail "owner source disappeared during download cancellation"
    wait_for_journal_absent "$NODE2_JOURNAL" 60 || fail "cancelled download remained in the origin journal"
    info "upload/download cancellation cleanup passed"

    info "revoking local ACLs during an active upload"
    make_share_configuration "$config_low" full 512
    configure_share "$config_low" enable-acl-rate
    wait_for_file_state "$HOST_SOCK" "$WORK/host-file-acl.json" enabled 60 || fail "ACL-rate reconfigure did not settle"
    start_transfer "$NODE1_SOCK" acl-revoke "$HOST_NODE_ID" upload "$SHARE_ID" acl-revoke.bin "$NODE1_ACL_UPLOAD_SOURCE" 262144
    transfer_id=$STARTED_TRANSFER_ID
    operation_id=$STARTED_OPERATION_ID
    wait_for_upload_staging 2097152 90 || fail "ACL-revoke upload never exposed owner staging"
    ACL_REVOKE_STAGING=$STAGING_PATH
    local config_revoked="$WORK/share-revoked.json"
    make_share_configuration "$config_revoked" revoked 512
    configure_share "$config_revoked" revoke-acl
    terminal_line=$(wait_for_terminal_log "$WORK/node1/node.log" "$transfer_id" \
        'file_(capability_revoked|transfer_(access_revoked|permission_revoked|session_unavailable|capability_revoked|send_failed))' 180) \
        || { tail -240 "$WORK/node1/node.log" >&2 || true; fail "ACL revocation did not terminate the active requester transfer"; }
    printf '%s\n' "$terminal_line" >"$WORK/acl-revoke-terminal.log"
    wait_for_upload_staging_absent 90 || fail "ACL revocation did not clean owner upload staging"
    wait_for_path_absent "$ACL_REVOKE_TARGET" 30 || fail "ACL-revoked upload was published"
    [ -f "$NODE1_ACL_UPLOAD_SOURCE" ] || fail "ACL-revoked upload source was removed"
    wait_for_file_state "$HOST_SOCK" "$WORK/host-file-acl-revoked.json" enabled 60 || fail "ACL-revoked capability did not settle active_transfers=0"
    wait_for_journal_absent "$NODE1_JOURNAL" 90 || fail "ACL-revoked transfer remained in journal"
    info "online ACL revocation failed closed and preserved the owner source"

    info "restoring ACLs before revoking the signed network File capability"
    make_share_configuration "$config_low" full 512
    configure_share "$config_low" restore-acl
    wait_for_file_state "$HOST_SOCK" "$WORK/host-file-acl-restored.json" enabled 60 || fail "ACL restore did not settle"
    start_transfer "$NODE1_SOCK" network-revoke "$HOST_NODE_ID" upload "$SHARE_ID" network-revoke.bin "$NODE1_NETWORK_REVOKE_SOURCE" 262144
    transfer_id=$STARTED_TRANSFER_ID
    operation_id=$STARTED_OPERATION_ID
    wait_for_upload_staging 2097152 120 || fail "network-revoke upload never exposed owner staging"
    NETWORK_REVOKE_STAGING=$STAGING_PATH
    set_network_file_capability no revoke-file
    terminal_line=$(wait_for_terminal_log "$WORK/node1/node.log" "$transfer_id" \
        'file_(capability_unavailable|capability_revoked|transfer_access_revoked|transfer_session_unavailable|transfer_send_failed)' 180) \
        || { tail -260 "$WORK/node1/node.log" >&2 || true; fail "network File revocation did not terminate the active requester transfer"; }
    printf '%s\n' "$terminal_line" >"$WORK/network-revoke-terminal.log"
    wait_for_upload_staging_absent 120 || fail "network File revocation did not clean owner staging"
    wait_for_path_absent "$NETWORK_REVOKE_TARGET" 30 || fail "network-revoked upload was published"
    [ -f "$NODE1_NETWORK_REVOKE_SOURCE" ] || fail "network-revoked upload source was removed"
    wait_for_journal_absent "$NODE1_JOURNAL" 90 || fail "network-revoked transfer remained in journal"
    info "network File capability revocation failed closed"

    info "restoring network capability and checking final zero-transfer state"
    set_network_file_capability yes restore-file
    wait_for_file_state "$HOST_SOCK" "$WORK/host-file-final.json" enabled 90 || fail "owner File capability did not return to zero active transfers"
    wait_for_upload_staging_absent 60 || fail "owner staging is not empty at final check"
    assert_share_staging_empty
    wait_for_staging_absent "$NODE1_DOWNLOADS" 30 || fail "node1 download staging is not empty"
    wait_for_staging_absent "$NODE2_DOWNLOADS" 30 || fail "node2 download staging is not empty"
    wait_for_journal_absent "$NODE1_JOURNAL" 60 || fail "node1 origin journal is not empty"
    wait_for_journal_absent "$NODE2_JOURNAL" 60 || fail "node2 origin journal is not empty"

    snapshot_to "$HOST_SOCK" "$WORK/final-host-snapshot.json"
    snapshot_to "$NODE1_SOCK" "$WORK/final-node1-snapshot.json"
    snapshot_to "$NODE2_SOCK" "$WORK/final-node2-snapshot.json"
    assert_runtime_stable host "$HOST_NODE_PID"
    assert_runtime_stable node1 "$NODE1_NODE_PID"
    assert_runtime_stable node2 "$NODE2_NODE_PID"
    assert_no_private_path_leaks

    ip rule show >"$WORK/host-ip-rule.after"
    ip -6 rule show >"$WORK/host-ip6-rule.after"
    ip route show default >"$WORK/host-default-route.after"
    cmp -s "$WORK/host-ip-rule.before" "$WORK/host-ip-rule.after" || fail "host IPv4 policy rules changed"
    cmp -s "$WORK/host-ip6-rule.before" "$WORK/host-ip6-rule.after" || fail "host IPv6 policy rules changed"
    cmp -s "$WORK/host-default-route.before" "$WORK/host-default-route.after" || fail "host default route changed"

    local summary_host=$WORK/resource-host-after.txt
    printf '[SUMMARY resources]\n'
    cat "$WORK/resource-host-before.txt" "$WORK/resource-node1-before.txt" "$WORK/resource-node2-before.txt"
    cat "$WORK/resource-host-after.txt" "$WORK/resource-node1-after.txt" "$WORK/resource-node2-after.txt"
    printf '[SUMMARY transfer terminal evidence]\n'
    cat "$WORK/acl-revoke-terminal.log" "$WORK/network-revoke-terminal.log"
    printf '[SUMMARY owner capability]\n'
    jq '.data.file_capability' "$WORK/final-host-snapshot.json"

    info "stopping tracked processes and removing only this test's namespaces"
    cleanup
    assert_pid_gone "$HPID" host
    assert_pid_gone "$N1PID" node1
    assert_pid_gone "$N2PID" node2
    assert_pid_gone "$HOST_NODE_PID" host-node
    assert_pid_gone "$NODE1_NODE_PID" node1-node
    assert_pid_gone "$NODE2_NODE_PID" node2-node
    for port in "$HOST_PORT" "$RELAY_PORT" "$WEBUI_HOST" "$WEBUI1" "$WEBUI2"; do
        assert_port_free "$port"
    done
    assert_namespace_absent "$NSH"
    assert_namespace_absent "$NS1"
    assert_namespace_absent "$NS2"
    ! ip link show "$VH1" >/dev/null 2>&1 || fail "test veth $VH1 remains"
    ! ip link show "$VH2" >/dev/null 2>&1 || fail "test veth $VH2 remains"

    info "PASS: formal invitations, signed File capability, direct P2P uploads/downloads, BLAKE3 verification,"
    info "      pause/veth outage, SIGKILL journal resume, cancel cleanup, live ACL/capability revocation,"
    info "      path-free observability, resource baselines and namespace/process cleanup all passed"
}

main "$@"
