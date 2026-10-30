#!/usr/bin/env bash
# Client-side only. A tightly scoped, temporary inbound SYN+ACK drop rule.
# NEVER run this on the remote server; do not run over SSH on a remote machine.
set -euo pipefail
if [[ $# -lt 3 ]]; then
  echo "Usage: sudo ./client_drop_once.sh SERVER_PUBLIC_IP CLIENT_NIC COMMAND [ARGS ...]" >&2
  echo "Example: sudo ./client_drop_once.sh 203.0.113.10 enp0s5 python3 lxt_public_lab.py probe --server 203.0.113.10" >&2
  exit 2
fi
if [[ $EUID -ne 0 ]]; then echo "[error] root required" >&2; exit 1; fi
SERVER_IP="$1"; CLIENT_NIC="$2"; shift 2
python3 - "$SERVER_IP" <<'PY'
import ipaddress,sys
try:
    a=ipaddress.IPv4Address(sys.argv[1]); assert not a.is_multicast and not a.is_unspecified
except (ValueError,AssertionError):
    sys.exit('invalid server IPv4')
PY
ip link show dev "$CLIENT_NIC" >/dev/null
if [[ "$CLIENT_NIC" == "lo" ]]; then echo "[error] loopback not allowed" >&2; exit 1; fi
RULE=(-i "$CLIENT_NIC" -s "$SERVER_IP" -p tcp --sport 39090 --tcp-flags SYN,ACK SYN,ACK -j DROP)
# A unique comment lets this exact rule be identified for cleanup.
TOKEN="lxt-v4-$$"
RULE+=( -m comment --comment "$TOKEN" )
cleanup() {
  iptables -D INPUT "${RULE[@]}" 2>/dev/null || true
  echo "[cleanup] removed only our experimental SYN-ACK DROP rule ($TOKEN)" >&2
}
trap cleanup EXIT INT TERM
iptables -I INPUT 1 "${RULE[@]}"
echo "[drop active] interface=$CLIENT_NIC, source=$SERVER_IP, TCP source-port=39090, SYN+ACK only; auto-cleanup on exit" >&2
"$@"
