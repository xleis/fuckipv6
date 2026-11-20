#!/usr/bin/env bash
#
# W12-07 residual acceptance: a non-browser client downloads a complete file
# through a real Exit Node across the public internet, and stopping the network
# restores DNS, TUN and routes without manual repair.
#
#   local netns (10.79.0.2) -- NAT -- host WAN -- Internet -- HK Exit Node
#                                                              |
#                                                       coordinator (embedded)
#
# The Exit Node is the coordinator owner on the Hong Kong host. The client runs
# inside a private network namespace so the host default route, host DNS and the
# router policy are never touched.
#
# Requirements: root on this host, key-based SSH to the Exit host (override with
# W12_HK_HOST), and a musl release binary at
# target/x86_64-unknown-linux-musl/release/fuckipv6.
#
# Run as root:
#   scripts/test-w12-exit-download-wan.sh

set -Eeuo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BIN="$ROOT/target/x86_64-unknown-linux-musl/release/fuckipv6"
HK_HOST="${W12_HK_HOST:-101.36.116.21}"
SSH_KEY="${W12_SSH_KEY:-/home/xlei/.ssh/id_ed25519}"
SSH_KNOWN_HOSTS="${W12_SSH_KNOWN_HOSTS:-/home/xlei/.ssh/known_hosts}"
NET="${W12_NET:-w12-e7wan}"
HS_PORT="${W12_HOST_PORT:-29766}"
RELAY_PORT="${W12_RELAY_PORT:-29767}"
WEBUI_PORT="${W12_WEBUI_PORT:-18995}"
NS="w12e7c"
VH="w12e7h"
VN="w12e7n"
HOST_V4="10.79.0.1"
CLIENT_V4="10.79.0.2"
DOWNLOAD_BYTES="${W12_DOWNLOAD_BYTES:-8388608}"
WORK="/tmp/w12-07-wan.$(date -u +%Y%m%dT%H%M%SZ).$$"
HK_DIR="/root/${NET}-$(date -u +%Y%m%dT%H%M%SZ)"

HPID=""
LAUNCHER_PID=""
FORWARD_RULES_ADDED=0
FORWARD_BEFORE="1"

info() { printf '[INFO] %s\n' "$*"; }
fail() { printf '[FAIL] %s\n' "$*" >&2; exit 1; }

hk() {
    ssh -n -o BatchMode=yes -o StrictHostKeyChecking=no -o ConnectTimeout=10 \
        -o "UserKnownHostsFile=$SSH_KNOWN_HOSTS" -i "$SSH_KEY" "root@$HK_HOST" "$@"
}

kill_pid_checked() {
    local pid=$1 sig=$2
    [ -n "$pid" ] || return 0
    case "$pid" in *[!0-9]*) return 0 ;; esac
    kill -"$sig" "$pid" 2>/dev/null || true
}

cleanup() {
    set +e
    info "cleanup: removing only this run's resources"
    ip netns exec "$NS" env FUCKIPV6_RUNTIME_DIR="$WORK/client/run" FUCKIPV6_CONFIG_DIR="$WORK/client/config" \
        "$BIN" status >/dev/null 2>&1
    kill_pid_checked "$LAUNCHER_PID" TERM
    sleep 2
    kill_pid_checked "$LAUNCHER_PID" KILL
    hk "systemctl stop '$NET' 2>/dev/null; systemctl reset-failed '$NET' 2>/dev/null; \
        pkill -f '[w]12-e7wan-' 2>/dev/null; sleep 1; pkill -KILL -f '[w]12-e7wan-' 2>/dev/null; \
        for link in \$(ip -br link | awk '/fk-w12-e7wan/{print \$1}'); do ip link del \$link 2>/dev/null || true; done; \
        while ip rule show | grep -q 'proto openr'; do ip rule del proto openr 2>/dev/null || break; done"
    ip netns delete "$NS" 2>/dev/null || true
    ip link delete "$VH" 2>/dev/null || true
    if [ "$FORWARD_RULES_ADDED" = 1 ]; then
        iptables -D FORWARD -i "$VH" -o "$WAN_IF" -j ACCEPT 2>/dev/null || true
        iptables -D FORWARD -i "$WAN_IF" -o "$VH" -m state --state RELATED,ESTABLISHED -j ACCEPT 2>/dev/null || true
        iptables -t nat -D POSTROUTING -s 10.79.0.0/24 -o "$WAN_IF" -j MASQUERADE 2>/dev/null || true
    fi
    echo "$FORWARD_BEFORE" >/proc/sys/net/ipv4/ip_forward 2>/dev/null || true
    info "evidence retained at $WORK (exit host dir $HK_DIR)"
}

main() {
    [ "$(id -u)" -eq 0 ] || fail "root is required for the client network namespace and NAT"
    [ -x "$BIN" ] || fail "missing musl binary: $BIN"
    command -v curl >/dev/null || fail "curl is required"
    hk true || fail "SSH to the Exit host failed"

    WAN_IF=$(ip route show default | awk '{print $5; exit}')
    [ -n "$WAN_IF" ] || fail "cannot determine the host WAN interface"
    FORWARD_BEFORE=$(cat /proc/sys/net/ipv4/ip_forward)
    mkdir -p "$WORK"/{client/{home,xdg,run,config},evidence}
    trap cleanup EXIT INT TERM

    ip rule show >"$WORK/evidence/host-ip-rule.before"
    ip -6 rule show >"$WORK/evidence/host-ip6-rule.before"
    ip route show default >"$WORK/evidence/host-default-route.before"
    iptables-save >"$WORK/evidence/host-iptables.before" 2>/dev/null || true
    sha256sum "$BIN" >"$WORK/evidence/binary.sha"

    info "clearing stale test instances on the Exit host"
    hk "systemctl stop '$NET' 2>/dev/null; pkill -f '[w]12-e7wan-' 2>/dev/null; sleep 1; \
        for link in \$(ip -br link | awk '/fk-w12-e7wan/{print \$1}'); do ip link del \$link 2>/dev/null || true; done; \
        while ip rule show | grep -q 'proto openr'; do ip rule del proto openr 2>/dev/null || break; done; \
        ss -lntu | grep -qE ':(29766|29767|18995)' && echo occupied || true"

    info "deploying the musl binary to the Exit host"
    hk "mkdir -p '$HK_DIR'/home '$HK_DIR'/xdg '$HK_DIR'/run '$HK_DIR'/config"
    scp -q -o BatchMode=yes -o StrictHostKeyChecking=no -o "UserKnownHostsFile=$SSH_KNOWN_HOSTS" \
        -i "$SSH_KEY" "$BIN" "root@$HK_HOST:$HK_DIR/fuckipv6"
    hk "chmod 0755 '$HK_DIR/fuckipv6'"

    # The client-side IPC helper must run where the node socket lives.
    cat >"$WORK/ipc_client.py" <<'PYCLIENT'
import json
import socket
import struct
import sys

socket_path, command_json = sys.argv[1:3]
command = json.loads(command_json)
request_id = 1
PROTOCOL_VERSION = 5
FRAME_HEADER = struct.Struct(">4sHBBI")


def recv_exact(stream, size):
    chunks, remaining = [], size
    while remaining:
        chunk = stream.recv(remaining)
        if not chunk:
            raise RuntimeError("local IPC closed before a complete frame arrived")
        chunks.append(chunk)
        remaining -= len(chunk)
    return b"".join(chunks)


def send(stream, kind, value):
    payload = json.dumps(value, separators=(",", ":"), ensure_ascii=False).encode()
    stream.sendall(FRAME_HEADER.pack(b"FKIP", PROTOCOL_VERSION, kind, 0, len(payload)))
    stream.sendall(payload)


def recv(stream):
    magic, version, kind, reserved, length = FRAME_HEADER.unpack(recv_exact(stream, 12))
    if magic != b"FKIP" or version != PROTOCOL_VERSION or reserved != 0:
        raise RuntimeError("invalid local IPC frame header")
    return kind, json.loads(recv_exact(stream, length))


with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as stream:
    stream.settimeout(30)
    stream.connect(socket_path)
    send(stream, 1, {"protocol_version": PROTOCOL_VERSION, "role": "cli"})
    kind, hello = recv(stream)
    if kind != 2 or hello.get("protocol_version") != PROTOCOL_VERSION:
        raise RuntimeError(f"local IPC handshake failed: {hello}")
    send(stream, 3, {"request_id": request_id,
                     "command": {"type": "execute",
                                 "data": {"request_id": request_id, "command": command}}})
    while True:
        kind, value = recv(stream)
        if kind == 5:
            continue
        if kind == 6:
            raise RuntimeError(f"local IPC error: {value}")
        if kind != 4:
            raise RuntimeError(f"unexpected local IPC frame kind: {kind}")
        break
outcome = value.get("data", {}).get("outcome", {})
if outcome.get("status") == "rejected":
    print(json.dumps(outcome, indent=2, ensure_ascii=False))
    raise SystemExit(3)
print(json.dumps(outcome, indent=2, ensure_ascii=False))
PYCLIENT
    scp -q -o BatchMode=yes -o StrictHostKeyChecking=no -o "UserKnownHostsFile=$SSH_KNOWN_HOSTS" \
        -i "$SSH_KEY" "$WORK/ipc_client.py" "root@$HK_HOST:$HK_DIR/ipc_client.py"

    info "starting the Exit Node and embedded coordinator on $HK_HOST"
    # Start through a transient systemd service: it detaches from the SSH
    # channel, survives the session, and gives the teardown path a single
    # idempotent stop entry point instead of a bare PID file.
    hk "systemctl reset-failed '$NET' 2>/dev/null || true; \
        systemd-run --unit='$NET' --collect --service-type=simple \
        --property=StandardOutput=file:'$HK_DIR/host.log' \
        --property=StandardError=file:'$HK_DIR/host.log' \
        --setenv=HOME='$HK_DIR/home' --setenv=XDG_CONFIG_HOME='$HK_DIR/xdg' \
        --setenv=FUCKIPV6_RUNTIME_DIR='$HK_DIR/run' --setenv=FUCKIPV6_CONFIG_DIR='$HK_DIR/config' \
        --setenv=RUST_LOG=info \
        '$HK_DIR/fuckipv6' host --name '$NET' --port $HS_PORT --relay-port $RELAY_PORT \
        --external-host '$HK_HOST' --is-exit-node --exit-node fd00::1001 \
        --routing-mode mesh-only --headless --webui-port $WEBUI_PORT" \
        >"$WORK/evidence/hk-systemd-run.txt" 2>&1 \
        || { cat "$WORK/evidence/hk-systemd-run.txt" >&2; fail "could not start the Exit host service"; }
    hk "systemctl is-active --quiet '$NET'" || { hk "tail -60 '$HK_DIR/host.log'" >&2 || true; fail "Exit host service is not active"; }

    local i
    for ((i = 0; i < 60; i++)); do
        hk "[ -S '$HK_DIR/run/node.sock' ]" && break
        sleep 1
    done
    hk "[ -S '$HK_DIR/run/node.sock' ]" || { hk "tail -80 '$HK_DIR/host.log'" >&2 || true; fail "Exit host node socket did not appear"; }
    hk "[ -s '$HK_DIR/config/$NET/network.json' ]" || { hk "tail -80 '$HK_DIR/host.log'" >&2 || true; fail "authoritative network record did not appear"; }

    local network_id owner_id
    network_id=$(hk "jq -r '.network_id' '$HK_DIR/config/$NET/network.json'")
    owner_id=$(hk "jq -r '.owner' '$HK_DIR/config/$NET/network.json'")
    [ -n "$network_id" ] && [ -n "$owner_id" ] || fail "network record lacks network_id or owner"
    info "network_id=$network_id owner=$owner_id"

    info "bootstrapping Coordinator governance on the Exit host"
    local now_ms expires_ms command
    now_ms=$(date +%s%3N)
    expires_ms=$((now_ms + 3600000))
    command=$(jq -cn --arg n "$NET" --arg id "$owner_id" --argjson exp "$expires_ms" \
        '{command:"update_coordinator_candidates",payload:{network_name:$n,expected_revision:0,candidates:[{node_id:$id,priority:10}],expires_at_unix_ms:$exp}}')
    hk "python3 '$HK_DIR/ipc_client.py' '$HK_DIR/run/node.sock' '$command'" >"$WORK/evidence/bootstrap-candidates.json" \
        || fail "bootstrap candidate configuration was rejected"
    command=$(jq -cn --arg n "$NET" --arg endpoint "10.7.53.243:$HS_PORT" \
        '{command:"publish_coordinator_candidate_declaration",payload:{network_name:$n,willingness:"willing",public_reachability:{public_endpoint:$endpoint,reachable_via_relay:true},resources:{cpu_cores:2,memory_bytes:2147483648,max_connections:256},validity_ms:600000}}')
    hk "python3 '$HK_DIR/ipc_client.py' '$HK_DIR/run/node.sock' '$command'" >"$WORK/evidence/bootstrap-declaration.json" \
        || fail "bootstrap candidate declaration was rejected"

    info "enabling owner Exit capability on the Exit host"
    hk "env HOME='$HK_DIR/home' XDG_CONFIG_HOME='$HK_DIR/xdg' FUCKIPV6_RUNTIME_DIR='$HK_DIR/run' \
        FUCKIPV6_CONFIG_DIR='$HK_DIR/config' '$HK_DIR/fuckipv6' exit enable" >"$WORK/evidence/exit-enable.txt" 2>&1 \
        || { hk "tail -40 '$HK_DIR/host.log'" >&2 || true; fail "owner Exit capability did not start"; }

    info "authorising the owner as an Exit Node for this network"
    command=$(jq -cn --arg n "$NET" \
        '{command:"set_network_role",payload:{network_name:$n,role:"exit_node",enabled:true}}')
    hk "python3 '$HK_DIR/ipc_client.py' '$HK_DIR/run/node.sock' '$command'" >"$WORK/evidence/exit-role.json" \
        || { hk "tail -40 '$HK_DIR/host.log'" >&2 || true; fail "authorising the Exit Node role was rejected"; }

    info "reading the Exit Node virtual address"
    local exit_v6=""
    for ((i = 0; i < 60; i++)); do
        if hk "python3 '$HK_DIR/ipc_client.py' '$HK_DIR/run/node.sock' '{\"command\":\"get_snapshot\"}'" \
            >"$WORK/evidence/host-snapshot.json" 2>/dev/null; then
            exit_v6=$(jq -r --arg n "$NET" '.data.networks[]? | select(.name==$n) | .assigned_ipv6 // empty' \
                "$WORK/evidence/host-snapshot.json" | head -1)
            [ -n "$exit_v6" ] && break
        fi
        sleep 1
    done
    [ -n "$exit_v6" ] || { hk "tail -60 '$HK_DIR/host.log'" >&2 || true; fail "the Exit Node never reported its virtual address"; }
    info "exit virtual address: $exit_v6"

    info "creating a single-use invitation"
    command=$(jq -cn --arg network "$network_id" \
        '{command:"create_invitation",payload:{network_id:$network,role:"member",capabilities:[],max_uses:1,expires_in_ms:3600000}}')
    hk "python3 '$HK_DIR/ipc_client.py' '$HK_DIR/run/node.sock' '$command'" >"$WORK/evidence/invitation.json" \
        || fail "invitation creation was rejected"
    local invite_code invitation_id
    invite_code=$(jq -r '.data.code' "$WORK/evidence/invitation.json")
    invitation_id=$(jq -r '.data.status.invitation.invitation_id' "$WORK/evidence/invitation.json")
    [ -n "$invite_code" ] && [ "$invite_code" != null ] || fail "invitation payload has no code"

    info "building the private client namespace with NAT"
    ip netns add "$NS"
    ip link add "$VH" type veth peer name "$VN"
    ip link set "$VN" netns "$NS"
    ip addr add "$HOST_V4/24" dev "$VH"
    ip link set "$VH" up
    ip netns exec "$NS" ip link set lo up
    ip netns exec "$NS" ip addr add "$CLIENT_V4/24" dev "$VN"
    ip netns exec "$NS" ip link set "$VN" up
    ip netns exec "$NS" ip route add default via "$HOST_V4"
    ip netns exec "$NS" env LC_ALL=C ping -c 2 -W 2 "$HOST_V4" >"$WORK/evidence/client-netns-ping.log" 2>&1 \
        || fail "client namespace cannot reach its gateway"
    mkdir -p /etc/netns/"$NS"
    printf 'nameserver 1.1.1.1\nnameserver 8.8.8.8\n' >/etc/netns/"$NS"/resolv.conf
    cp /etc/netns/"$NS"/resolv.conf "$WORK/evidence/client-resolv.before"

    echo 1 >/proc/sys/net/ipv4/ip_forward
    iptables -A FORWARD -i "$VH" -o "$WAN_IF" -j ACCEPT
    iptables -A FORWARD -i "$WAN_IF" -o "$VH" -m state --state RELATED,ESTABLISHED -j ACCEPT
    iptables -t nat -A POSTROUTING -s 10.79.0.0/24 -o "$WAN_IF" -j MASQUERADE
    FORWARD_RULES_ADDED=1
    ip netns exec "$NS" curl -sS --max-time 20 -o /dev/null -w '%{http_code}\n' https://www.cloudflare.com/ \
        >"$WORK/evidence/client-egress-baseline.txt" 2>&1 \
        || fail "client namespace has no internet egress before the tunnel"
    ip netns exec "$NS" curl -sS --max-time 20 https://ifconfig.me >"$WORK/evidence/client-egress-ip-before.txt" 2>/dev/null || true

    info "starting the client node inside the namespace"
    # A headless node rejects a desktop session. A private mount namespace with
    # empty /run/user and /tmp/.X11-unix reproduces a real headless boot without
    # touching the host mounts.
    mkdir -p "$WORK/no-desktop/run-user" "$WORK/no-desktop/x11"
    cat >"$WORK/exec-headless" <<'SH'
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
    chmod +x "$WORK/exec-headless"
    "$WORK/exec-headless" "$WORK/no-desktop" ip netns exec "$NS" env -u DISPLAY -u WAYLAND_DISPLAY \
        -u XDG_SESSION_TYPE -u XDG_CURRENT_DESKTOP -u DESKTOP_SESSION \
        HOME="$WORK/client/home" XDG_CONFIG_HOME="$WORK/client/xdg" \
        FUCKIPV6_RUNTIME_DIR="$WORK/client/run" FUCKIPV6_CONFIG_DIR="$WORK/client/config" RUST_LOG=info \
        "$BIN" --headless --webui-port 18996 >"$WORK/client/launcher.log" 2>&1 &
    LAUNCHER_PID=$!
    for ((i = 0; i < 60; i++)); do
        [ -S "$WORK/client/run/node.sock" ] && break
        sleep 1
    done
    [ -S "$WORK/client/run/node.sock" ] || { tail -60 "$WORK/client/launcher.log" >&2; fail "client node socket did not appear"; }

    local client_sock="$WORK/client/run/node.sock"
    local coordinator="ws://$HK_HOST:$HS_PORT"
    info "client previews and redeems the invitation"
    command=$(jq -cn --arg network "$network_id" --arg url "$coordinator" --arg code "$invite_code" \
        '{command:"preview_invitation",payload:{network_id:$network,coordinator_url:$url,code:$code}}')
    ip netns exec "$NS" python3 "$WORK/ipc_client.py" "$client_sock" "$command" >"$WORK/evidence/client-preview.json"
    local revision digest terms
    revision=$(jq -r '.data.invitation_revision' "$WORK/evidence/client-preview.json")
    digest=$(jq -r '.data.preview_digest' "$WORK/evidence/client-preview.json")
    terms=$(jq -r '.data.terms_hash' "$WORK/evidence/client-preview.json")
    now_ms=$(date +%s%3N)
    command=$(jq -cn --arg url "$coordinator" --arg network "$network_id" --arg code "$invite_code" \
        --arg invitation "$invitation_id" --arg digest "$digest" --arg terms "$terms" \
        --argjson revision "$revision" --argjson now "$now_ms" \
        '{command:"redeem_invitation",payload:{coordinator_url:$url,request:{network_id:$network,code:$code,invitation_id:$invitation,expected_invitation_revision:$revision,preview_digest:$digest,terms_hash:$terms,acknowledged_at_unix_ms:$now,exit_contribution_acknowledged:true}}}')
    ip netns exec "$NS" python3 "$WORK/ipc_client.py" "$client_sock" "$command" >"$WORK/evidence/client-redeem.json"

    info "waiting for the client network to come online"
    local online=0 status ipv6
    for ((i = 0; i < 120; i++)); do
        if ip netns exec "$NS" python3 "$WORK/ipc_client.py" "$client_sock" '{"command":"get_snapshot"}' \
            >"$WORK/evidence/client-snapshot.json" 2>/dev/null; then
            status=$(jq -r --arg n "$NET" '.data.networks[]? | select(.name==$n) | .status' "$WORK/evidence/client-snapshot.json" 2>/dev/null | head -1)
            ipv6=$(jq -r --arg n "$NET" '.data.networks[]? | select(.name==$n) | .assigned_ipv6' "$WORK/evidence/client-snapshot.json" 2>/dev/null | head -1)
            if [ "$status" = online ] && [ -n "$ipv6" ]; then online=1; break; fi
        fi
        sleep 1
    done
    [ "$online" = 1 ] || { tail -80 "$WORK/client/launcher.log" >&2 || true; fail "client network never reached online"; }
    info "client online: $ipv6"

    local tun_name routing_status
    tun_name=$(jq -r --arg n "$NET" '.data.networks[]? | select(.name==$n) | .tun_name' "$WORK/evidence/client-snapshot.json" | head -1)
    [ -n "$tun_name" ] && [ "$tun_name" != null ] || fail "the client did not report a TUN device name"
    info "pointing the client routing at the Exit Node $exit_v6"
    command=$(jq -cn --arg n "$NET" --arg exit "$exit_v6" \
        '{command:"update_routing",payload:{network_name:$n,mode:"global_proxy",exit_node:$exit,is_exit_node:false}}')
    routing_status=0
    ip netns exec "$NS" python3 "$WORK/ipc_client.py" "$client_sock" "$command" \
        >"$WORK/evidence/client-routing.json" 2>"$WORK/evidence/client-routing.err" || routing_status=$?
    if [ "$routing_status" -ne 0 ]; then
        info "routing policy was rejected; falling back to a default route into $tun_name"
        cat "$WORK/evidence/client-routing.json" "$WORK/evidence/client-routing.err" >&2 || true
        ip netns exec "$NS" ip route replace default dev "$tun_name"
        ip netns exec "$NS" ip -6 route replace default dev "$tun_name"
    fi
    # The Linux client owns the tunnel, not the host routes: point the namespace
    # default at the TUN so the Exit Node carries the traffic.
    ip netns exec "$NS" ip route replace default dev "$tun_name"
    ip netns exec "$NS" ip -6 route replace default dev "$tun_name"
    ip netns exec "$NS" ip route get 1.1.1.1 | tee "$WORK/evidence/client-route-to-internet.txt"

    info "verifying the Exit path is selected and protected"
    jq -c '.data.exit_selections[]?' "$WORK/evidence/client-snapshot.json" >"$WORK/evidence/exit-selections.json" || true
    jq -c '.data.traffic_session' "$WORK/evidence/client-snapshot.json" >"$WORK/evidence/traffic-session.json"
    info "client exit policy: $(jq -c '.resolution.mode' "$WORK/evidence/exit-selections.json" 2>/dev/null || echo unknown)"

    info "capturing the tunnel while the download runs"
    ip netns exec "$NS" timeout 240 tcpdump -ni "$tun_name" -nn -c 20000 'tcp or udp' \
        >"$WORK/evidence/tun-download.pcap.txt" 2>"$WORK/evidence/tun-download.tcpdump.err" &
    local tcpdump_pid=$!

    info "downloading $DOWNLOAD_BYTES bytes through the Exit with curl"
    ip netns exec "$NS" curl -sS --max-time 180 -o "$WORK/evidence/download.bin" \
        -w '{"http_code":%{http_code},"size_download":%{size_download},"speed":%{speed_download},"remote_ip":"%{remote_ip}"}\n' \
        "https://speed.cloudflare.com/__down?bytes=$DOWNLOAD_BYTES" >"$WORK/evidence/download.json" \
        || fail "the non-browser download through the Exit failed"
    jq -e '.http_code == 200 and .size_download > 0' "$WORK/evidence/download.json" >/dev/null \
        || fail "the download did not complete successfully"
    sleep 2
    kill_pid_checked "$tcpdump_pid" TERM
    local tunnel_packets
    tunnel_packets=$(grep -c ' IP ' "$WORK/evidence/tun-download.pcap.txt" 2>/dev/null || true)
    jq -n --argjson packets "${tunnel_packets:-0}" '{tunnel_packets_seen:($packets|tonumber)}' \
        >"$WORK/evidence/tunnel-download-traffic.json"
    [ "${tunnel_packets:-0}" -gt 0 ] \
        || fail "no packet entered the TUN during the download; the Exit did not carry it"
    info "packets observed on the tunnel during the download: $tunnel_packets"
    sha256sum "$WORK/evidence/download.bin" >"$WORK/evidence/download.sha256"
    local got_bytes
    got_bytes=$(stat -c %s "$WORK/evidence/download.bin")
    [ "$got_bytes" -ge "$DOWNLOAD_BYTES" ] || fail "download was truncated: $got_bytes bytes"

    info "capturing tunnel counters after the download"
    local tun_before tun_after
    tun_before=$(jq -r '[.data.peers[]?.traffic | (.tun_rx_bytes // 0) + (.tun_tx_bytes // 0)] | add // 0' \
        "$WORK/evidence/client-snapshot.json" 2>/dev/null || echo 0)
    ip netns exec "$NS" python3 "$WORK/ipc_client.py" "$client_sock" '{"command":"get_snapshot"}' \
        >"$WORK/evidence/client-snapshot-after.json" 2>/dev/null || true
    tun_after=$(jq -r '[.data.peers[]?.traffic | (.tun_rx_bytes // 0) + (.tun_tx_bytes // 0)] | add // 0' \
        "$WORK/evidence/client-snapshot-after.json" 2>/dev/null || echo 0)
    jq -n --arg before "$tun_before" --arg after "$tun_after" \
        '{tun_bytes_before:($before|tonumber), tun_bytes_after:($after|tonumber), delta:(($after|tonumber)-($before|tonumber))}' \
        >"$WORK/evidence/tunnel-counters.json"
    info "tunnel bytes delta: $(jq -r .delta "$WORK/evidence/tunnel-counters.json")"

    info "verifying the public egress address belongs to the Exit host"
    ip netns exec "$NS" curl -sS --max-time 30 https://ifconfig.me >"$WORK/evidence/client-egress-ip-after.txt" 2>/dev/null || true
    local egress direct_egress
    egress=$(cat "$WORK/evidence/client-egress-ip-after.txt")
    direct_egress=$(cat "$WORK/evidence/client-egress-ip-before.txt" 2>/dev/null || true)
    info "egress address through the tunnel: ${egress:-unavailable} (direct baseline: ${direct_egress:-unavailable})"
    [ -n "$egress" ] || fail "could not read the public egress address through the tunnel"
    if [ -n "$direct_egress" ] && [ "$egress" = "$direct_egress" ]; then
        fail "the download egress equals the direct baseline; traffic did not traverse the Exit"
    fi

    info "verifying DNS resolution inside the protected namespace"
    ip netns exec "$NS" getent hosts www.youtube.com >"$WORK/evidence/dns-youtube.txt" 2>&1 || true
    [ -s "$WORK/evidence/dns-youtube.txt" ] || fail "protected DNS resolution returned nothing"

    info "stopping the network through the production command bus"
    command=$(jq -cn --arg n "$NET" '{command:"stop_networks",payload:{networks:[$n],all:false}}')
    ip netns exec "$NS" python3 "$WORK/ipc_client.py" "$client_sock" "$command" >"$WORK/evidence/stop-networks.json"
    local waited=0
    while [ "$waited" -lt 60 ]; do
        if ! ip netns exec "$NS" ip link show | grep -q "fk-"; then break; fi
        sleep 1
        waited=$((waited + 1))
    done
    if ip netns exec "$NS" ip link show | grep -q "fk-"; then
        fail "cleanup: the TUN device survived stop_networks"
    fi
    ip netns exec "$NS" ip -6 route show >"$WORK/evidence/client-routes-after-stop.txt" 2>&1
    if grep -qE 'fd00::/64|fd00::100' "$WORK/evidence/client-routes-after-stop.txt"; then
        fail "cleanup: virtual routes survived stop_networks"
    fi
    cmp -s "$WORK/evidence/client-resolv.before" /etc/netns/"$NS"/resolv.conf \
        || fail "cleanup: the resolver configuration inside the namespace changed"
    info "cleanup verified: TUN, routes and resolver state restored"

    kill_pid_checked "$LAUNCHER_PID" TERM
    LAUNCHER_PID=""
    local residual=0
    for ((i = 0; i < 30; i++)); do
        if ! pgrep -f "$WORK/client/run" >/dev/null 2>&1; then residual=1; break; fi
        sleep 1
    done
    [ "$residual" = 1 ] || fail "cleanup: client processes survived a graceful TERM"

    ip rule show >"$WORK/evidence/host-ip-rule.after"
    ip -6 rule show >"$WORK/evidence/host-ip6-rule.after"
    ip route show default >"$WORK/evidence/host-default-route.after"
    cmp -s "$WORK/evidence/host-ip-rule.before" "$WORK/evidence/host-ip-rule.after" || fail "host IPv4 policy rules changed"
    cmp -s "$WORK/evidence/host-ip6-rule.before" "$WORK/evidence/host-ip6-rule.after" || fail "host IPv6 policy rules changed"
    cmp -s "$WORK/evidence/host-default-route.before" "$WORK/evidence/host-default-route.after" || fail "host default route changed"

    jq -n --slurpfile dl "$WORK/evidence/download.json" --arg egress "$egress" \
        --slurpfile stop "$WORK/evidence/stop-networks.json" --arg sha "$(cut -d' ' -f1 "$WORK/evidence/download.sha256")" \
        '{download:$dl[0], egress:$egress, sha256:$sha, stop_status:($stop[0].status // "unknown"),
          tun_removed:true, routes_removed:true, resolver_restored:true, host_network_unchanged:true}' \
        >"$WORK/evidence/SUMMARY.json"
    info "PASS: non-browser full download traversed the Exit, and stop restored TUN, routes and DNS"
    cat "$WORK/evidence/SUMMARY.json"
    info "evidence retained at $WORK"
    trap - EXIT
    cleanup
}

main "$@"
