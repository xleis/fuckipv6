#!/usr/bin/env bash
#
# Run a W12 network-namespace acceptance script without host root.
#
# The whole topology lives in an unprivileged user namespace: a child netns gets
# an internet uplink from slirp4netns, /var/run/netns and /etc/netns are private
# tmpfs mounts, and the wrapped script keeps CAP_NET_ADMIN inside its own
# namespaces only. Host routes, host DNS, host firewall and the host default
# route are never touched.
#
# Usage:
#   scripts/w12-rootless-netns.sh bash scripts/test-w12-exit-download-wan.sh

set -Eeuo pipefail

if [ "$#" -eq 0 ]; then
    printf 'usage: %s COMMAND [ARG...]\n' "${BASH_SOURCE[0]}" >&2
    exit 2
fi

if [ "$(id -u)" -eq 0 ]; then
    exec "$@"
fi

[ -e /dev/net/tun ] || { echo "[FAIL] /dev/net/tun is unavailable" >&2; exit 1; }
command -v slirp4netns >/dev/null || { echo "[FAIL] slirp4netns is required for a rootless uplink" >&2; exit 1; }

exec unshare -Urm --propagation private bash -c '
    set -Eeuo pipefail
    # /run stays host-owned, so the node cannot create its DNS lock there; give
    # the namespace its own runtime tree before any node starts.
    mount -t tmpfs tmpfs /run
    mkdir -p /run/netns /run/user /etc/netns /tmp/.X11-unix
    printf "nameserver 10.0.2.3\noptions edns0\n" >/run/w12-resolv.conf
    mount --bind /run/w12-resolv.conf /etc/resolv.conf
    # The Exit DNS guard rewrites resolv.conf and, when it uses the resolved
    # backend, nsswitch.conf. Give it owned copies so the namespace does not need
    # write access to the host /etc.
    cp /etc/nsswitch.conf /run/w12-nsswitch.conf
    mount --bind /run/w12-nsswitch.conf /etc/nsswitch.conf
    mount -t tmpfs tmpfs /var/run/netns
    mount -t tmpfs tmpfs /etc/netns
    # Never signal by uid inside a user namespace: parent-uid 1000 processes map
    # to uid 0 here, so a uid-filtered kill would hit the calling session.
    ready=0
    child=""
    slirp=""
    for attempt in 1 2 3; do
        unshare -n -- sleep 86400 </dev/null >/dev/null 2>&1 & child=$!
        # slirp must never observe the child before it actually owns its own
        # netns: setns into the parent netns is rejected, and that race is the
        # only reason this step used to fail intermittently.
        switched=0
        for _ in $(seq 1 200); do
            if [ "$(readlink "/proc/$child/ns/net" 2>/dev/null)" != "$(readlink /proc/self/ns/net)" ]; then
                switched=1
                break
            fi
            sleep 0.05
        done
        if [ "$switched" -ne 1 ]; then
            kill "$child" 2>/dev/null || true
            continue
        fi
        slirp4netns --configure --mtu=1500 --disable-host-loopback "$child" tap0 \
            </dev/null >"/tmp/w12-slirp.log" 2>&1 &
        slirp=$!
        for _ in $(seq 1 40); do
            if ! kill -0 "$child" 2>/dev/null; then break; fi
            if nsenter -t "$child" -n -- ip -br addr show tap0 2>/dev/null | grep -q 10.0.2.100; then
                ready=1
                break
            fi
            sleep 0.25
        done
        if [ "$ready" -eq 1 ]; then break; fi
        kill "$slirp" "$child" 2>/dev/null || true
        sleep 1
    done
    if [ "$ready" -ne 1 ]; then
        echo "[FAIL] the rootless slirp uplink did not appear" >&2
        cat /tmp/w12-slirp.log >&2 || true
        kill "$slirp" "$child" 2>/dev/null || true
        exit 1
    fi
    trap "kill $slirp $child 2>/dev/null || true" EXIT INT TERM
    nsenter -t "$child" -n -- "$@"
' w12-rootless "$@"
