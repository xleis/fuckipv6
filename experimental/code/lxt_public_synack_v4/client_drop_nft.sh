#!/usr/bin/env bash
# Client-side only. Temporary nftables SYN+ACK drop for the LXT V4 public lab.
# This host has no iptables; nft is the native backend. The rule is scoped to
# one inbound interface, one source IPv4, TCP source port 39090, SYN+ACK only.
# It creates only its own table (inet lxt_v4) and removes only that table.
set -euo pipefail
TABLE=lxt_v4

usage() {
  echo "Usage: $0 start SERVER_IP NIC [TTL] | stop | status" >&2
  exit 2
}

[ $# -ge 1 ] || usage
cmd=$1; shift
case "$cmd" in
  start)
    [ $# -ge 2 ] || usage
    SERVER_IP=$1; NIC=$2; TTL=${3:-600}
    [ "$(id -u)" -eq 0 ] || { echo "[error] root required" >&2; exit 1; }
    python3 - "$SERVER_IP" <<'PY'
import ipaddress, sys
try:
    a = ipaddress.IPv4Address(sys.argv[1])
    assert not a.is_multicast and not a.is_unspecified
except (ValueError, AssertionError):
    sys.exit('invalid server IPv4')
PY
    ip link show dev "$NIC" >/dev/null
    [ "$NIC" != "lo" ] || { echo "[error] loopback not allowed" >&2; exit 1; }
    if nft list table inet "$TABLE" >/dev/null 2>&1; then
      echo "[error] table inet $TABLE already exists; run stop first" >&2
      exit 1
    fi
    nft add table inet "$TABLE"
    nft add chain inet "$TABLE" input '{ type filter hook input priority 0; policy accept; }'
    nft add rule inet "$TABLE" input iifname "$NIC" ip saddr "$SERVER_IP" \
      tcp sport 39090 tcp flags \& \(syn\|ack\) == \(syn\|ack\) counter drop
    # Safety net: remove the table after TTL even if the test shell dies.
    setsid sh -c "sleep $TTL; nft delete table inet $TABLE" >/dev/null 2>&1 </dev/null &
    echo $! > /tmp/lxt-drop-timer.pid
    echo "[drop active] table=inet $TABLE iface=$NIC src=$SERVER_IP tcp sport=39090 flags=syn+ack ttl=${TTL}s"
    ;;
  stop)
    if [ -f /tmp/lxt-drop-timer.pid ]; then
      kill "$(cat /tmp/lxt-drop-timer.pid)" 2>/dev/null || true
      rm -f /tmp/lxt-drop-timer.pid
    fi
    nft delete table inet "$TABLE" 2>/dev/null || true
    echo "[drop removed] table=inet $TABLE (only our table)"
    ;;
  status)
    nft list table inet "$TABLE" 2>/dev/null || echo "[drop absent] table=inet $TABLE"
    ;;
  *)
    usage
    ;;
esac
