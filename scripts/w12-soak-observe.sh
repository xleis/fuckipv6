#!/usr/bin/env bash
#
# Live view of a running W12-09 soak instance.
#
# The soak runs both peers and the Coordinator inside a rootless user namespace,
# so host-side `ip netns`/`ss` show nothing. Everything below is read through
# /proc/<node-pid>/net, which exposes the namespace's counters and socket tables
# without entering it.
#
# Usage: scripts/w12-soak-observe.sh [WORKDIR] [--watch]

set -Eeuo pipefail

WORK="${1:-/tmp/w12-soak-24h-r3}"
[ "${1:-}" = "--watch" ] && WORK="/tmp/w12-soak-24h-r3"
[ "${2:-}" = "--watch" ] && WATCH=1 || WATCH=0
[ "${1:-}" = "--watch" ] && WATCH=1

[ -d "$WORK" ] || { echo "no soak workdir at $WORK" >&2; exit 1; }

decode_addr() {
    # /proc/net/tcp hex: little-endian IPv4 + big-endian port
    local hex=${1%:*} port=${1##*:}
    # /proc/net/tcp prints the IPv4 address as a little-endian u32: reverse the
    # byte pairs, then append the big-endian port.
    printf '%d.%d.%d.%d:%d' \
        $((16#${hex:6:2})) $((16#${hex:4:2})) $((16#${hex:2:2})) $((16#${hex:0:2})) $((16#$port))
}

snapshot() {
    local n1 n2 cycle
    n1=$(cat "$WORK/node1/run/node.lock" 2>/dev/null || echo 0)
    n2=$(cat "$WORK/node2/run/node.lock" 2>/dev/null || echo 0)
    cycle=$(jq -r .cycle "$WORK/soak/run-state.json" 2>/dev/null || echo '?')
    printf 'workdir   : %s\n' "$WORK"
    printf 'cycle     : %s   node1 pid %s   node2 pid %s\n' "$cycle" "$n1" "$n2"
    if [ -r "/proc/$n1/ns/user" ] && [ -r "/proc/$n1/ns/net" ]; then
        printf 'namespace : user %s  net %s\n' "$(readlink "/proc/$n1/ns/user")" "$(readlink "/proc/$n1/ns/net")"
    fi
    printf '\n-- veth and TUN byte counters (node1 namespace, 5 s delta) --\n'
    local before after
    before=$(awk 'NR>2 {print $1, $2, $10}' "/proc/$n1/net/dev" 2>/dev/null || true)
    sleep 5
    after=$(awk 'NR>2 {print $1, $2, $10}' "/proc/$n1/net/dev" 2>/dev/null || true)
    join <(printf '%s\n' "$before") <(printf '%s\n' "$after") 2>/dev/null |
        awk '{printf "%-14s rx_bytes %-12s +%-10s tx_bytes %-12s +%s\n", $1, $2, ($4-$2), $3, ($5-$3)}'
    printf '\n-- established peers (node1 namespace TCP table) --\n'
    awk 'NR>1 && $4=="01" {print $2, $3}' "/proc/$n1/net/tcp" 2>/dev/null |
        while read -r local remote; do
            printf 'local %s  ->  peer %s  (ESTABLISHED)\n' "$(decode_addr "$local")" "$(decode_addr "$remote")"
        done
    printf '\n-- latest soak cycle --\n'
    tail -1 "$WORK/soak/metrics.jsonl" 2>/dev/null | jq -c '{
        cycle, at,
        node1_p2p_healthy: .node1_peer.p2p_healthy,
        node2_p2p_healthy: .node2_peer.p2p_healthy,
        p2p_rx_packets: .node1_peer.traffic.p2p_rx_packets,
        p2p_tx_packets: .node1_peer.traffic.p2p_tx_packets,
        ping_1_to_2_rtt_ms: .ping_1_to_2.avg_rtt_ms,
        ping_2_to_1_rtt_ms: .ping_2_to_1.avg_rtt_ms,
        canary: (.canary_1_to_2 // "not sampled this cycle"),
        node1_rss_kib: .processes.node1.node.rss_kib,
        node1_fds: .processes.node1.node.fd_count,
        runtime_files: .runtime_files.node1.files
    }'
    printf '\n-- soak progress --\n'
    jq -r '"elapsed \(.elapsed_seconds | floor) s / planned \(.planned_seconds | floor) s  (\((.elapsed_seconds/.planned_seconds*100) | floor)%)"' \
        "$WORK/soak/run-state.json" 2>/dev/null || true
}

if [ "$WATCH" = 1 ]; then
    while true; do clear; date; snapshot; sleep 10; done
else
    snapshot
fi
