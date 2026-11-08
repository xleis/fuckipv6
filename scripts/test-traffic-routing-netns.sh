#!/usr/bin/env bash
#
# Privileged Linux acceptance test for W11-09 traffic-policy E2E evidence.
#
# The test runs in private user/network/mount namespaces. Four isolated links
# represent the domain, application, direct-fallback and DNS paths. Real DNS
# answers feed real TCP attempts; destination and socket-UID policy routes
# select the observed source path, nftables rejects unapproved output, and an
# upstream observer records the source address that actually reached it.

set -euo pipefail

PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SKIP_STATUS=77
INNER_ENV="FUCKIPV6_TRAFFIC_E2E_INNER"
TOPOLOGY_ENV="FUCKIPV6_TRAFFIC_E2E_TOPOLOGY"
WORK_ENV="FUCKIPV6_TRAFFIC_E2E_WORK"
REPORT_ENV="FUCKIPV6_TRAFFIC_E2E_REPORT"
OBSERVATIONS_ENV="FUCKIPV6_TRAFFIC_E2E_OBSERVATIONS"
SECOND_UID_ENV="FUCKIPV6_TRAFFIC_E2E_SECOND_UID"
UPSTREAM_NS_ENV="FUCKIPV6_TRAFFIC_E2E_UPSTREAM_NS"
KEEP_ENV="FUCKIPV6_TRAFFIC_E2E_KEEP"
UPSTREAM_NS="fip6traffic-up"
SECOND_UID=100000
DNS_ADDRESS="10.40.0.1"

info() {
    printf '[INFO] %s\n' "$*"
}

fail() {
    printf '[FAIL] %s\n' "$*" >&2
    exit 1
}

skip() {
    printf '[SKIP] %s\n' "$*" >&2
    exit "$SKIP_STATUS"
}

require_tools() {
    for tool in cargo ip mount nft python3 unshare; do
        if ! command -v "$tool" >/dev/null 2>&1; then
            fail "missing required tool: $tool"
        fi
    done
}

subid_start() {
    local path="$1" identity="$2"
    awk -F: -v identity="$identity" '$1 == identity && $3 > 0 { print $2; exit }' "$path"
}

enter_isolated_namespaces() {
    if [ "$(id -u)" -eq 0 ]; then
        exec unshare --net --mount --fork --propagation private -- "$0" "$@"
    fi

    local uid gid user subuid subgid
    uid="$(id -u)"
    gid="$(id -g)"
    user="$(id -un)"
    subuid="$(subid_start /etc/subuid "$user")"
    subgid="$(subid_start /etc/subgid "$user")"
    if [ -z "$subuid" ] || [ -z "$subgid" ]; then
        skip "a subordinate UID/GID is required for real two-application UID classification"
    fi

    exec unshare \
        --user \
        --map-users="0:${uid}:1" \
        --map-users="${SECOND_UID}:${subuid}:1" \
        --map-groups="0:${gid}:1" \
        --map-groups="${SECOND_UID}:${subgid}:1" \
        --net \
        --mount \
        --fork \
        --propagation private \
        -- "$0" "$@"
}

cleanup() {
    set +e
    if [ -n "${PEER_PID:-}" ]; then
        kill "$PEER_PID" >/dev/null 2>&1 || true
        wait "$PEER_PID" >/dev/null 2>&1 || true
    fi
    ip netns delete "$UPSTREAM_NS" >/dev/null 2>&1 || true
    for link in fip6dom fip6app fip6raw fip6dns; do
        ip link delete "$link" >/dev/null 2>&1 || true
    done
    nft delete table ip fip6e2e >/dev/null 2>&1 || true
    ip rule del uidrange "${SECOND_UID}-${SECOND_UID}" lookup 110 >/dev/null 2>&1 || true
    ip rule del to 198.51.100.20/32 lookup 120 >/dev/null 2>&1 || true
    ip rule del fwmark 0x10 lookup 110 >/dev/null 2>&1 || true
    ip rule del fwmark 0x20 lookup 120 >/dev/null 2>&1 || true
    ip rule del fwmark 0x30 lookup 130 >/dev/null 2>&1 || true
    if [ -n "${FUCKIPV6_TRAFFIC_E2E_WORK:-}" ]; then
        if [ "${!KEEP_ENV:-0}" = "1" ]; then
            info "preserving traffic-routing E2E evidence at ${FUCKIPV6_TRAFFIC_E2E_WORK}"
        else
            rm -rf "$FUCKIPV6_TRAFFIC_E2E_WORK"
        fi
    fi
}

prepare_links() {
    ip netns add "$UPSTREAM_NS"
    ip netns exec "$UPSTREAM_NS" ip link set lo up
    for address in 198.51.100.20 203.0.113.30 203.0.113.40 203.0.113.50; do
        ip netns exec "$UPSTREAM_NS" ip addr add "${address}/32" dev lo
    done

    local link peer link_address upstream_address
    while read -r link link_address upstream_address; do
        peer="${link}up"
        ip link add "$link" type veth peer name "$peer"
        ip link set "$peer" netns "$UPSTREAM_NS"
        ip addr add "$link_address" dev "$link"
        ip link set "$link" up
        ip netns exec "$UPSTREAM_NS" ip addr add "$upstream_address" dev "$peer"
        ip netns exec "$UPSTREAM_NS" ip link set "$peer" up
    done <<'NETWORKS'
fip6dom 10.10.0.2/24 10.10.0.1/24
fip6app 10.20.0.2/24 10.20.0.1/24
fip6raw 10.30.0.2/24 10.30.0.1/24
fip6dns 10.40.0.2/24 10.40.0.1/24
NETWORKS

    # The main-table route is deliberately a real direct path. A failed
    # policy-route lookup must never silently fall through to this source.
    ip route replace default via 10.30.0.1 dev fip6raw
}

prepare_resolver() {
    local resolv_conf="${!WORK_ENV}/resolv.conf"
    printf 'nameserver %s\noptions timeout:1 attempts:1\n' "$DNS_ADDRESS" >"$resolv_conf"
    mount --bind "$resolv_conf" /etc/resolv.conf
}

start_peer() {
    local ready="${!WORK_ENV}/peer.ready"
    ip netns exec "$UPSTREAM_NS" python3 -u \
        "$PROJECT_ROOT/scripts/traffic_routing_e2e_peer.py" \
        --dns-address "$DNS_ADDRESS" \
        --tcp-port 443 \
        --record domain.test=198.51.100.20 \
        --record app.test=203.0.113.30 \
        --record fail.test=203.0.113.40 \
        --record blocked.test=203.0.113.50 \
        --observations "${!OBSERVATIONS_ENV}" \
        --ready "$ready" \
        >"${!WORK_ENV}/peer.log" 2>&1 &
    PEER_PID=$!

    local deadline=$((SECONDS + 10))
    while [ ! -e "$ready" ]; do
        if ! kill -0 "$PEER_PID" 2>/dev/null; then
            cat "${!WORK_ENV}/peer.log" >&2 || true
            fail "isolated DNS/TCP peer exited before becoming ready"
        fi
        if [ "$SECONDS" -ge "$deadline" ]; then
            cat "${!WORK_ENV}/peer.log" >&2 || true
            fail "isolated DNS/TCP peer did not become ready"
        fi
        sleep 0.1
    done
    # The peer is reaped after the private namespace is removed. Disowning it
    # avoids a benign "Killed" job notice while preserving explicit cleanup.
    disown "$PEER_PID" 2>/dev/null || true
}

prebuild() {
    cargo test \
        --manifest-path "$PROJECT_ROOT/Cargo.toml" \
        -p fuckipv6-client \
        --no-default-features \
        --test traffic_routing_e2e_linux \
        --no-run
}

run_test() {
    cargo test \
        --manifest-path "$PROJECT_ROOT/Cargo.toml" \
        -p fuckipv6-client \
        --no-default-features \
        --test traffic_routing_e2e_linux \
        traffic_policy_netns_evidence_has_no_source_leak \
        -- \
        --ignored \
        --exact \
        --test-threads=1 \
        --nocapture
}

main() {
    require_tools

    if [ -z "${!WORK_ENV:-}" ]; then
        export "${WORK_ENV}=$(mktemp -d /tmp/fuckipv6-traffic-e2e.XXXXXX)"
        export "${REPORT_ENV}=${!WORK_ENV}/report.json"
        export "${OBSERVATIONS_ENV}=${!WORK_ENV}/observations.jsonl"
        export "${SECOND_UID_ENV}=${SECOND_UID}"
        export "${UPSTREAM_NS_ENV}=${UPSTREAM_NS}"
        export "${TOPOLOGY_ENV}=1"
        : >"${!OBSERVATIONS_ENV}"
    fi
    trap cleanup EXIT INT TERM

    if [ "${!INNER_ENV:-}" != "1" ]; then
        info "prebuilding the privileged traffic-routing E2E test"
        prebuild
        export "${INNER_ENV}=1"
        info "entering isolated user/network/mount namespaces"
        enter_isolated_namespaces "$@"
    fi

    mount -t tmpfs tmpfs /run
    prepare_links
    prepare_resolver
    start_peer
    info "running traffic-routing E2E with DNS, nftables, UID classification and TCP observation"
    run_test
    info "report: ${!REPORT_ENV}"
    cat "${!REPORT_ENV}"
    printf '\n'
    info "all Linux traffic-routing E2E checks passed"
}

main "$@"
