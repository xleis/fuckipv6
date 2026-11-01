#!/usr/bin/env bash
#
# Privileged Linux acceptance test for W2-09 resource recovery.
#
# The test runs in private network and mount namespaces. It creates a real TUN
# interface, installs real policy rules/table routes and a temporary resolver
# file, injects one route-delete and one TUN-delete failure, then verifies that
# cleanup can be retried without touching the host network or /etc/resolv.conf.

set -euo pipefail

PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SKIP_STATUS=77
INNER_ENV="FUCKIPV6_LIFECYCLE_NETNS_INNER"
WORK_ENV="FUCKIPV6_LIFECYCLE_NETNS_WORK"
RESOLV_CONF_ENV="FUCKIPV6_LIFECYCLE_RESOLV_CONF"
RULE_FAULT_MARKER_ENV="FUCKIPV6_LIFECYCLE_RULE_FAULT_MARKER"
TUN_FAULT_MARKER_ENV="FUCKIPV6_LIFECYCLE_TUN_FAULT_MARKER"
TUN_NAME="fip6clean"

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

require_root() {
    if [ "$(id -u)" -ne 0 ]; then
        skip "root is required for TUN, network namespaces and policy routing"
    fi
}

require_tools() {
    for tool in cargo ip mount unshare; do
        if ! command -v "$tool" >/dev/null 2>&1; then
            fail "missing required tool: $tool"
        fi
    done
    if [ ! -e /dev/net/tun ]; then
        skip "/dev/net/tun is unavailable"
    fi
}

require_netns_privileges() {
    if ! unshare --net --mount --fork --propagation private true >/dev/null 2>&1; then
        skip "network/mount namespace creation is unavailable"
    fi
}

cleanup() {
    set +e
    if ip link show dev "$TUN_NAME" >/dev/null 2>&1; then
        ip link delete "$TUN_NAME" >/dev/null 2>&1 || true
    fi
    if [ -n "${FUCKIPV6_LIFECYCLE_NETNS_WORK:-}" ]; then
        rm -rf "$FUCKIPV6_LIFECYCLE_NETNS_WORK"
    fi
}

prepare_isolated_files() {
    ip link set lo up
    mount -t tmpfs tmpfs /run

    local resolv_conf="${!RESOLV_CONF_ENV}"
    printf '# isolated baseline\nsearch cleanup.invalid\nnameserver 203.0.113.53\n' >"$resolv_conf"
    mount --bind "$resolv_conf" /etc/resolv.conf
}

create_fault_wrapper() {
    local real_ip fault_bin
    real_ip="$(command -v ip)"
    fault_bin="${!WORK_ENV}/fault-bin"
    mkdir -p "$fault_bin"

    cat >"$fault_bin/ip" <<EOF
#!/usr/bin/env bash
set -euo pipefail
arguments=" \$* "
if [[ "\$arguments" == *" rule del "* && ! -e "${!RULE_FAULT_MARKER_ENV}" ]]; then
    : >"${!RULE_FAULT_MARKER_ENV}"
    printf 'injected policy-rule deletion failure\n' >&2
    exit 42
fi
if [[ "\$arguments" == *" link delete ${TUN_NAME}"* && ! -e "${!TUN_FAULT_MARKER_ENV}" ]]; then
    : >"${!TUN_FAULT_MARKER_ENV}"
    printf 'injected TUN deletion failure\n' >&2
    exit 43
fi
exec "$real_ip" "\$@"
EOF
    chmod 0755 "$fault_bin/ip"
    export PATH="$fault_bin:$PATH"
}

run_test() {
    prepare_isolated_files
    create_fault_wrapper

    FUCKIPV6_LIFECYCLE_NETNS_TOPOLOGY=1 \
        cargo test \
        --manifest-path "$PROJECT_ROOT/Cargo.toml" \
        -p fuckipv6-client \
        --no-default-features \
        --test lifecycle_cleanup_linux \
        native_cleanup_recovery_restores_dns_routes_and_tun \
        -- \
        --ignored \
        --exact \
        --test-threads=1 \
        --nocapture
}

main() {
    require_root
    require_tools
    require_netns_privileges

    if [ -z "${!WORK_ENV:-}" ]; then
        export "${WORK_ENV}=$(mktemp -d /tmp/fuckipv6-lifecycle-netns.XXXXXX)"
        export "${RESOLV_CONF_ENV}=${!WORK_ENV}/resolv.conf"
        export "${RULE_FAULT_MARKER_ENV}=${!WORK_ENV}/rule-delete-failed"
        export "${TUN_FAULT_MARKER_ENV}=${!WORK_ENV}/tun-delete-failed"
        touch "${!RESOLV_CONF_ENV}"
    fi
    trap cleanup EXIT INT TERM

    if [ "${!INNER_ENV:-}" != "1" ]; then
        info "prebuilding the privileged lifecycle test"
        cargo test \
            --manifest-path "$PROJECT_ROOT/Cargo.toml" \
            -p fuckipv6-client \
            --no-default-features \
            --test lifecycle_cleanup_linux \
            --no-run

        export "${INNER_ENV}=1"
        info "entering isolated network and mount namespaces"
        exec unshare --net --mount --fork --propagation private -- "$0" "$@"
    fi

    run_test
    info "all Linux lifecycle cleanup recovery checks passed"
}

main "$@"
