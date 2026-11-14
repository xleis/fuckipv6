#!/usr/bin/env bash
#
# W14 acceptance test for whole-machine power loss.
#
# A minimal Linux VM runs the real Coordinator candidate store in a QEMU guest.
# The guest commits a candidate configuration revision and fsyncs it to an ext4
# virtio disk. The host then sends SIGKILL to QEMU without a guest shutdown,
# inspects the raw ext4 image, and boots the same disk in a second VM to prove
# that the committed authority survives an abrupt machine power cut.

set -euo pipefail

PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SKIP_STATUS=77
REPORT_ENV="FUCKIPV6_W14_POWERLOSS_REPORT"
WORK_ENV="FUCKIPV6_W14_POWERLOSS_WORK"
KERNEL="${FUCKIPV6_W14_POWERLOSS_KERNEL:-/boot/vmlinuz-linux}"
BUSYBOX="${FUCKIPV6_W14_POWERLOSS_BUSYBOX:-/usr/lib/initcpio/busybox}"
MOUNT_BIN="${FUCKIPV6_W14_POWERLOSS_MOUNT_BIN:-/usr/bin/mount}"
TEST_NAME="w14_process_child_fixture"
PREPARE_NAME="w14_vm_powerloss_prepare_fixture"

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
        skip "root is required for deterministic KVM access"
    fi
}

require_tools() {
    for tool in cargo cpio debugfs e2fsck gzip kill ldd mkfs.ext4 qemu-system-x86_64 python3 sha256sum strip truncate; do
        command -v "$tool" >/dev/null 2>&1 || fail "missing required tool: $tool"
    done
    [ -r "$KERNEL" ] || fail "kernel image is not readable: $KERNEL"
    [ -x "$BUSYBOX" ] || fail "busybox is not executable: $BUSYBOX"
    [ -x "$MOUNT_BIN" ] || fail "mount binary is not executable: $MOUNT_BIN"
    [ -c /dev/kvm ] || skip "KVM is unavailable"
}

cleanup() {
    set +e
    if [ -n "${VM_PID:-}" ] && kill -0 "$VM_PID" 2>/dev/null; then
        kill -KILL "$VM_PID" 2>/dev/null || true
        wait "$VM_PID" 2>/dev/null || true
    fi
    if [ "${SCRIPT_SUCCEEDED:-0}" -eq 1 ] && [ "${FUCKIPV6_W14_POWERLOSS_KEEP:-0}" != "1" ] &&
        [ -n "${FUCKIPV6_W14_POWERLOSS_WORK:-}" ]; then
        rm -rf "$FUCKIPV6_W14_POWERLOSS_WORK"
    elif [ -n "${FUCKIPV6_W14_POWERLOSS_WORK:-}" ]; then
        printf '[INFO] retained failure artifacts: %s\n' "$FUCKIPV6_W14_POWERLOSS_WORK" >&2
    fi
}

find_test_binary() {
    cargo test \
        --manifest-path "$PROJECT_ROOT/Cargo.toml" \
        -p fuckipv6-coordinator \
        --test w14_coordinator_fault_process \
        --no-run \
        --message-format=json |
        python3 -c '
import json, sys
for line in sys.stdin:
    try:
        message = json.loads(line)
    except json.JSONDecodeError:
        continue
    if (
        message.get("reason") == "compiler-artifact"
        and message.get("target", {}).get("name") == "w14_coordinator_fault_process"
        and message.get("executable")
    ):
        print(message["executable"])
        break
'
}

copy_runtime_libraries() {
    local root="$1"
    shift
    local libraries
    libraries="$(
        for binary in "$@"; do
            ldd "$binary" 2>/dev/null || true
        done |
        awk '
            /=> \// { print $3 }
            /^[[:space:]]*\/lib64\// { print $1 }
        ' |
        sort -u
    )"
    while IFS= read -r library; do
        [ -n "$library" ] || continue
        [ -r "$library" ] || fail "runtime library is not readable: $library"
        mkdir -p "$root$(dirname "$library")"
        cp -L "$library" "$root$library"
    done <<< "$libraries"
}

write_init() {
    local root="$1"
    cat > "$root/init" <<'INIT'
#!/bin/sh
export PATH=/bin
mount -t proc proc /proc || exit 10
mount -t sysfs sysfs /sys || exit 11
mount -t devtmpfs devtmpfs /dev || exit 12
mkdir -p /tmp /work
mount -t tmpfs tmpfs /tmp || exit 13

attempt=0
while [ ! -b /dev/vda ] && [ "$attempt" -lt 100 ]; do
    sleep 0.1
    attempt=$((attempt + 1))
done
[ -b /dev/vda ] || { echo W14_VIRTIO_DISK_MISSING; poweroff -f; exit 14; }
mount -t ext4 -o rw /dev/vda /work || { echo W14_EXT4_MOUNT_FAILED; poweroff -f; exit 15; }

export FUCKIPV6_W14_PROCESS_CHILD=1
export FUCKIPV6_W14_PROCESS_JOB=/work/job.json
/w14-test --exact w14_process_child_fixture --nocapture
status=$?
echo "W14_CHILD_EXIT=$status"
if [ "$status" -ne 0 ]; then
    echo W14_CHILD_FAILED
    poweroff -f
    exit "$status"
fi
if [ -s /work/status-output.json ]; then
    echo W14_STATUS_COMMITTED
elif [ -s /work/output.json ]; then
    echo W14_CHILD_COMMITTED
else
    echo W14_CHILD_OUTPUT_MISSING
    poweroff -f
    exit 16
fi
sleep 2
poweroff -f
INIT
    chmod 0755 "$root/init"
}

make_initramfs() {
    local root="$1"
    local output="$2"
    (cd "$root" && find . -print0 | cpio --null --create --format=newc 2>/dev/null | gzip -9) > "$output"
}

prepare_fixture() {
    local fixture_dir="$1"
    FUCKIPV6_W14_VM_FIXTURE_DIR="$fixture_dir" \
        "$TEST_BINARY" --exact "$PREPARE_NAME" --ignored --nocapture
}

create_disk() {
    local image="$1"
    local fixture_dir="$2"
    truncate -s 128M "$image"
    mkfs.ext4 -F -q -d "$fixture_dir" "$image"
}

set_status_job() {
    local fixture_dir="$1"
    local job_path="$fixture_dir/status-job.json"
    python3 - "$fixture_dir/metadata.json" "$job_path" <<'PY'
import json
import sys
import time

metadata_path, job_path = sys.argv[1:]
with open(metadata_path, encoding="utf-8") as source:
    metadata = json.load(source)
job = {
    "kind": "store",
    "store_path": "/work/coordinator_candidates.json",
    "network_path": "/work/network.json",
    "output_path": "/work/status-output.json",
    "started_path": "/work/status-started",
    "gate_path": None,
    "operation": {
        "operation": "status",
        "actor": metadata["actor"],
        "network_id": metadata["network_id"],
        "now_unix_ms": int(time.time() * 1000),
    },
}
with open(job_path, "w", encoding="utf-8") as output:
    json.dump(job, output, indent=2)
PY
    replace_ext4_file "$DISK_IMAGE" /job.json "$job_path"
}

run_vm() {
    local image="$1"
    local initramfs="$2"
    local log="$3"
    qemu-system-x86_64 \
        -machine q35,accel=kvm \
        -cpu host \
        -m 512M \
        -smp 1 \
        -kernel "$KERNEL" \
        -initrd "$initramfs" \
        -append "console=ttyS0 rdinit=/init panic=-1" \
        -drive "file=$image,if=virtio,format=raw,cache=none" \
        -display none \
        -serial "file:$log" \
        -monitor none \
        -no-reboot \
        > /dev/null 2>&1 &
    VM_PID=$!
}

wait_for_marker() {
    local log="$1"
    local marker="$2"
    local attempts=0
    while [ "$attempts" -lt 1800 ]; do
        if grep -q "$marker" "$log" 2>/dev/null; then
            return 0
        fi
        if ! kill -0 "$VM_PID" 2>/dev/null; then
            fail "VM exited before marker $marker; log: $log"
        fi
        sleep 0.1
        attempts=$((attempts + 1))
    done
    fail "timed out waiting for VM marker $marker; log: $log"
}

wait_for_vm_exit() {
    local attempts=0
    while kill -0 "$VM_PID" 2>/dev/null; do
        if [ "$attempts" -ge 600 ]; then
            fail "VM did not power off within 60 seconds"
        fi
        sleep 0.1
        attempts=$((attempts + 1))
    done
    wait "$VM_PID" 2>/dev/null || true
}

recover_ext4_after_power_cut() {
    local image="$1"
    local output="$2"
    local status=0
    e2fsck -fy "$image" > "$output" 2>&1 || status=$?
    case "$status" in
        0|1|2) ;;
        *) fail "e2fsck failed with exit status $status; output: $output" ;;
    esac
    FSCK_EXIT_STATUS="$status"
}

extract_ext4_file() {
    local image="$1"
    local guest_path="$2"
    local output="$3"
    debugfs -R "cat $guest_path" "$image" > "$output" 2> "$output.debugfs"
    [ -s "$output" ] || fail "unable to read $guest_path from ext4 image: $output.debugfs"
}

replace_ext4_file() {
    local image="$1"
    local guest_path="$2"
    local source_path="$3"
    debugfs -w -R "rm $guest_path" "$image" > /dev/null 2>&1 || true
    debugfs -w -R "write $source_path $guest_path" "$image" > /dev/null 2>&1 ||
        fail "unable to replace $guest_path in ext4 image"
}

validate_committed_store() {
    python3 - "$1" "$FIXTURE_DIR/metadata.json" <<'PY'
import json
import sys

store_path, metadata_path = sys.argv[1:]
with open(metadata_path, encoding="utf-8") as source:
    metadata = json.load(source)
with open(store_path, encoding="utf-8") as source:
    store = json.load(source)
stored = store["networks"][metadata["network_id"]]
revision = stored["config"]["revision"]
expected = metadata["expected_config_revision"]
if revision != expected:
    raise SystemExit(f"config revision {revision} != expected {expected}")
if stored.get("latest_term") is not None:
    raise SystemExit("unexpected Coordinator term after bootstrap-only write")
print(json.dumps({"config_revision": revision, "latest_term": None}, sort_keys=True))
PY
}

validate_status_output() {
    python3 - "$1" <<'PY'
import json
import sys

with open(sys.argv[1], encoding="utf-8") as source:
    output = json.load(source)
if not output.get("ok"):
    raise SystemExit(f"status operation failed after power cut: {output}")
status = output.get("status") or {}
config = status.get("config") or {}
if config.get("revision") != 1:
    raise SystemExit(f"status did not expose revision 1: {output}")
print(json.dumps({"ok": True, "candidate_revision": config.get("revision")}, sort_keys=True))
PY
}

main() {
    require_root
    require_tools

    if [ -z "${!WORK_ENV:-}" ]; then
        export "${WORK_ENV}=$(mktemp -d /tmp/fuckipv6-w14-powerloss.XXXXXX)"
    fi
    export "${REPORT_ENV}=${!WORK_ENV}/report.json"
    export CARGO_TARGET_DIR="${FUCKIPV6_W14_POWERLOSS_CARGO_TARGET_DIR:-${!WORK_ENV}/target}"
    trap cleanup EXIT INT TERM

    FIXTURE_DIR="${!WORK_ENV}/fixture"
    DISK_IMAGE="${!WORK_ENV}/powerloss.ext4"
    INIT_ROOT="${!WORK_ENV}/initramfs-root"
    INITRAMFS="${!WORK_ENV}/initramfs.cpio.gz"
    mkdir -p "$FIXTURE_DIR" "$INIT_ROOT/bin" "$INIT_ROOT/dev" "$INIT_ROOT/proc" "$INIT_ROOT/sys" "$INIT_ROOT/tmp" "$INIT_ROOT/work"

    info "building the Linux test binary"
    TEST_BINARY="$(find_test_binary)"
    [ -x "$TEST_BINARY" ] || fail "cargo did not return an executable test binary"

    prepare_fixture "$FIXTURE_DIR"
    create_disk "$DISK_IMAGE" "$FIXTURE_DIR"

    cp "$TEST_BINARY" "$INIT_ROOT/w14-test"
    strip --strip-debug "$INIT_ROOT/w14-test"
    cp "$BUSYBOX" "$INIT_ROOT/bin/busybox"
    cp "$MOUNT_BIN" "$INIT_ROOT/bin/mount"
    for applet in sh mkdir cat echo sleep poweroff grep; do
        ln -sf /bin/busybox "$INIT_ROOT/bin/$applet"
    done
    copy_runtime_libraries "$INIT_ROOT" "$INIT_ROOT/w14-test" "$BUSYBOX" "$MOUNT_BIN"
    write_init "$INIT_ROOT"
    make_initramfs "$INIT_ROOT" "$INITRAMFS"

    BOOT1_LOG="${!WORK_ENV}/boot1.log"
    info "booting guest and committing coordinator authority"
    run_vm "$DISK_IMAGE" "$INITRAMFS" "$BOOT1_LOG"
    wait_for_marker "$BOOT1_LOG" W14_CHILD_COMMITTED
    POWER_CUT_QEMU_PID="$VM_PID"
    kill -KILL "$VM_PID"
    wait "$VM_PID" 2>/dev/null || true
    VM_PID=""
    info "QEMU received SIGKILL without guest shutdown"

    recover_ext4_after_power_cut "$DISK_IMAGE" "${!WORK_ENV}/fsck-after-power-cut.log"
    POWER_CUT_FSCK_STATUS="$FSCK_EXIT_STATUS"
    extract_ext4_file "$DISK_IMAGE" /coordinator_candidates.json "${!WORK_ENV}/committed-store.json"
    COMMITTED_EVIDENCE="$(validate_committed_store "${!WORK_ENV}/committed-store.json")"
    set_status_job "$FIXTURE_DIR"

    BOOT2_LOG="${!WORK_ENV}/boot2.log"
    info "rebooting the same virtual disk and reading durable authority"
    run_vm "$DISK_IMAGE" "$INITRAMFS" "$BOOT2_LOG"
    wait_for_marker "$BOOT2_LOG" W14_STATUS_COMMITTED
    wait_for_vm_exit
    VM_PID=""

    recover_ext4_after_power_cut "$DISK_IMAGE" "${!WORK_ENV}/fsck-after-reboot.log"
    extract_ext4_file "$DISK_IMAGE" /status-output.json "${!WORK_ENV}/status-output.json"
    extract_ext4_file "$DISK_IMAGE" /coordinator_candidates.json "${!WORK_ENV}/recovered-store.json"
    STATUS_EVIDENCE="$(validate_status_output "${!WORK_ENV}/status-output.json")"
    STORE_SHA256="$(sha256sum "${!WORK_ENV}/recovered-store.json" | awk '{print $1}')"

    python3 - "${!REPORT_ENV}" "$KERNEL" "$COMMITTED_EVIDENCE" "$STATUS_EVIDENCE" "$STORE_SHA256" "$POWER_CUT_QEMU_PID" "$POWER_CUT_FSCK_STATUS" <<'PY'
import json
import os
import sys
import time

report_path, kernel, committed, status, digest, qemu_pid, fsck_status = sys.argv[1:]
report = {
    "schema": "fuckipv6.w14.powerloss.v1",
    "mode": "qemu-kvm-virtio-ext4-sigkill",
    "power_cut": "host SIGKILL to the QEMU process; no guest shutdown or sync",
    "qemu_pid_killed": int(qemu_pid),
    "host_validation": "e2fsck -fy followed by raw-image debugfs extraction",
    "fsck_exit_status_after_power_cut": int(fsck_status),
    "kernel": os.path.basename(kernel),
    "committed_before_power_cut": json.loads(committed),
    "recovered_after_reboot": json.loads(status),
    "store_sha256_after_reboot": digest,
    "completed_at_unix_ms": int(time.time() * 1000),
}
with open(report_path, "w", encoding="utf-8") as output:
    json.dump(report, output, indent=2, sort_keys=True)
    output.write("\n")
PY

    [ -s "${!REPORT_ENV}" ] || fail "power-loss report was not written"
    SCRIPT_SUCCEEDED=1
    info "W14 Coordinator whole-VM power-loss check passed"
    info "report: ${!REPORT_ENV}"
    cat "${!REPORT_ENV}"
}

main "$@"
