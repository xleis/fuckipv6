#!/usr/bin/env bash
# Client-side Phase 4 harness: temporary SYN+ACK drop, control timeout, UDP rescue.
# The pre-shared key is read from stdin (one line), never from argv.
set -u
cd /root/lxt_public_synack_v4 || exit 1
MODE=${1:-full}
COUNT=${2:-6}
read -r LXT_LAB_KEY
if [ "${#LXT_LAB_KEY}" -ne 64 ]; then
  echo "[fatal] bad key length ${#LXT_LAB_KEY}"
  exit 1
fi
export LXT_LAB_KEY
DROP=./client_drop_nft.sh

cleanup() {
  if [ -f /tmp/lxt-client.pid ]; then
    kill "$(cat /tmp/lxt-client.pid)" 2>/dev/null || true
    rm -f /tmp/lxt-client.pid
  fi
  "$DROP" stop >/dev/null 2>&1 || true
}
trap cleanup EXIT INT TERM HUP

# Stop any sidecar from an earlier run and make sure no drop table is left.
pkill -f '^python3 lxt_public_lab.py client-sidecar' 2>/dev/null || true
sleep 1
"$DROP" stop >/dev/null 2>&1 || true

if [ "$MODE" = full ]; then
  echo "== CONTROL: no sidecar, drop active"
  timeout 12 tcpdump -ni eth0 -S -tttt -c 8 \
    'tcp port 39090 and tcp[tcpflags] & (tcp-syn|tcp-ack) == (tcp-syn|tcp-ack)' \
    >/tmp/td-control.txt 2>&1 </dev/null &
  TDPID=$!
  "$DROP" start 185.217.110.15 eth0 600
  timeout 15 python3 lxt_public_lab.py probe --server 185.217.110.15 \
    --count 1 --timeout 3 --retries 0
  RC=$?
  echo "control_exit=$RC"
  sleep 1
  kill "$TDPID" 2>/dev/null || true
  wait "$TDPID" 2>/dev/null || true
  echo "== nft counter (control)"
  "$DROP" status
  echo "== capture (control, eth0)"
  cat /tmp/td-control.txt
else
  "$DROP" start 185.217.110.15 eth0 600
fi

echo "== START SIDECAR"
( exec env LXT_LAB_KEY="$LXT_LAB_KEY" python3 lxt_public_lab.py client-sidecar \
    --interface eth0 --server 185.217.110.15 --tcp-port 39090 --udp-port 39091 ) \
  >/tmp/lxt-client.log 2>&1 </dev/null &
echo $! >/tmp/lxt-client.pid
sleep 2
cat /tmp/lxt-client.log

echo "== RESCUE: drop active, sidecar up, count=$COUNT"
timeout 20 tcpdump -ni eth0 -S -tttt -c 12 \
  'tcp port 39090 and tcp[tcpflags] & (tcp-syn|tcp-ack) == (tcp-syn|tcp-ack)' \
  >/tmp/td-rescue-eth0.txt 2>&1 </dev/null &
TD1=$!
timeout 20 tcpdump -ni lxtrescue0 -S -tttt -c 6 'tcp port 39090' \
  >/tmp/td-rescue-tun.txt 2>&1 </dev/null &
TD2=$!
timeout 60 python3 lxt_public_lab.py probe --server 185.217.110.15 \
  --count "$COUNT" --timeout 4
RC2=$?
echo "rescue_exit=$RC2"
sleep 1
kill "$TD1" "$TD2" 2>/dev/null || true
wait "$TD1" 2>/dev/null || true
wait "$TD2" 2>/dev/null || true
echo "== nft counter (rescue)"
"$DROP" status
echo "== capture (rescue, eth0)"
cat /tmp/td-rescue-eth0.txt
echo "== capture (rescue, TUN)"
cat /tmp/td-rescue-tun.txt
echo "== client sidecar tail"
tail -30 /tmp/lxt-client.log

cleanup
trap - EXIT INT TERM HUP
echo "== POST-CLEANUP"
"$DROP" status
ip -br link show lxtrescue0 2>/dev/null || echo "no tun"
pgrep -af '^python3 lxt_public_lab.py client-sidecar' || echo "no sidecar"
