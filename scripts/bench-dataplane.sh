#!/usr/bin/env bash
#
# Reproducible Linux network-namespace data-plane benchmark for fuckipv6,
# kernel WireGuard, and wireguard-go. The script never changes global sysctls.
#
# Run as root after building release binaries:
#   cargo build --release -p fuckipv6-client -p fuckipv6-coordinator
#   sudo scripts/bench-dataplane.sh --impls "fuckipv6 wireguard-kernel"
#
# wireguard-go and iperf3 are required only when selected. Results include raw
# JSON, perf counters, process logs, metadata, JSONL records, and median CSV.

set -Eeuo pipefail

PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
FUCKIPV6_BIN="${FUCKIPV6_BIN:-$PROJECT_ROOT/target/release/fuckipv6}"
COORDINATOR_BIN="${COORDINATOR_BIN:-$PROJECT_ROOT/target/release/fuckipv6-coordinator}"
WG_GO_BIN="${WG_GO_BIN:-$(command -v wireguard-go || true)}"
SUMMARIZER="$PROJECT_ROOT/scripts/bench-summarize.py"
IMPLS="fuckipv6 wireguard-kernel"
RUNS=3
DURATION=10
OMIT=0
MTU=1420
UDP_1420_RATE="50M"
UDP_64_RATE="5M"
USE_PERF=1
OUTPUT_DIR=""
CLIENT_CPUS=""
SERVER_CPUS=""
CONTROL_CPUS=""

usage() {
    cat <<'EOF'
Usage: sudo scripts/bench-dataplane.sh [options]

Options:
  --impls "LIST"       Space-separated: fuckipv6, wireguard-kernel, wireguard-go, all
  --runs N             Repetitions per test (default: 3)
  --duration SECONDS   iperf3 measurement duration (default: 10)
  --omit SECONDS       Initial iperf3 warm-up omitted from results (default: 0)
  --mtu BYTES          TUN interface MTU (default: 1420)
  --udp-1420-rate RATE Controlled UDP 1420-byte offered rate (default: 50M)
  --udp-64-rate RATE   Controlled UDP 64-byte offered rate (default: 5M)
  --client-cpus LIST   Pin client-side processes with taskset (for example: 2 or 2-3)
  --server-cpus LIST   Pin server-side processes with taskset (for example: 3 or 3-4)
  --control-cpus LIST  Pin the coordinator with taskset (for example: 0)
  --output DIR         Result directory (default: target/benchmarks/dataplane-TIMESTAMP)
  --no-perf            Skip system-wide CPU/perf counters
  -h, --help           Show this help

Environment:
  FUCKIPV6_BIN         Release client binary
  COORDINATOR_BIN      Release coordinator binary
  WG_GO_BIN            wireguard-go binary (required for wireguard-go)
EOF
}

info() {
    printf '[INFO] %s\n' "$*"
}

fail() {
    printf '[FAIL] %s\n' "$*" >&2
    exit 1
}

while [ "$#" -gt 0 ]; do
    case "$1" in
        --impls)
            IMPLS="${2:?--impls requires a value}"
            shift 2
            ;;
        --runs)
            RUNS="${2:?--runs requires a value}"
            shift 2
            ;;
        --duration)
            DURATION="${2:?--duration requires a value}"
            shift 2
            ;;
        --omit)
            OMIT="${2:?--omit requires a value}"
            shift 2
            ;;
        --mtu)
            MTU="${2:?--mtu requires a value}"
            shift 2
            ;;
        --udp-1420-rate)
            UDP_1420_RATE="${2:?--udp-1420-rate requires a value}"
            shift 2
            ;;
        --udp-64-rate)
            UDP_64_RATE="${2:?--udp-64-rate requires a value}"
            shift 2
            ;;
        --client-cpus)
            CLIENT_CPUS="${2:?--client-cpus requires a value}"
            shift 2
            ;;
        --server-cpus)
            SERVER_CPUS="${2:?--server-cpus requires a value}"
            shift 2
            ;;
        --control-cpus)
            CONTROL_CPUS="${2:?--control-cpus requires a value}"
            shift 2
            ;;
        --output)
            OUTPUT_DIR="${2:?--output requires a value}"
            shift 2
            ;;
        --no-perf)
            USE_PERF=0
            shift
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            fail "Unknown argument: $1"
            ;;
    esac
done

[[ "$RUNS" =~ ^[1-9][0-9]*$ ]] || fail "--runs must be a positive integer"
[[ "$DURATION" =~ ^[1-9][0-9]*$ ]] || fail "--duration must be a positive integer"
[[ "$OMIT" =~ ^[0-9]+$ ]] || fail "--omit must be a non-negative integer"
[[ "$MTU" =~ ^[1-9][0-9]*$ ]] || fail "--mtu must be a positive integer"
[[ "$UDP_1420_RATE" =~ ^[1-9][0-9]*[KMG]?$ ]] \
    || fail "--udp-1420-rate must look like 100M, 1G, or 500K"
[[ "$UDP_64_RATE" =~ ^[1-9][0-9]*[KMG]?$ ]] \
    || fail "--udp-64-rate must look like 100M, 1G, or 500K"
(( DURATION > OMIT + 1 )) || fail "--duration must exceed --omit by at least two seconds"

if [ "$IMPLS" = "all" ]; then
    IMPLS="fuckipv6 wireguard-kernel wireguard-go"
fi
read -r -a IMPL_LIST <<<"$IMPLS"
[ "${#IMPL_LIST[@]}" -gt 0 ] || fail "No implementation selected"
for impl in "${IMPL_LIST[@]}"; do
    case "$impl" in
        fuckipv6|wireguard-kernel|wireguard-go) ;;
        *) fail "Unsupported implementation: $impl" ;;
    esac
done

[ "$(id -u)" -eq 0 ] || fail "Run as root: sudo $0 $*"

require_command() {
    command -v "$1" >/dev/null 2>&1 || fail "Missing required command: $1"
}

for command in ip iperf3 ping python3 wg; do
    require_command "$command"
done
if [ -n "$CLIENT_CPUS$SERVER_CPUS$CONTROL_CPUS" ]; then
    require_command taskset
fi
[ -z "$CLIENT_CPUS" ] || taskset -c "$CLIENT_CPUS" true \
    || fail "Invalid --client-cpus value: $CLIENT_CPUS"
[ -z "$SERVER_CPUS" ] || taskset -c "$SERVER_CPUS" true \
    || fail "Invalid --server-cpus value: $SERVER_CPUS"
[ -z "$CONTROL_CPUS" ] || taskset -c "$CONTROL_CPUS" true \
    || fail "Invalid --control-cpus value: $CONTROL_CPUS"
[ -x "$SUMMARIZER" ] || fail "Summarizer is not executable: $SUMMARIZER"
[ -x "$FUCKIPV6_BIN" ] || fail "Missing release client: $FUCKIPV6_BIN (build it first)"
[ -x "$COORDINATOR_BIN" ] || fail "Missing release coordinator: $COORDINATOR_BIN (build it first)"
if printf '%s\n' "${IMPL_LIST[@]}" | grep -qx 'wireguard-go'; then
    [ -n "$WG_GO_BIN" ] && [ -x "$WG_GO_BIN" ] \
        || fail "wireguard-go not found; set WG_GO_BIN or install wireguard-go"
fi

if [ "$USE_PERF" -eq 1 ]; then
    require_command perf
    if ! perf stat -a -e task-clock -- true >/dev/null 2>&1; then
        fail "perf is unavailable; rerun with --no-perf to collect throughput only"
    fi
fi

TIMESTAMP="$(date -u +%Y%m%dT%H%M%SZ)"
OUTPUT_DIR="${OUTPUT_DIR:-$PROJECT_ROOT/target/benchmarks/dataplane-$TIMESTAMP}"
RAW_DIR="$OUTPUT_DIR/raw"
LOG_DIR="$OUTPUT_DIR/logs"
WORK_DIR="$OUTPUT_DIR/work"
RESULTS_JSONL="$OUTPUT_DIR/results.jsonl"
SUMMARY_CSV="$OUTPUT_DIR/summary.csv"
mkdir -p "$RAW_DIR" "$LOG_DIR" "$WORK_DIR/secrets"
chmod 700 "$WORK_DIR" "$WORK_DIR/secrets"
[ ! -e "$RESULTS_JSONL" ] || fail "Result file already exists: $RESULTS_JSONL"

SUFFIX="$(printf '%04d' "$(( $$ % 10000 ))")"
BRIDGE="fip6b$SUFFIX"
NS1="fip6n1$SUFFIX"
NS2="fip6n2$SUFFIX"
HOST_VETH1="fip6h1$SUFFIX"
HOST_VETH2="fip6h2$SUFFIX"
UNDERLAY_OCTET="$(( $$ % 180 + 20 ))"
UNDERLAY_NET="10.213.${UNDERLAY_OCTET}.0/24"
HOST_IP="10.213.${UNDERLAY_OCTET}.1"
NS1_IP="10.213.${UNDERLAY_OCTET}.11"
NS2_IP="10.213.${UNDERLAY_OCTET}.12"
VIRTUAL_IP1="fd00::1"
VIRTUAL_IP2="fd00::2"
CLIENT_IF="fip6"
WG_IF1="fipwg1$SUFFIX"
WG_IF2="fipwg2$SUFFIX"

READY=0
CLIENT_PIDS=()
OTHER_PIDS=()
CLIENT_AFFINITY=()
SERVER_AFFINITY=()
CONTROL_AFFINITY=()
[ -z "$CLIENT_CPUS" ] || CLIENT_AFFINITY=(taskset -c "$CLIENT_CPUS")
[ -z "$SERVER_CPUS" ] || SERVER_AFFINITY=(taskset -c "$SERVER_CPUS")
[ -z "$CONTROL_CPUS" ] || CONTROL_AFFINITY=(taskset -c "$CONTROL_CPUS")

read -r SIGNALING_PORT RELAY_PORT WEBUI_PORT IPERF_PORT < <(
    python3 - <<'PY'
import socket

sockets = []
try:
    for _ in range(4):
        sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        sock.bind(("", 0))
        sockets.append(sock)
    print(*(sock.getsockname()[1] for sock in sockets))
finally:
    for sock in sockets:
        sock.close()
PY
)

cleanup() {
    set +e
    trap - EXIT INT TERM
    for pid in "${CLIENT_PIDS[@]:-}"; do
        [ -n "$pid" ] && kill "$pid" 2>/dev/null
    done
    for pid in "${OTHER_PIDS[@]:-}"; do
        [ -n "$pid" ] && kill "$pid" 2>/dev/null
    done
    [ -n "${COORDINATOR_PID:-}" ] && kill "$COORDINATOR_PID" 2>/dev/null

    for pid in "${CLIENT_PIDS[@]:-}" "${OTHER_PIDS[@]:-}" "${COORDINATOR_PID:-}"; do
        [ -n "$pid" ] && wait "$pid" 2>/dev/null
    done

    if [ "$READY" -eq 1 ]; then
        ip netns delete "$NS1" 2>/dev/null
        ip netns delete "$NS2" 2>/dev/null
        ip link delete "$BRIDGE" 2>/dev/null
    fi
    rm -rf "$WORK_DIR"
    if [ -n "${SUDO_UID:-}" ] && [ -n "${SUDO_GID:-}" ]; then
        chown -R "$SUDO_UID:$SUDO_GID" "$OUTPUT_DIR" 2>/dev/null
    fi
}
trap cleanup EXIT INT TERM

setup_topology() {
    ip link show "$BRIDGE" >/dev/null 2>&1 && fail "Interface already exists: $BRIDGE"
    ip netns list | awk '{print $1}' | grep -qx "$NS1" && fail "Namespace exists: $NS1"
    ip netns list | awk '{print $1}' | grep -qx "$NS2" && fail "Namespace exists: $NS2"

    ip link add "$BRIDGE" type bridge
    ip addr add "$HOST_IP/24" dev "$BRIDGE"
    ip link set "$BRIDGE" up
    READY=1

    ip netns add "$NS1"
    ip link add "$HOST_VETH1" type veth peer name eth0
    ip link set eth0 netns "$NS1"
    ip link set "$HOST_VETH1" master "$BRIDGE"
    ip link set "$HOST_VETH1" up
    ip netns exec "$NS1" ip link set lo up
    ip netns exec "$NS1" ip addr add "$NS1_IP/24" dev eth0
    ip netns exec "$NS1" ip link set eth0 up

    ip netns add "$NS2"
    ip link add "$HOST_VETH2" type veth peer name eth0
    ip link set eth0 netns "$NS2"
    ip link set "$HOST_VETH2" master "$BRIDGE"
    ip link set "$HOST_VETH2" up
    ip netns exec "$NS2" ip link set lo up
    ip netns exec "$NS2" ip addr add "$NS2_IP/24" dev eth0
    ip netns exec "$NS2" ip link set eth0 up

    info "Underlay: $UNDERLAY_NET ($NS1_IP <-> $NS2_IP)"
}

write_client_config() {
    local ns="$1"
    local p2p_port="$2"
    local config="$WORK_DIR/${ns}.toml"
    cat >"$config" <<EOF
coordinator_url = "ws://${HOST_IP}:${SIGNALING_PORT}"
private_key_path = "${WORK_DIR}/secrets/${ns}.pem"

[network]
virtual_network = "fd00::/64"

[tun]
name = "${CLIENT_IF}"
mtu = ${MTU}

[p2p]
listen_port = ${p2p_port}
connect_timeout = 10
heartbeat_interval = 30

[p2p.nat_detection]
enabled = false

[log]
level = "info"
EOF
}

wait_for_tun_ip() {
    local ns="$1"
    local interface="$2"
    local address=""
    for _ in $(seq 1 40); do
        address="$(ip netns exec "$ns" ip -6 -o addr show dev "$interface" scope global 2>/dev/null \
            | awk '{print $4}' | cut -d/ -f1 | head -n1 || true)"
        if [ -n "$address" ]; then
            printf '%s' "$address"
            return 0
        fi
        sleep 0.5
    done
    return 1
}

wait_for_ping() {
    local source_ns="$1"
    local destination="$2"
    for _ in $(seq 1 20); do
        if ip netns exec "$source_ns" ping -6 -c 1 -W 1 "$destination" >/dev/null 2>&1; then
            return 0
        fi
        sleep 0.5
    done
    return 1
}

start_fuckipv6() {
    local coordinator_log="$LOG_DIR/fuckipv6-coordinator.log"
    "${CONTROL_AFFINITY[@]}" env RUST_LOG=info "$COORDINATOR_BIN" \
        --bind "0.0.0.0:${SIGNALING_PORT}" \
        --relay-bind "0.0.0.0:${RELAY_PORT}" \
        --webui-bind "127.0.0.1:${WEBUI_PORT}" \
        --ip-pool "fd00::1000/120" \
        >"$coordinator_log" 2>&1 &
    COORDINATOR_PID=$!
    OTHER_PIDS+=("$COORDINATOR_PID")
    sleep 2
    kill -0 "$COORDINATOR_PID" 2>/dev/null || fail "Coordinator failed to start"

    write_client_config "$NS1" "$(( IPERF_PORT + 1 ))"
    write_client_config "$NS2" "$(( IPERF_PORT + 2 ))"

    ip netns exec "$NS1" "${CLIENT_AFFINITY[@]}" env RUST_LOG=info "$FUCKIPV6_BIN" \
        start --config "$WORK_DIR/${NS1}.toml" >"$LOG_DIR/$NS1.log" 2>&1 &
    CLIENT_PIDS+=("$!")
    ip netns exec "$NS2" "${SERVER_AFFINITY[@]}" env RUST_LOG=info "$FUCKIPV6_BIN" \
        start --config "$WORK_DIR/${NS2}.toml" >"$LOG_DIR/$NS2.log" 2>&1 &
    CLIENT_PIDS+=("$!")

    VIRTUAL_IP1="$(wait_for_tun_ip "$NS1" "$CLIENT_IF")" \
        || fail "fuckipv6 client 1 did not receive a virtual IP"
    VIRTUAL_IP2="$(wait_for_tun_ip "$NS2" "$CLIENT_IF")" \
        || fail "fuckipv6 client 2 did not receive a virtual IP"
    wait_for_ping "$NS1" "$VIRTUAL_IP2" \
        || fail "fuckipv6 virtual tunnel did not become reachable"
    info "fuckipv6 tunnel: $VIRTUAL_IP1 -> $VIRTUAL_IP2"
}

generate_wg_keys() {
    local name="$1"
    local private_key
    private_key="$(wg genkey)"
    (umask 077; printf '%s\n' "$private_key" >"$WORK_DIR/secrets/${name}.key")
    wg pubkey <"$WORK_DIR/secrets/${name}.key" >"$WORK_DIR/secrets/${name}.pub"
}

configure_wg_interface() {
    local ns="$1"
    local interface="$2"
    local local_ip="$3"
    local local_port="$4"
    local peer_port="$5"
    local peer_public="$6"
    local peer_ip="$7"
    local peer_endpoint="$8"

    ip netns exec "$ns" ip addr add "$local_ip/64" dev "$interface"
    ip netns exec "$ns" ip link set dev "$interface" mtu "$MTU" up
    ip netns exec "$ns" wg set "$interface" \
        private-key "$WORK_DIR/secrets/${ns}.key" \
        listen-port "$local_port" \
        peer "$peer_public" \
        allowed-ips "$peer_ip/128" \
        endpoint "$peer_endpoint:$peer_port"
}

start_wireguard_kernel() {
    generate_wg_keys "$NS1"
    generate_wg_keys "$NS2"
    local pub1 pub2
    pub1="$(cat "$WORK_DIR/secrets/${NS1}.pub")"
    pub2="$(cat "$WORK_DIR/secrets/${NS2}.pub")"
    local wg_port1=$(( IPERF_PORT + 10 ))
    local wg_port2=$(( IPERF_PORT + 11 ))

    ip netns exec "$NS1" ip link add "$WG_IF1" type wireguard
    ip netns exec "$NS2" ip link add "$WG_IF2" type wireguard
    configure_wg_interface "$NS1" "$WG_IF1" "$VIRTUAL_IP1" "$wg_port1" \
        "$wg_port2" "$pub2" "$VIRTUAL_IP2" "$NS2_IP"
    configure_wg_interface "$NS2" "$WG_IF2" "$VIRTUAL_IP2" "$wg_port2" \
        "$wg_port1" "$pub1" "$VIRTUAL_IP1" "$NS1_IP"

    wait_for_ping "$NS1" "$VIRTUAL_IP2" || fail "kernel WireGuard tunnel is unreachable"
    info "kernel WireGuard tunnel: $VIRTUAL_IP1 -> $VIRTUAL_IP2"
}

start_wireguard_go() {
    generate_wg_keys "$NS1"
    generate_wg_keys "$NS2"
    local pub1 pub2
    pub1="$(cat "$WORK_DIR/secrets/${NS1}.pub")"
    pub2="$(cat "$WORK_DIR/secrets/${NS2}.pub")"
    local wg_port1=$(( IPERF_PORT + 10 ))
    local wg_port2=$(( IPERF_PORT + 11 ))

    ip netns exec "$NS1" "${CLIENT_AFFINITY[@]}" "$WG_GO_BIN" -f "$WG_IF1" \
        >"$LOG_DIR/${NS1}-wireguard-go.log" 2>&1 &
    OTHER_PIDS+=("$!")
    ip netns exec "$NS2" "${SERVER_AFFINITY[@]}" "$WG_GO_BIN" -f "$WG_IF2" \
        >"$LOG_DIR/${NS2}-wireguard-go.log" 2>&1 &
    OTHER_PIDS+=("$!")

    for _ in $(seq 1 40); do
        if ip netns exec "$NS1" ip link show "$WG_IF1" >/dev/null 2>&1 \
            && ip netns exec "$NS2" ip link show "$WG_IF2" >/dev/null 2>&1; then
            break
        fi
        sleep 0.25
    done
    ip netns exec "$NS1" ip link show "$WG_IF1" >/dev/null 2>&1 \
        || fail "wireguard-go failed to create $WG_IF1"
    ip netns exec "$NS2" ip link show "$WG_IF2" >/dev/null 2>&1 \
        || fail "wireguard-go failed to create $WG_IF2"

    configure_wg_interface "$NS1" "$WG_IF1" "$VIRTUAL_IP1" "$wg_port1" \
        "$wg_port2" "$pub2" "$VIRTUAL_IP2" "$NS2_IP"
    configure_wg_interface "$NS2" "$WG_IF2" "$VIRTUAL_IP2" "$wg_port2" \
        "$wg_port1" "$pub1" "$VIRTUAL_IP1" "$NS1_IP"

    wait_for_ping "$NS1" "$VIRTUAL_IP2" || fail "wireguard-go tunnel is unreachable"
    info "wireguard-go tunnel: $VIRTUAL_IP1 -> $VIRTUAL_IP2"
}

start_iperf_server() {
    ip netns exec "$NS2" "${SERVER_AFFINITY[@]}" \
        iperf3 -s -6 -B "$VIRTUAL_IP2" -p "$IPERF_PORT" \
        >"$LOG_DIR/iperf3-server.log" 2>&1 &
    local iperf_pid="$!"
    OTHER_PIDS+=("$iperf_pid")
    sleep 1
    kill -0 "$iperf_pid" 2>/dev/null || fail "iperf3 server failed to start"
}

stop_implementation() {
    local pid
    for pid in "${CLIENT_PIDS[@]:-}"; do
        [ -n "$pid" ] && kill "$pid" 2>/dev/null || true
    done
    for pid in "${OTHER_PIDS[@]:-}"; do
        [ -n "$pid" ] && kill "$pid" 2>/dev/null || true
    done
    [ -n "${COORDINATOR_PID:-}" ] && kill "$COORDINATOR_PID" 2>/dev/null || true
    for pid in "${CLIENT_PIDS[@]:-}" "${OTHER_PIDS[@]:-}" "${COORDINATOR_PID:-}"; do
        [ -n "$pid" ] && wait "$pid" 2>/dev/null || true
    done
    CLIENT_PIDS=()
    OTHER_PIDS=()
    COORDINATOR_PID=""
    ip netns exec "$NS1" ip link delete "$WG_IF1" 2>/dev/null || true
    ip netns exec "$NS2" ip link delete "$WG_IF2" 2>/dev/null || true
    ip netns exec "$NS1" ip link delete "$CLIENT_IF" 2>/dev/null || true
    ip netns exec "$NS2" ip link delete "$CLIENT_IF" 2>/dev/null || true
    sleep 1
}

record_metadata() {
    local dirty="clean"
    if [ -n "$(git -C "$PROJECT_ROOT" status --porcelain 2>/dev/null)" ]; then
        dirty="dirty"
    fi
    python3 - "$OUTPUT_DIR/metadata.json" <<PY
import json
import pathlib
import platform
import subprocess
import sys

def command(*args):
    try:
        return subprocess.check_output(args, text=True, stderr=subprocess.DEVNULL).strip()
    except (OSError, subprocess.CalledProcessError):
        return None

cpu = None
try:
    for line in pathlib.Path('/proc/cpuinfo').read_text().splitlines():
        if line.startswith('model name'):
            cpu = line.split(':', 1)[1].strip()
            break
except OSError:
    pass

metadata = {
    'timestamp_utc': '$TIMESTAMP',
    'git_commit': command('git', '-C', '$PROJECT_ROOT', 'rev-parse', 'HEAD'),
    'git_worktree': '$dirty',
    'kernel': platform.release(),
    'machine': platform.machine(),
    'cpu': cpu,
    'logical_cpus': command('nproc'),
    'test_duration_seconds': int('$DURATION'),
    'omitted_seconds': int('$OMIT'),
    'runs': int('$RUNS'),
    'mtu': int('$MTU'),
    'udp_1420_controlled_rate': '$UDP_1420_RATE',
    'udp_64_controlled_rate': '$UDP_64_RATE',
    'cpu_affinity': {
        'control_cpus': '$CONTROL_CPUS' or None,
        'client_cpus': '$CLIENT_CPUS' or None,
        'server_cpus': '$SERVER_CPUS' or None,
    },
    'underlay_network': '$UNDERLAY_NET',
    'virtual_network': 'fd00::/64',
    'iperf3': command('iperf3', '--version'),
    'wg': command('wg', '--version'),
    'wireguard_go': command('$WG_GO_BIN', '--version') if '$WG_GO_BIN' else None,
    'perf_enabled': bool(int('$USE_PERF')),
    'perf_scope': 'system-wide task-clock/cycles/instructions' if int('$USE_PERF') else None,
    'implementations': '$IMPLS'.split(),
    'fuckipv6_binary_sha256': command('sha256sum', '$FUCKIPV6_BIN'),
    'coordinator_binary_sha256': command('sha256sum', '$COORDINATOR_BIN'),
}
pathlib.Path(sys.argv[1]).write_text(json.dumps(metadata, indent=2, sort_keys=True) + '\n')
PY
    git -C "$PROJECT_ROOT" status --porcelain >"$OUTPUT_DIR/git-status.txt" 2>/dev/null || true
    lscpu >"$OUTPUT_DIR/lscpu.txt" 2>/dev/null || true
}

record_result() {
    local impl="$1"
    local test="$2"
    local run="$3"
    local streams="$4"
    local payload_bytes="$5"
    local workload="$6"
    local offered_rate="$7"
    local cpu_seconds="$8"
    local iperf_json="$RAW_DIR/${impl}__${test}__run${run}.json"
    local perf_csv="$RAW_DIR/${impl}__${test}__run${run}.perf.csv"
    local -a summarize=(python3 "$SUMMARIZER" record
        --iperf-json "$iperf_json"
        --results "$RESULTS_JSONL"
        --impl "$impl"
        --test "$test"
        --run "$run"
        --field "mtu=$MTU"
        --field "streams=$streams"
        --field "payload_bytes=$payload_bytes"
        --field "workload=$workload"
        --field "offered_rate=$offered_rate"
        --field "duration_seconds=$DURATION"
        --field "omit_seconds=$OMIT"
        --field "cpu_seconds=$cpu_seconds"
        --field "client_cpus=$CLIENT_CPUS"
        --field "server_cpus=$SERVER_CPUS"
        --field "control_cpus=$CONTROL_CPUS")
    if [ "$USE_PERF" -eq 1 ]; then
        summarize+=(--perf-csv "$perf_csv")
    fi
    "${summarize[@]}"
}

execute_case() {
    local impl="$1"
    local test="$2"
    local streams="$3"
    local payload_bytes="$4"
    local workload="$5"
    local offered_rate="$6"
    shift 6
    local extra=("$@")
    local run
    for run in $(seq 1 "$RUNS"); do
        local iperf_json="$RAW_DIR/${impl}__${test}__run${run}.json"
        local perf_csv="$RAW_DIR/${impl}__${test}__run${run}.perf.csv"
        local stderr_log="$LOG_DIR/${impl}__${test}__run${run}.stderr"
        local cpu_before cpu_after cpu_seconds
        cpu_before="$(awk '/^cpu / { total=0; for (i=2; i<=9; i++) total+=$i; print total, $5+$6 }' /proc/stat)"
        local cmd=(ip netns exec "$NS1" "${CLIENT_AFFINITY[@]}" iperf3 -6 -J -c "$VIRTUAL_IP2" \
            -p "$IPERF_PORT" -t "$DURATION")
        if [ "$OMIT" -gt 0 ]; then
            cmd+=(-O "$OMIT")
        fi
        cmd+=("${extra[@]}")
        info "Benchmark $impl / $test / run $run"
        if [ "$USE_PERF" -eq 1 ]; then
            if ! timeout "$(( DURATION + OMIT + 25 ))" perf stat -a -x, \
                -e task-clock,cycles,instructions -o "$perf_csv" -- "${cmd[@]}" \
                >"$iperf_json" 2>"$stderr_log"; then
                tail -30 "$stderr_log" >&2 || true
                fail "iperf3/perf failed for $impl/$test/run $run"
            fi
        else
            if ! timeout "$(( DURATION + OMIT + 25 ))" "${cmd[@]}" \
                >"$iperf_json" 2>"$stderr_log"; then
                tail -30 "$stderr_log" >&2 || true
                fail "iperf3 failed for $impl/$test/run $run"
            fi
        fi
        cpu_after="$(awk '/^cpu / { total=0; for (i=2; i<=9; i++) total+=$i; print total, $5+$6 }' /proc/stat)"
        cpu_seconds="$(awk -v before="$cpu_before" -v after="$cpu_after" \
            -v hz="$(getconf CLK_TCK)" 'BEGIN {
                split(before, b, " "); split(after, a, " ");
                total = a[1] - b[1]; idle = a[2] - b[2];
                if (total < 0 || idle < 0) exit 1;
                printf "%.9f", (total - idle) / hz;
            }')"
        record_result "$impl" "$test" "$run" "$streams" "$payload_bytes" \
            "$workload" "$offered_rate" "$cpu_seconds"
    done
}

run_impl() {
    local impl="$1"
    info "Starting implementation: $impl"
    case "$impl" in
        fuckipv6) start_fuckipv6 ;;
        wireguard-kernel) start_wireguard_kernel ;;
        wireguard-go) start_wireguard_go ;;
    esac
    start_iperf_server

    execute_case "$impl" tcp_1flow 1 0 unlimited unlimited -P 1
    execute_case "$impl" tcp_8flow 8 0 unlimited unlimited -P 8
    execute_case "$impl" udp_1420_saturated 1 1420 saturated unlimited \
        -u -b 0 -l 1420
    execute_case "$impl" udp_64_saturated 1 64 saturated unlimited \
        -u -b 0 -l 64
    execute_case "$impl" udp_1420_controlled 1 1420 controlled "$UDP_1420_RATE" \
        -u -b "$UDP_1420_RATE" -l 1420
    execute_case "$impl" udp_64_controlled 1 64 controlled "$UDP_64_RATE" \
        -u -b "$UDP_64_RATE" -l 64

    stop_implementation
}

render_summary() {
    python3 "$SUMMARIZER" aggregate --results "$RESULTS_JSONL" --csv "$SUMMARY_CSV"
    info "Median summary: $SUMMARY_CSV"
    column -s, -t <"$SUMMARY_CSV" 2>/dev/null || cat "$SUMMARY_CSV"
}

record_metadata
setup_topology
for implementation in "${IMPL_LIST[@]}"; do
    run_impl "$implementation"
done
render_summary

python3 - "$RESULTS_JSONL" "${IMPL_LIST[@]}" <<'PY'
import json
import pathlib
import sys

records = [json.loads(line) for line in pathlib.Path(sys.argv[1]).read_text().splitlines()]
actual = {(record['impl'], record['test']) for record in records}
expected = {
    (implementation, test)
    for implementation in sys.argv[2:]
    for test in (
        'tcp_1flow',
        'tcp_8flow',
        'udp_1420_saturated',
        'udp_64_saturated',
        'udp_1420_controlled',
        'udp_64_controlled',
    )
}
missing = sorted(expected - actual)
if missing:
    raise SystemExit(f'Missing benchmark results: {missing}')
PY

info "Raw results: $RAW_DIR"
info "Logs: $LOG_DIR"
info "Metadata: $OUTPUT_DIR/metadata.json"
