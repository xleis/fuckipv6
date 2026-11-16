#!/usr/bin/env bash
#
# Privileged W14 acceptance test for the real multi-node administrator
# threshold-recovery drill.
#
# The Coordinator runs inside its own network namespace, connected to the host
# with a veth pair. The drill is the same multiprocess/offline-Trustee scenario
# as `w14_admin_recovery_multinode_offline_trustee_drill`, with one extra phase:
# nftables drops host traffic inside the namespace, the operator must observe
# that no acknowledgement arrives, and the flow only completes after the
# partition is restored.

set -euo pipefail

PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SKIP_STATUS=77
NETNS="fip6-w14-recovery"
HOST_LINK="fip6-w14-rec0"
PEER_LINK="fip6-w14-rec1"
HOST_IP="10.242.1.1"
COORDINATOR_IP="10.242.1.10"
WORK_ENV="FUCKIPV6_W14_ADMIN_RECOVERY_NETNS_WORK"
REPORT_ENV="FUCKIPV6_W14_ADMIN_RECOVERY_REPORT"

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

cleanup_topology() {
    ip netns delete "$NETNS" 2>/dev/null || true
    ip link delete "$HOST_LINK" 2>/dev/null || true
}

cleanup() {
    set +e
    cleanup_topology
    if [ -n "${!WORK_ENV:-}" ]; then
        rm -rf "${!WORK_ENV}"
    fi
}

require_root() {
    if [ "$(id -u)" -ne 0 ]; then
        skip "root is required to create a network namespace and apply nftables rules"
    fi
}

require_tools() {
    for tool in cargo ip nft ping timeout; do
        if ! command -v "$tool" >/dev/null 2>&1; then
            fail "missing required tool: $tool"
        fi
    done
}

create_topology() {
    cleanup_topology
    ip netns add "$NETNS"
    ip link add "$HOST_LINK" type veth peer name "$PEER_LINK"
    ip link set "$PEER_LINK" netns "$NETNS"
    ip addr add "$HOST_IP/24" dev "$HOST_LINK"
    ip link set "$HOST_LINK" up

    ip netns exec "$NETNS" ip link set lo up
    ip netns exec "$NETNS" ip addr add "$COORDINATOR_IP/24" dev "$PEER_LINK"
    ip netns exec "$NETNS" ip link set "$PEER_LINK" up

    if ! timeout 3 ip netns exec "$NETNS" ping -c 1 -W 1 "$HOST_IP" >/dev/null; then
        fail "Coordinator namespace $NETNS cannot reach host address $HOST_IP"
    fi
}

run_test() {
    create_topology
    info "Coordinator namespace: $NETNS ($COORDINATOR_IP), operator side: $HOST_IP"
    FUCKIPV6_W14_ADMIN_RECOVERY_NETNS="$NETNS" \
    FUCKIPV6_W14_ADMIN_RECOVERY_HOST_IP="$COORDINATOR_IP" \
    FUCKIPV6_W14_ADMIN_RECOVERY_REPORT="${!REPORT_ENV}" \
        cargo test \
        --manifest-path "$PROJECT_ROOT/Cargo.toml" \
        -p fuckipv6-coordinator \
        --test w14_admin_recovery_multinode \
        w14_admin_recovery_netns_drill \
        -- \
        --ignored \
        --exact \
        --test-threads=1 \
        --nocapture
    cleanup_topology
}

main() {
    require_root
    require_tools

    if [ -z "${!WORK_ENV:-}" ]; then
        export "${WORK_ENV}=$(mktemp -d /tmp/fuckipv6-w14-admin-recovery-netns.XXXXXX)"
        export "${REPORT_ENV}=${!WORK_ENV}/report.json"
    fi
    export CARGO_TARGET_DIR="${FUCKIPV6_W14_ADMIN_RECOVERY_CARGO_TARGET_DIR:-${PROJECT_ROOT}/target}"
    trap cleanup EXIT INT TERM

    run_test
    [ -s "${!REPORT_ENV}" ] || fail "W14 administrator recovery drill report was not written: ${!REPORT_ENV}"
    python3 - "${!REPORT_ENV}" <<'PY'
import json
import sys

report = json.load(open(sys.argv[1]))
required = {
    "schema": "fuckipv6.w14.admin-recovery.drill.v1",
    "trustee_threshold": 2,
    "approvals_collected": 2,
    "coordinator_restarts": 2,
    "durable_pending_after_restart": True,
    "pending_applicant_survived": True,
    "idempotent_reexecute": True,
    "partition_enforced": True,
    "no_ack_while_partitioned": True,
}
for key, expected in required.items():
    actual = report.get(key)
    if actual != expected:
        raise SystemExit(f"[FAIL] {key}: expected {expected!r}, got {actual!r}")
if report.get("new_revision") != report.get("previous_revision", 0) + 1:
    raise SystemExit("[FAIL] recovery did not advance the network revision exactly once")
if not report.get("unreachable_while_down"):
    raise SystemExit("[FAIL] the drill did not observe a control-plane outage")
print("[INFO] W14 administrator recovery netns drill report verified")
PY
    info "W14 administrator recovery multi-node drill passed"
    info "report: ${!REPORT_ENV}"
    cat "${!REPORT_ENV}"
}

main "$@"
