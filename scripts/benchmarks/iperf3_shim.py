#!/usr/bin/env python3
"""Transparent iperf3 shim that records concurrent ICMP tail-latency samples.

The NX-17 runner prepends a symlink named ``iperf3`` pointing at this file to
PATH. Client invocations start a fixed-rate ping through the tunnel, execute the
real iperf3 binary, and preserve both raw streams. Server invocations only pass
through to iperf3. A missing result is never synthesized.
"""

from __future__ import annotations

import fcntl
import json
import math
import os
import signal
import subprocess
import sys
import time
from pathlib import Path
from typing import Any

SCHEMA = "fuckipv6.dataplane-latency-invocation/v1"


def _option(args: list[str], name: str, default: str | None = None) -> str | None:
    try:
        return args[args.index(name) + 1]
    except (ValueError, IndexError):
        return default


def classify_iperf_args(args: list[str]) -> str:
    """Map an iperf3 client command line to the predeclared traffic model."""
    streams = int(float(_option(args, "-P", "1") or "1"))
    if "-u" not in args:
        if streams == 1:
            return "tcp_1flow"
        if streams == 8:
            return "tcp_8flow"
        return "unknown"

    payload = int(float(_option(args, "-l", "0") or "0"))
    rate = (_option(args, "-b", "") or "").strip().lower()
    saturated = rate in {"0", "0m", "0g", "unlimited"}
    if payload == 1420:
        return "udp_1420_saturated" if saturated else "udp_1420_controlled"
    if payload == 64:
        return "udp_64_saturated" if saturated else "udp_64_controlled"
    return "unknown"


def _seconds_option(args: list[str], name: str, default: float) -> float:
    raw = _option(args, name)
    try:
        value = float(raw) if raw is not None else default
    except ValueError:
        return default
    return value if math.isfinite(value) and value > 0 else default


def _append_metadata(path: Path, record: dict[str, Any]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    line = (json.dumps(record, sort_keys=True, separators=(",", ":")) + "\n").encode()
    with path.open("ab") as output:
        fcntl.flock(output.fileno(), fcntl.LOCK_EX)
        output.write(line)
        output.flush()
        os.fsync(output.fileno())
        fcntl.flock(output.fileno(), fcntl.LOCK_UN)


def _relative_to_output(path: Path, output_dir: Path) -> str:
    try:
        return str(path.relative_to(output_dir))
    except ValueError:
        return str(path)


def main() -> int:
    real_iperf3 = os.environ.get("FIP6_BENCH_REAL_IPERF3")
    metadata_path = os.environ.get("FIP6_BENCH_LATENCY_LOG")
    output_dir_raw = os.environ.get("FIP6_BENCH_LATENCY_DIR")
    if not real_iperf3 or not metadata_path or not output_dir_raw:
        print(
            "iperf3-shim: FIP6_BENCH_REAL_IPERF3, FIP6_BENCH_LATENCY_LOG, and "
            "FIP6_BENCH_LATENCY_DIR are required",
            file=sys.stderr,
        )
        return 125

    args = sys.argv[1:]
    metadata_path_obj = Path(metadata_path)
    output_dir = Path(output_dir_raw)
    started_ns = time.time_ns()
    is_server = "-s" in args

    record: dict[str, Any] = {
        "schema": SCHEMA,
        "kind": "server" if is_server else "client",
        "started_ns": started_ns,
        "args": args,
    }

    if is_server:
        _append_metadata(metadata_path_obj, record)
        return subprocess.run([real_iperf3, *args], check=False).returncode

    target = _option(args, "-c")
    if not target:
        # Preserve transparent behavior for --version, --help and any other
        # non-benchmark invocation used by preflight or metadata collection.
        return subprocess.run([real_iperf3, *args], check=False).returncode

    interval_ms = float(os.environ.get("FIP6_BENCH_PROBE_INTERVAL_MS", "10"))
    timeout_seconds = float(os.environ.get("FIP6_BENCH_PROBE_TIMEOUT_S", "1"))
    duration = _seconds_option(args, "-t", 10.0)
    omit = _seconds_option(args, "-O", 0.0)
    # iperf3 -t is the total stream duration; -O only excludes its warm-up
    # interval from iperf3 aggregation. Probe for the full stream duration.
    expected_samples = max(1, int(math.ceil(duration * 1000.0 / interval_ms)))
    latency_dir = output_dir / "latency"
    latency_dir.mkdir(parents=True, exist_ok=True)
    ping_file = latency_dir / f"probe-{started_ns}-{os.getpid()}.txt"
    ping_stderr = latency_dir / f"probe-{started_ns}-{os.getpid()}.stderr.txt"
    command = [
        "ping",
        "-6",
        "-n",
        "-D",
        "-i",
        f"{interval_ms / 1000.0:.6f}",
        "-W",
        f"{timeout_seconds:.3f}",
        "-c",
        str(expected_samples),
        target,
    ]

    ping_process: subprocess.Popen[Any] | None = None
    ping_started_ns = time.time_ns()
    try:
        with ping_file.open("wb") as stdout, ping_stderr.open("wb") as stderr:
            ping_process = subprocess.Popen(
                command,
                stdout=stdout,
                stderr=stderr,
                start_new_session=True,
            )
            iperf_result = subprocess.run([real_iperf3, *args], check=False)
    finally:
        ping_returncode: int | None = None
        if ping_process is not None and ping_process.poll() is None:
            try:
                os.killpg(ping_process.pid, signal.SIGINT)
            except ProcessLookupError:
                pass
        if ping_process is not None:
            try:
                ping_returncode = ping_process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                try:
                    os.killpg(ping_process.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
                ping_returncode = ping_process.wait(timeout=5)

    record.update(
        {
            "finished_ns": time.time_ns(),
            "ping_started_ns": ping_started_ns,
            "test_hint": classify_iperf_args(args),
            "target": target,
            "duration_seconds": duration,
            "omit_seconds": omit,
            "expected_samples_basis": "iperf3 -t duration",
            "probe_interval_ms": interval_ms,
            "probe_timeout_seconds": timeout_seconds,
            "expected_samples": expected_samples,
            "ping_file": _relative_to_output(ping_file, output_dir.parent),
            "ping_stderr_file": _relative_to_output(ping_stderr, output_dir.parent),
            "ping_returncode": ping_returncode,
            "iperf_returncode": iperf_result.returncode,
        }
    )
    _append_metadata(metadata_path_obj, record)
    return iperf_result.returncode


if __name__ == "__main__":
    raise SystemExit(main())
