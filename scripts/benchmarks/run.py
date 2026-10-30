#!/usr/bin/env python3
"""Run the pinned NX-17 dataplane comparison and enforce its evidence gate."""

from __future__ import annotations

import argparse
import copy
import json
import os
import shutil
import subprocess
import sys
import time
from pathlib import Path
from typing import Any

from gate import evaluate_output
from harness import (
    CANONICAL_POLICY_ID,
    COMPARATORS,
    DEFAULT_POLICY,
    EVIDENCE_MANIFEST_SCHEMA,
    IMPLEMENTATIONS,
    PLAN_SCHEMA,
    SOURCE_SCHEMA,
    policy_metadata,
    sample_environment,
    sha256_file,
    utc_now,
    write_json,
)

SCRIPT_DIR = Path(__file__).resolve().parent
PROJECT_ROOT = SCRIPT_DIR.parents[1]
DATAPLANE_SCRIPT = PROJECT_ROOT / "scripts" / "bench-dataplane.sh"
LATENCY_SHIM = SCRIPT_DIR / "iperf3_shim.py"


def _csv_or_space(value: str) -> list[str]:
    return [item for item in value.replace(",", " ").split() if item]


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--impls",
        default=" ".join(IMPLEMENTATIONS),
        help="implementations to run (default: fuckipv6 wireguard-kernel wireguard-go)",
    )
    parser.add_argument("--runs", type=int, default=5)
    parser.add_argument("--duration", type=int, default=30)
    parser.add_argument("--omit", type=int, default=2)
    parser.add_argument("--mtu", type=int, default=1420)
    parser.add_argument("--udp-1420-rate", default="50M")
    parser.add_argument("--udp-64-rate", default="5M")
    parser.add_argument("--client-cpus", required=True, help="pinned iperf/fuckipv6 client CPUs")
    parser.add_argument("--server-cpus", required=True, help="pinned iperf/fuckipv6 server CPUs")
    parser.add_argument("--control-cpus", required=True, help="pinned coordinator CPUs")
    parser.add_argument(
        "--expected-kernel",
        required=True,
        help="exact uname release containing the pinned kernel WireGuard implementation",
    )
    parser.add_argument(
        "--wireguard-go-bin",
        type=Path,
        help="pinned wireguard-go binary; defaults to PATH lookup",
    )
    parser.add_argument(
        "--expected-wireguard-go-version",
        help="exact output of wireguard-go --version",
    )
    parser.add_argument(
        "--expected-wireguard-go-sha256",
        help="exact SHA-256 of the wireguard-go binary",
    )
    parser.add_argument(
        "--allow-missing-wireguard-go",
        action="store_true",
        help="collect a partial matrix and report INCOMPLETE instead of fabricating data",
    )
    parser.add_argument(
        "--fuckipv6-bin",
        type=Path,
        default=PROJECT_ROOT / "target" / "release" / "fuckipv6",
    )
    parser.add_argument(
        "--coordinator-bin",
        type=Path,
        default=PROJECT_ROOT / "target" / "release" / "fuckipv6-coordinator",
    )
    parser.add_argument("--output", type=Path, help="result directory (must not contain prior evidence)")
    parser.add_argument("--max-load1", type=float, default=0.5)
    parser.add_argument("--max-cpu-busy-percent", type=float, default=5.0)
    parser.add_argument("--require-performance-governor", action="store_true")
    parser.add_argument("--throughput-min-ratio", type=float, default=1.05)
    parser.add_argument("--p99-max-ratio", type=float, default=1.0)
    parser.add_argument("--cpu-cost-max-ratio", type=float, default=1.0)
    parser.add_argument("--controlled-udp-max-loss-percent", type=float, default=1.0)
    parser.add_argument("--latency-interval-ms", type=float, default=10.0)
    parser.add_argument("--latency-timeout-seconds", type=float, default=1.0)
    parser.add_argument("--latency-min-samples", type=int, default=200)
    parser.add_argument("--latency-max-loss-percent", type=float, default=1.0)
    parser.add_argument(
        "--dry-run",
        action="store_true",
        help="validate parameters and print the plan without collecting evidence",
    )
    return parser.parse_args(argv)


def validate_args(args: argparse.Namespace) -> None:
    if args.runs < 1:
        raise ValueError("--runs must be positive")
    if args.duration < 2 or args.omit < 0 or args.duration <= args.omit + 1:
        raise ValueError("--duration must exceed --omit by at least two seconds")
    if args.mtu < 576:
        raise ValueError("--mtu must be at least 576")
    for name in (
        "max_load1",
        "max_cpu_busy_percent",
        "throughput_min_ratio",
        "p99_max_ratio",
        "cpu_cost_max_ratio",
        "controlled_udp_max_loss_percent",
        "latency_interval_ms",
        "latency_timeout_seconds",
        "latency_max_loss_percent",
    ):
        if getattr(args, name) < 0:
            raise ValueError(f"--{name.replace('_', '-')} must be non-negative")
    if args.throughput_min_ratio <= 0 or args.p99_max_ratio <= 0 or args.cpu_cost_max_ratio <= 0:
        raise ValueError("performance ratios must be positive")
    if args.latency_interval_ms <= 0 or args.latency_timeout_seconds <= 0:
        raise ValueError("latency interval and timeout must be positive")
    if args.latency_min_samples < 1:
        raise ValueError("--latency-min-samples must be positive")
    requested = _csv_or_space(args.impls)
    unsupported = sorted(set(requested) - set(IMPLEMENTATIONS))
    if unsupported:
        raise ValueError(f"unsupported implementations: {unsupported}")
    if len(requested) != len(set(requested)):
        raise ValueError("--impls contains duplicates")
    if "fuckipv6" not in requested:
        raise ValueError("--impls must include fuckipv6")
    if "wireguard-kernel" not in requested:
        raise ValueError("--impls must include the pinned kernel WireGuard comparator")
    if "wireguard-go" not in requested and not args.allow_missing_wireguard_go:
        raise ValueError(
            "wireguard-go is a required comparator; install it or pass "
            "--allow-missing-wireguard-go for explicitly incomplete evidence"
        )


def build_policy(args: argparse.Namespace) -> dict[str, Any]:
    policy = copy.deepcopy(DEFAULT_POLICY)
    if args.runs < policy["required_runs"]:
        raise ValueError(
            f"NX-17 requires at least {policy['required_runs']} runs per test; "
            f"got --runs {args.runs}"
        )
    policy.update(
        {
            "required_runs": args.runs,
            "throughput_min_ratio": args.throughput_min_ratio,
            "p99_max_ratio": args.p99_max_ratio,
            "cpu_cost_max_ratio": args.cpu_cost_max_ratio,
            "controlled_udp_max_loss_percent": args.controlled_udp_max_loss_percent,
        }
    )
    policy["traffic"].update(
        {
            "duration_seconds": args.duration,
            "omit_seconds": args.omit,
            "mtu": args.mtu,
        }
    )
    policy["environment"].update(
        {
            "load1_max": args.max_load1,
            "cpu_busy_percent_max": args.max_cpu_busy_percent,
            "require_performance_governor": args.require_performance_governor,
        }
    )
    policy["latency"].update(
        {
            "probe_interval_ms": args.latency_interval_ms,
            "probe_timeout_seconds": args.latency_timeout_seconds,
            "min_samples_per_run": args.latency_min_samples,
            "max_loss_percent": args.latency_max_loss_percent,
        }
    )
    policy["traffic_models"][-2]["controlled_rate"] = args.udp_1420_rate
    policy["traffic_models"][-1]["controlled_rate"] = args.udp_64_rate
    return policy


def _cpu_affinity(args: argparse.Namespace) -> dict[str, set[int]]:
    from harness import cpu_sets_disjoint, parse_cpu_list

    available = os.sched_getaffinity(0) if hasattr(os, "sched_getaffinity") else None
    cpu_sets = {
        "client": parse_cpu_list(args.client_cpus, available),
        "server": parse_cpu_list(args.server_cpus, available),
        "control": parse_cpu_list(args.control_cpus, available),
    }
    cpu_sets_disjoint(cpu_sets)
    return cpu_sets


def _require_executable(path: Path, label: str) -> Path:
    resolved = path.expanduser().resolve()
    if not resolved.is_file() or not os.access(resolved, os.X_OK):
        raise ValueError(f"{label} is missing or not executable: {resolved}")
    return resolved


def _require_command(name: str) -> Path:
    found = shutil.which(name)
    if not found:
        raise ValueError(f"required command is missing: {name}")
    return Path(found).resolve()


def _required_tools() -> dict[str, Path]:
    tools = {
        name: _require_command(name)
        for name in ("ip", "iperf3", "perf", "ping", "python3", "taskset", "wg")
    }
    tools["git"] = _require_command("git")
    return tools


def _git_value(*args: str) -> str | None:
    try:
        return subprocess.check_output(
            ["git", "-C", str(PROJECT_ROOT), *args],
            text=True,
            stderr=subprocess.DEVNULL,
            timeout=10,
        ).strip()
    except (OSError, subprocess.CalledProcessError, subprocess.TimeoutExpired):
        return None


def _resolve_wireguard_go(args: argparse.Namespace) -> tuple[dict[str, Any] | None, dict[str, Any] | None]:
    candidate = args.wireguard_go_bin
    if candidate is None:
        found = shutil.which("wireguard-go")
        candidate = Path(found) if found else None
    if candidate is None or not candidate.expanduser().resolve().is_file():
        reason = "wireguard-go was not found or --wireguard-go-bin is missing"
        if args.allow_missing_wireguard_go:
            return None, {"implementation": "wireguard-go", "reason": reason}
        raise ValueError(
            f"{reason}; install a pinned build or use --allow-missing-wireguard-go "
            "for an explicitly incomplete run"
        )

    path = _require_executable(candidate, "wireguard-go")
    version = subprocess.run(
        [str(path), "--version"], check=False, text=True, capture_output=True, timeout=10
    )
    if version.returncode != 0:
        raise ValueError(f"cannot read pinned wireguard-go version: {version.stderr.strip()}")
    version_text = (version.stdout or version.stderr).strip()
    actual_sha256 = sha256_file(path)
    if not args.expected_wireguard_go_version:
        raise ValueError("--expected-wireguard-go-version is required for wireguard-go evidence")
    if not args.expected_wireguard_go_sha256:
        raise ValueError("--expected-wireguard-go-sha256 is required for wireguard-go evidence")
    if version_text != args.expected_wireguard_go_version.strip():
        raise ValueError(
            f"wireguard-go version mismatch: got {version_text!r}, "
            f"expected {args.expected_wireguard_go_version!r}"
        )
    if actual_sha256 != args.expected_wireguard_go_sha256.strip().lower():
        raise ValueError(
            f"wireguard-go SHA-256 mismatch: got {actual_sha256}, "
            f"expected {args.expected_wireguard_go_sha256}"
        )
    return {
        "path": str(path),
        "version": version_text,
        "sha256": actual_sha256,
        "expected_version": args.expected_wireguard_go_version,
        "expected_sha256": args.expected_wireguard_go_sha256,
    }, None


def _new_output_dir(path: Path) -> Path:
    output = path.expanduser().resolve()
    if output.exists() and any(output.iterdir()):
        raise ValueError(f"output directory already contains files: {output}")
    output.mkdir(parents=True, exist_ok=True)
    return output


def _write_plan(output: Path, plan: dict[str, Any]) -> None:
    plan_path = output / "plan.json"
    write_json(plan_path, plan)
    (output / "plan.sha256").write_text(sha256_file(plan_path) + "\n", encoding="utf-8")


def write_evidence_manifest(output: Path) -> None:
    """Hash all retained evidence after collection without trusting result summaries."""
    excluded = {"evidence.sha256", "gate-report.json"}
    entries = []
    for path in sorted(output.rglob("*")):
        if not path.is_file():
            continue
        relative = path.relative_to(output).as_posix()
        if relative in excluded:
            continue
        entries.append(f"{sha256_file(path)}  {relative}")
    (output / "evidence.sha256").write_text(
        f"{EVIDENCE_MANIFEST_SCHEMA}\n" + "\n".join(entries) + "\n",
        encoding="utf-8",
    )


def _runner_error(output: Path, message: str) -> None:
    (output / "runner-error.txt").write_text(message.rstrip() + "\n", encoding="utf-8")


def _gate_summary(report: dict[str, Any], output: Path, exit_code: int) -> None:
    print(f"NX-17 gate: {report['status'].upper()} (exit {exit_code})")
    for error in report.get("errors", []):
        print(f"  ERROR: {error}", file=sys.stderr)
    for warning in report.get("warnings", []):
        print(f"  INCOMPLETE: {warning}", file=sys.stderr)
    for failure in report.get("failed_checks", []):
        print(f"  THRESHOLD FAILURE: {failure}", file=sys.stderr)
    print(f"JSON report: {output / 'gate-report.json'}")


def main(argv: list[str] | None = None) -> int:
    try:
        args = parse_args(argv)
        validate_args(args)
        policy = build_policy(args)
        cpu_sets = _cpu_affinity(args)
    except (OSError, ValueError) as error:
        print(f"benchmark-run: {error}", file=sys.stderr)
        return 1

    if args.dry_run:
        metadata = policy_metadata(policy)
        plan = {
            "schema": PLAN_SCHEMA,
            "created_utc": utc_now(),
            "evidence": False,
            "policy": policy,
            "policy_id": metadata["id"],
            "policy_sha256": metadata["sha256"],
            "policy_canonical": metadata["canonical"],
            "requested_implementations": _csv_or_space(args.impls),
            "implementations": _csv_or_space(args.impls),
            "skipped": [],
            "expected_kernel": args.expected_kernel,
            "cpu_affinity": {name: sorted(cpus) for name, cpus in cpu_sets.items()},
            "environment_limits": {
                "load1_max": args.max_load1,
                "cpu_busy_percent_max": args.max_cpu_busy_percent,
                "require_performance_governor": args.require_performance_governor,
            },
            "traffic": {
                "runs": args.runs,
                "duration_seconds": args.duration,
                "omit_seconds": args.omit,
                "mtu": args.mtu,
                "udp_1420_controlled_rate": args.udp_1420_rate,
                "udp_64_controlled_rate": args.udp_64_rate,
            },
            "latency_probe": {
                "implementation": "concurrent ICMPv6 echo through the tunnel",
                "interval_ms": args.latency_interval_ms,
                "timeout_seconds": args.latency_timeout_seconds,
                "aggregation": "nearest-rank p99 per run, median across runs",
            },
        }
        print(json.dumps(plan, indent=2, sort_keys=True))
        return 0

    if os.geteuid() != 0:
        print("benchmark-run: full collection must run as root (network namespaces/TUN)", file=sys.stderr)
        return 1
    requested = _csv_or_space(args.impls)
    output = args.output or PROJECT_ROOT / "target" / "benchmarks" / f"nx17-{time.strftime('%Y%m%dT%H%M%SZ', time.gmtime())}"
    try:
        extra_tools = _required_tools()
        if not DATAPLANE_SCRIPT.is_file() or not os.access(DATAPLANE_SCRIPT, os.X_OK):
            raise ValueError(f"dataplane runner is missing or not executable: {DATAPLANE_SCRIPT}")
        if not LATENCY_SHIM.is_file() or not os.access(LATENCY_SHIM, os.X_OK):
            raise ValueError(f"latency shim is missing or not executable: {LATENCY_SHIM}")
        fuckipv6_bin = _require_executable(args.fuckipv6_bin, "fuckipv6")
        coordinator_bin = _require_executable(args.coordinator_bin, "coordinator")
        output = _new_output_dir(output)
        if "wireguard-go" in requested:
            wireguard_go, skip = _resolve_wireguard_go(args)
        else:
            wireguard_go = None
            skip = {
                "implementation": "wireguard-go",
                "reason": "explicitly omitted with --allow-missing-wireguard-go",
            }
    except (OSError, ValueError, subprocess.TimeoutExpired) as error:
        print(f"benchmark-run: {error}", file=sys.stderr)
        return 1

    implementations = [impl for impl in requested if wireguard_go is not None or impl != "wireguard-go"]
    skipped = [skip] if skip is not None else []
    for comparator in COMPARATORS:
        if comparator not in implementations and not any(
            item.get("implementation") == comparator for item in skipped
        ):
            skipped.append({"implementation": comparator, "reason": "not selected"})

    metadata = policy_metadata(policy)
    git_status = _git_value("status", "--porcelain")
    plan = {
        "schema": PLAN_SCHEMA,
        "created_utc": utc_now(),
        "evidence": True,
        "policy": policy,
        "policy_id": metadata["id"],
        "policy_sha256": metadata["sha256"],
        "policy_canonical": metadata["canonical"],
        "canonical_policy_id": CANONICAL_POLICY_ID,
        "source": {
            "schema": SOURCE_SCHEMA,
            "git_commit": _git_value("rev-parse", "HEAD"),
            "git_tree": _git_value("rev-parse", "HEAD^{tree}"),
            "git_status": git_status,
            "dirty": bool(git_status),
        },
        "requested_implementations": requested,
        "implementations": implementations,
        "skipped": skipped,
        "expected_kernel": args.expected_kernel,
        "wireguard_go": wireguard_go,
        "binaries": {
            "fuckipv6": {"path": str(fuckipv6_bin), "sha256": sha256_file(fuckipv6_bin)},
            "coordinator": {"path": str(coordinator_bin), "sha256": sha256_file(coordinator_bin)},
            "wireguard-go": (
                {"path": wireguard_go["path"], "sha256": wireguard_go["sha256"]}
                if wireguard_go is not None
                else None
            ),
            "tools": {
                name: {"path": str(path), "sha256": sha256_file(path)}
                for name, path in extra_tools.items()
            },
        },
        "cpu_affinity": {name: sorted(cpus) for name, cpus in cpu_sets.items()},
        "environment_limits": {
            "load1_max": args.max_load1,
            "cpu_busy_percent_max": args.max_cpu_busy_percent,
            "require_performance_governor": args.require_performance_governor,
        },
        "traffic": {
            "runs": args.runs,
            "duration_seconds": args.duration,
            "omit_seconds": args.omit,
            "mtu": args.mtu,
            "udp_1420_controlled_rate": args.udp_1420_rate,
            "udp_64_controlled_rate": args.udp_64_rate,
        },
        "latency_probe": {
            "implementation": "concurrent ICMPv6 echo through the tunnel",
            "interval_ms": args.latency_interval_ms,
            "timeout_seconds": args.latency_timeout_seconds,
            "aggregation": "nearest-rank p99 per run, median across runs",
        },
    }
    _write_plan(output, plan)

    try:
        environment = sample_environment(
            expected_kernel=args.expected_kernel,
            quiet_load1_max=args.max_load1,
            quiet_busy_percent_max=args.max_cpu_busy_percent,
            cpu_sets=cpu_sets,
            wireguard_go=wireguard_go,
        )
        write_json(output / "environment.json", environment)
    except (OSError, ValueError) as error:
        _runner_error(output, f"environment capture failed: {error}")
        environment = {}

    preflight_failures = list(environment.get("quiet_host", {}).get("failures", []))
    if not environment:
        preflight_failures.append("environment capture failed")
    if args.require_performance_governor:
        governors = environment.get("frequency_governors", {})
        if not governors:
            preflight_failures.append("frequency governors are unavailable")
        else:
            non_performance = {
                cpu: governor
                for cpu, governor in governors.items()
                if governor != "performance"
            }
            if non_performance:
                preflight_failures.append(
                    f"frequency governors are not all performance: {non_performance}"
                )
    if preflight_failures:
        _runner_error(output, "; ".join(preflight_failures))
        write_evidence_manifest(output)
        report, exit_code = evaluate_output(output)
        _gate_summary(report, output, exit_code)
        return exit_code

    shim_dir = output / ".shim"
    shim_dir.mkdir(exist_ok=True)
    shim_path = shim_dir / "iperf3"
    if shim_path.exists():
        shim_path.unlink()
    shim_path.symlink_to(LATENCY_SHIM)

    env = os.environ.copy()
    env["PATH"] = f"{shim_dir}:{env.get('PATH', '')}"
    env["FIP6_BENCH_REAL_IPERF3"] = str(extra_tools["iperf3"])
    env["FIP6_BENCH_LATENCY_LOG"] = str(output / "latency-invocations.jsonl")
    env["FIP6_BENCH_LATENCY_DIR"] = str(output)
    env["FIP6_BENCH_PROBE_INTERVAL_MS"] = str(args.latency_interval_ms)
    env["FIP6_BENCH_PROBE_TIMEOUT_S"] = str(args.latency_timeout_seconds)
    env["FUCKIPV6_BIN"] = str(fuckipv6_bin)
    env["COORDINATOR_BIN"] = str(coordinator_bin)
    if wireguard_go is not None:
        env["WG_GO_BIN"] = wireguard_go["path"]

    command = [
        str(DATAPLANE_SCRIPT),
        "--impls",
        " ".join(implementations),
        "--runs",
        str(args.runs),
        "--duration",
        str(args.duration),
        "--omit",
        str(args.omit),
        "--mtu",
        str(args.mtu),
        "--udp-1420-rate",
        args.udp_1420_rate,
        "--udp-64-rate",
        args.udp_64_rate,
        "--client-cpus",
        args.client_cpus,
        "--server-cpus",
        args.server_cpus,
        "--control-cpus",
        args.control_cpus,
        "--output",
        str(output),
    ]
    plan["collection_command"] = command
    _write_plan(output, plan)

    with (output / "harness.log").open("w", encoding="utf-8") as log:
        process = subprocess.run(command, stdout=log, stderr=subprocess.STDOUT, env=env, check=False)
    if process.returncode != 0:
        _runner_error(output, f"bench-dataplane.sh exited with status {process.returncode}")

    write_evidence_manifest(output)
    report, exit_code = evaluate_output(output)
    _gate_summary(report, output, exit_code)
    return exit_code


if __name__ == "__main__":
    raise SystemExit(main())
