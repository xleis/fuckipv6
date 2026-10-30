#!/usr/bin/env python3
"""Shared helpers for the reproducible NX-17 dataplane benchmark."""

from __future__ import annotations

import hashlib
import json
import os
import platform
import statistics
import subprocess
import time
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Iterable

POLICY_SCHEMA = "fuckipv6.dataplane-policy/v1"
PLAN_SCHEMA = "fuckipv6.dataplane-plan/v1"
ENVIRONMENT_SCHEMA = "fuckipv6.dataplane-environment/v1"
REPORT_SCHEMA = "fuckipv6.dataplane-gate-report/v1"
LATENCY_SCHEMA = "fuckipv6.dataplane-latency-invocation/v1"
SOURCE_SCHEMA = "fuckipv6.dataplane-source/v1"
EVIDENCE_MANIFEST_SCHEMA = "fuckipv6.dataplane-evidence-sha256/v1"
CANONICAL_POLICY_ID = "nx17-canonical-v1"

IMPLEMENTATIONS = ("fuckipv6", "wireguard-kernel", "wireguard-go")
COMPARATORS = ("wireguard-kernel", "wireguard-go")
TESTS = (
    "tcp_1flow",
    "tcp_8flow",
    "udp_1420_saturated",
    "udp_64_saturated",
    "udp_1420_controlled",
    "udp_64_controlled",
)

TRAFFIC_MODELS = (
    {
        "name": "tcp_1flow",
        "transport": "tcp",
        "streams": 1,
        "payload_bytes": None,
        "load": "unlimited",
        "latency_gate": True,
    },
    {
        "name": "tcp_8flow",
        "transport": "tcp",
        "streams": 8,
        "payload_bytes": None,
        "load": "unlimited",
        "latency_gate": True,
    },
    {
        "name": "udp_1420_saturated",
        "transport": "udp",
        "streams": 1,
        "payload_bytes": 1420,
        "load": "saturated",
        "latency_gate": False,
    },
    {
        "name": "udp_64_saturated",
        "transport": "udp",
        "streams": 1,
        "payload_bytes": 64,
        "load": "saturated",
        "latency_gate": False,
    },
    {
        "name": "udp_1420_controlled",
        "transport": "udp",
        "streams": 1,
        "payload_bytes": 1420,
        "load": "controlled",
        "controlled_rate": "50M",
        "latency_gate": True,
    },
    {
        "name": "udp_64_controlled",
        "transport": "udp",
        "streams": 1,
        "payload_bytes": 64,
        "load": "controlled",
        "controlled_rate": "5M",
        "latency_gate": True,
    },
)

DEFAULT_POLICY: dict[str, Any] = {
    "schema": POLICY_SCHEMA,
    "required_runs": 5,
    "throughput_min_ratio": 1.05,
    "p99_max_ratio": 1.0,
    "cpu_cost_max_ratio": 1.0,
    "controlled_udp_max_loss_percent": 1.0,
    "traffic": {
        "duration_seconds": 30,
        "omit_seconds": 2,
        "mtu": 1420,
    },
    "environment": {
        "load1_max": 0.5,
        "cpu_busy_percent_max": 5.0,
        "require_performance_governor": False,
    },
    "latency": {
        "probe_interval_ms": 10.0,
        "probe_timeout_seconds": 1.0,
        "min_samples_per_run": 200,
        "max_loss_percent": 1.0,
        "gated_tests": [
            model["name"] for model in TRAFFIC_MODELS if model["latency_gate"]
        ],
    },
    "traffic_models": list(TRAFFIC_MODELS),
}


def canonical_json(value: Any) -> bytes:
    """Serialize a policy deterministically for hashing and equality checks."""
    return json.dumps(value, sort_keys=True, separators=(",", ":")).encode("utf-8")


def policy_sha256(policy: dict[str, Any]) -> str:
    return hashlib.sha256(canonical_json(policy)).hexdigest()


CANONICAL_POLICY_SHA256 = policy_sha256(DEFAULT_POLICY)


def policy_metadata(policy: dict[str, Any]) -> dict[str, Any]:
    digest = policy_sha256(policy)
    canonical = digest == CANONICAL_POLICY_SHA256
    return {
        "id": CANONICAL_POLICY_ID if canonical else "custom",
        "sha256": digest,
        "canonical": canonical,
        "canonical_id": CANONICAL_POLICY_ID,
        "canonical_sha256": CANONICAL_POLICY_SHA256,
    }


def utc_now() -> str:
    return datetime.now(timezone.utc).isoformat().replace("+00:00", "Z")


def write_json(path: Path, value: Any) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_name(path.name + ".tmp")
    temporary.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    temporary.replace(path)


def read_json(path: Path) -> Any:
    return json.loads(path.read_text(encoding="utf-8"))


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def command_output(argv: list[str]) -> str | None:
    try:
        return subprocess.check_output(
            argv, text=True, stderr=subprocess.DEVNULL, timeout=10
        ).strip()
    except (OSError, subprocess.CalledProcessError, subprocess.TimeoutExpired):
        return None


def parse_cpu_list(value: str, allowed: Iterable[int] | None = None) -> set[int]:
    """Parse a Linux CPU list such as ``0,2-3`` without silently reordering it."""
    if not value.strip():
        raise ValueError("CPU list must not be empty")
    allowed_set = set(allowed) if allowed is not None else None
    result: set[int] = set()
    for item in value.split(","):
        item = item.strip()
        if not item:
            raise ValueError(f"invalid empty item in CPU list: {value!r}")
        if "-" in item:
            bounds = item.split("-")
            if len(bounds) != 2 or not all(bound.isdigit() for bound in bounds):
                raise ValueError(f"invalid CPU range: {item!r}")
            start, end = (int(bound) for bound in bounds)
            if start > end:
                raise ValueError(f"descending CPU range: {item!r}")
            cpus = set(range(start, end + 1))
        else:
            if not item.isdigit():
                raise ValueError(f"invalid CPU id: {item!r}")
            cpus = {int(item)}
        overlap = result & cpus
        if overlap:
            raise ValueError(f"duplicate CPUs in list: {sorted(overlap)}")
        result.update(cpus)
    if allowed_set is not None and not result <= allowed_set:
        unavailable = sorted(result - allowed_set)
        raise ValueError(f"CPUs are not available to this process: {unavailable}")
    return result


def cpu_sets_disjoint(named_sets: dict[str, set[int]]) -> None:
    names = list(named_sets)
    for index, left_name in enumerate(names):
        for right_name in names[index + 1 :]:
            overlap = named_sets[left_name] & named_sets[right_name]
            if overlap:
                raise ValueError(
                    f"CPU sets must be disjoint; {left_name} and {right_name} share "
                    f"{sorted(overlap)}"
                )


def read_load_average() -> tuple[float, float, float]:
    one, five, fifteen = os.getloadavg()
    return float(one), float(five), float(fifteen)


def _cpu_totals() -> tuple[int, int]:
    fields = Path("/proc/stat").read_text(encoding="utf-8").splitlines()[0].split()[1:]
    values = [int(value) for value in fields]
    idle = values[3] + (values[4] if len(values) > 4 else 0)
    return sum(values), idle


def sample_cpu_busy_percent(interval_seconds: float = 1.0) -> float:
    if interval_seconds <= 0:
        raise ValueError("CPU sample interval must be positive")
    total_before, idle_before = _cpu_totals()
    time.sleep(interval_seconds)
    total_after, idle_after = _cpu_totals()
    total_delta = total_after - total_before
    idle_delta = idle_after - idle_before
    if total_delta <= 0 or idle_delta < 0:
        raise ValueError("could not obtain a valid /proc/stat CPU sample")
    return 100.0 * (total_delta - idle_delta) / total_delta


def logical_cpu_count() -> int:
    count = os.cpu_count()
    if count is None or count < 1:
        raise ValueError("could not determine logical CPU count")
    return count


def sample_environment(
    *,
    expected_kernel: str,
    quiet_load1_max: float,
    quiet_busy_percent_max: float,
    cpu_sets: dict[str, set[int]],
    wireguard_go: dict[str, Any] | None,
) -> dict[str, Any]:
    load_one, load_five, load_fifteen = read_load_average()
    busy_percent = sample_cpu_busy_percent(1.0)
    failures = []
    if load_one > quiet_load1_max:
        failures.append(
            f"load1 {load_one:.2f} exceeds quiet-host limit {quiet_load1_max:.2f}"
        )
    if busy_percent > quiet_busy_percent_max:
        failures.append(
            f"sampled CPU busy {busy_percent:.2f}% exceeds quiet-host limit "
            f"{quiet_busy_percent_max:.2f}%"
        )
    kernel = platform.release()
    if kernel != expected_kernel:
        failures.append(f"kernel {kernel!r} does not match pinned {expected_kernel!r}")

    governors: dict[str, str] = {}
    for path in sorted(Path("/sys/devices/system/cpu").glob("cpu[0-9]*/cpufreq/scaling_governor")):
        try:
            governors[path.parent.parent.name] = path.read_text(encoding="utf-8").strip()
        except OSError:
            continue

    return {
        "schema": ENVIRONMENT_SCHEMA,
        "captured_utc": utc_now(),
        "hostname": platform.node(),
        "kernel": kernel,
        "expected_kernel": expected_kernel,
        "machine": platform.machine(),
        "platform": platform.platform(),
        "logical_cpus": logical_cpu_count(),
        "cpu_model": command_output(
            ["sh", "-c", "awk -F: '/model name/ {gsub(/^ +/, \"\", $2); print $2; exit}' /proc/cpuinfo"]
        ),
        "load_average": {
            "1m": load_one,
            "5m": load_five,
            "15m": load_fifteen,
        },
        "cpu_busy_percent_1s": busy_percent,
        "quiet_host": {
            "passed": not failures,
            "load1_max": quiet_load1_max,
            "cpu_busy_percent_max": quiet_busy_percent_max,
            "failures": failures,
        },
        "cpu_affinity": {
            name: ",".join(str(cpu) for cpu in sorted(cpus))
            for name, cpus in cpu_sets.items()
        },
        "frequency_governors": governors,
        "wireguard_go": wireguard_go,
        "kernel_wireguard": {
            "kernel_release": kernel,
            "wg_userspace": command_output(["wg", "--version"]),
            "modinfo": command_output(["modinfo", "wireguard"]),
        },
        "tools": {
            "iperf3": command_output(["iperf3", "--version"]),
            "perf": command_output(["perf", "--version"]),
            "taskset": command_output(["taskset", "--version"]),
            "ping": command_output(["ping", "-V"]),
            "python": platform.python_version(),
        },
        "top_processes": command_output(
            ["sh", "-c", "ps -eo pid,comm,pcpu,psr --sort=-pcpu | head -n 16"]
        ),
        "sysctls": {
            name: command_output(["sysctl", "-n", name])
            for name in (
                "net.core.rmem_max",
                "net.core.wmem_max",
                "net.ipv4.tcp_congestion_control",
            )
        },
    }


def median(values: Iterable[float]) -> float:
    materialized = list(values)
    if not materialized:
        raise ValueError("median requires at least one value")
    return float(statistics.median(materialized))
