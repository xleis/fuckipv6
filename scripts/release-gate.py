#!/usr/bin/env python3
"""Reproducible engineering release gate with machine-readable reports.

The gate verifies build/test/lint/benchmark health. It does not certify the
protocol, replace a live cross-platform network test, or substitute for an
independent security audit.
"""

from __future__ import annotations

import argparse
import importlib.util
import json
import math
import os
import platform
import shutil
import subprocess
import sys
import time
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Iterable, Sequence

SCHEMA_VERSION = 1
EXPECTED_SCHEMA = "fuckipv6-release-gate-report"
PROJECT_ROOT = Path(__file__).resolve().parent.parent
DEFAULT_PLATFORMS = ("linux", "windows", "macos")
DEFAULT_LINUX_CROSS_TARGETS = (
    "x86_64-unknown-linux-musl",
    "x86_64-pc-windows-gnu",
)
COMMON_REQUIRED_CHECKS = (
    "format",
    "helper-tests",
    "single-binary",
    "docs",
    "check",
    "test",
    "clippy",
    "release-build",
    "stable-source",
    "clean-worktree",
)
PLATFORM_REQUIRED_CHECKS = {
    "linux": (
        "cross-check-x86_64-unknown-linux-musl",
        "cross-check-x86_64-pc-windows-gnu",
    ),
    "macos": (
        "cross-check-x86_64-apple-darwin",
        "cross-check-aarch64-apple-darwin",
    ),
    "windows": (),
}
UPSTREAM_JOBS = (
    "docs",
    "supply-chain",
    "gpui-smoke",
    "linux",
    "windows",
    "macos",
)
SECURITY_RELEASE_BLOCKERS = (
    "Secure Session v2 has not been integrated into the production UDP/TUN data plane.",
    "The planned hybrid X-Wing and composite Ed25519 + ML-DSA handshake is not implemented.",
    "An independent cryptographic and protocol security audit has not been completed.",
    "Long-duration WAN, DPI/QoS resistance, and real Windows/macOS runtime validation are incomplete.",
)

_COMPARATOR_SCRIPT = Path(__file__).with_name("criterion_compare.py")
_COMPARATOR_SPEC = importlib.util.spec_from_file_location(
    "criterion_compare", _COMPARATOR_SCRIPT
)
assert _COMPARATOR_SPEC is not None and _COMPARATOR_SPEC.loader is not None
criterion_compare = importlib.util.module_from_spec(_COMPARATOR_SPEC)
sys.modules[_COMPARATOR_SPEC.name] = criterion_compare
_COMPARATOR_SPEC.loader.exec_module(criterion_compare)


def utc_now() -> str:
    return datetime.now(timezone.utc).isoformat(timespec="seconds").replace("+00:00", "Z")


def write_json(path: Path, document: dict[str, Any]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_suffix(path.suffix + ".tmp")
    temporary.write_text(
        json.dumps(document, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    temporary.replace(path)


def command_output(command: Sequence[str], *, env: dict[str, str] | None = None) -> str | None:
    try:
        completed = subprocess.run(
            list(command),
            cwd=PROJECT_ROOT,
            env=env,
            check=False,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
            timeout=30,
        )
    except (OSError, subprocess.TimeoutExpired):
        return None
    if completed.returncode != 0:
        return None
    return completed.stdout.strip() or None


def git_output(*arguments: str) -> str | None:
    return command_output(("git", *arguments))


def source_checkpoint() -> dict[str, Any]:
    return {
        "commit": git_output("rev-parse", "HEAD"),
        "branch": git_output("branch", "--show-current"),
        "dirty": bool(git_output("status", "--porcelain", "--untracked-files=normal")),
    }


def source_stability_check(
    start: dict[str, Any], end: dict[str, Any]
) -> dict[str, Any]:
    start_commit = start.get("commit")
    end_commit = end.get("commit")
    stable = isinstance(start_commit, str) and start_commit == end_commit
    result: dict[str, Any] = {
        "name": "stable-source",
        "command": ["git", "rev-parse", "HEAD"],
        "status": "passed" if stable else "failed",
        "exit_code": 0 if stable else 1,
        "duration_seconds": 0.0,
        "log": None,
        "start_commit": start_commit,
        "end_commit": end_commit,
    }
    if not stable:
        result["error"] = "Git HEAD changed while the release gate was running"
    return result


def base_environment() -> dict[str, str]:
    env = os.environ.copy()
    env.setdefault("CARGO_TERM_COLOR", "never")
    env.setdefault("CARGO_INCREMENTAL", "0")
    return env


def rustup_tool_path(toolchain: str, tool: str) -> str:
    rustup = shutil.which("rustup")
    if rustup is None:
        raise ValueError("--toolchain requires rustup on PATH")
    path = command_output((rustup, "which", "--toolchain", toolchain, tool))
    if path is None:
        raise ValueError(f"rustup toolchain {toolchain!r} does not provide {tool}")
    return path


def toolchain_environment(toolchain: str | None) -> dict[str, str]:
    env = base_environment()
    if toolchain:
        cargo = rustup_tool_path(toolchain, "cargo")
        rustc = rustup_tool_path(toolchain, "rustc")
        rustdoc = rustup_tool_path(toolchain, "rustdoc")
        toolchain_bin = str(Path(rustc).parent)
        env["PATH"] = os.pathsep.join((toolchain_bin, env.get("PATH", "")))
        env["CARGO"] = cargo
        env["RUSTC"] = rustc
        env["RUSTDOC"] = rustdoc
        env["RUSTUP_TOOLCHAIN"] = toolchain
    return env


def cargo_prefix(toolchain: str | None, env: dict[str, str]) -> list[str]:
    if toolchain:
        command = [rustup_tool_path(toolchain, "cargo")]
    else:
        cargo = shutil.which("cargo", path=env.get("PATH"))
        if cargo is None:
            raise ValueError("cargo is not on PATH")
        command = [cargo]
    # A globally configured rustc wrapper can silently replace the selected
    # toolchain's rustc. The release gate must use one coherent toolchain.
    return [*command, "--config", 'build.rustc-wrapper=""']


def run_logged(
    name: str,
    command: Sequence[str],
    log_path: Path,
    *,
    env: dict[str, str],
) -> dict[str, Any]:
    log_path.parent.mkdir(parents=True, exist_ok=True)
    started = time.monotonic()
    printable = " ".join(command)
    print(f"[gate] {name}: {printable}", flush=True)
    exit_code: int | None = None
    error: str | None = None
    try:
        with log_path.open("wb") as log:
            process = subprocess.Popen(
                list(command),
                cwd=PROJECT_ROOT,
                env=env,
                stdout=subprocess.PIPE,
                stderr=subprocess.STDOUT,
                start_new_session=os.name != "nt",
            )
            assert process.stdout is not None
            for line in iter(process.stdout.readline, b""):
                sys.stdout.buffer.write(line)
                sys.stdout.buffer.flush()
                log.write(line)
                log.flush()
            exit_code = process.wait()
    except OSError as exc:
        error = str(exc)
        exit_code = None
    duration = round(time.monotonic() - started, 3)
    passed = exit_code == 0
    print(
        f"[gate] {name}: {'PASS' if passed else 'FAIL'} ({duration:.3f}s)",
        flush=True,
    )
    result: dict[str, Any] = {
        "name": name,
        "command": list(command),
        "status": "passed" if passed else "failed",
        "exit_code": exit_code,
        "duration_seconds": duration,
        "log": str(log_path.relative_to(PROJECT_ROOT)),
    }
    if error:
        result["error"] = error
    return result


def parse_criterion_samples(criterion_dir: Path) -> list[dict[str, Any]]:
    samples: list[dict[str, Any]] = []
    if not criterion_dir.is_dir():
        return samples
    for estimate_path in sorted(criterion_dir.glob("**/new/estimates.json")):
        try:
            estimate = json.loads(estimate_path.read_text(encoding="utf-8"))
            relative = estimate_path.parent.parent.relative_to(criterion_dir)
            mean = estimate["mean"]
            median = estimate["median"]
            sample_path = estimate_path.with_name("sample.json")
            sample_count = None
            if sample_path.is_file():
                sample_document = json.loads(
                    sample_path.read_text(encoding="utf-8")
                )
                times = sample_document.get("times")
                if isinstance(times, list) and times:
                    sample_count = len(times)
            sample = {
                "benchmark": "/".join(relative.parts),
                "mean_ns": mean["point_estimate"],
                "mean_ci_ns": [
                    mean["confidence_interval"]["lower_bound"],
                    mean["confidence_interval"]["upper_bound"],
                ],
                "median_ns": median["point_estimate"],
                "median_ci_ns": [
                    median["confidence_interval"]["lower_bound"],
                    median["confidence_interval"]["upper_bound"],
                ],
                "standard_error_ns": mean["standard_error"],
            }
            if sample_count is not None:
                sample["sample_count"] = sample_count
            samples.append(sample)
        except (KeyError, OSError, TypeError, ValueError, json.JSONDecodeError):
            samples.append(
                {
                    "benchmark": "/".join(
                        estimate_path.parent.parent.relative_to(criterion_dir).parts
                    ),
                    "error": "invalid Criterion estimate file",
                }
            )
    return samples


def valid_benchmark_samples(benchmark: dict[str, Any]) -> list[dict[str, Any]]:
    samples = benchmark.get("samples", [])
    if not isinstance(samples, list):
        return []
    return [
        sample
        for sample in samples
        if isinstance(sample, dict)
        and sample.get("error") is None
        and isinstance(sample.get("mean_ns"), (int, float))
        and sample["mean_ns"] > 0
    ]


def detect_platform_label() -> str:
    system = platform.system().lower()
    return {"linux": "linux", "windows": "windows", "darwin": "macos"}.get(system, system)


def default_cross_targets(platform_label: str) -> list[str]:
    if platform_label == "linux":
        return list(DEFAULT_LINUX_CROSS_TARGETS)
    return []


def required_check_names(platform_label: str) -> list[str]:
    """Return the checks a platform report must contain to be release-ready."""
    return [
        *COMMON_REQUIRED_CHECKS,
        *PLATFORM_REQUIRED_CHECKS.get(platform_label, ()),
    ]


def helper_test_arguments() -> list[str]:
    """Keep the plan and executable gate on one helper-test inventory."""
    return [
        sys.executable,
        "-m",
        "unittest",
        "scripts/test_bench_compare.py",
        "scripts/test_bench_summarize.py",
        "scripts/test_criterion_compare.py",
        "scripts/test_release_gate.py",
        "scripts/test_release_material.py",
        "scripts/test_check_single_binary.py",
        "scripts/test_check_docs.py",
        "-v",
    ]


def apply_run_defaults(args: argparse.Namespace) -> None:
    if args.platform is None:
        args.platform = detect_platform_label()
    if args.benchmark == "none" and args.benchmark_baseline:
        raise ValueError("--benchmark-baseline requires Criterion benchmarking")
    if args.benchmark_min_samples < 1:
        raise ValueError("--benchmark-min-samples must be at least 1")
    if (
        not math.isfinite(args.benchmark_max_regression_percent)
        or args.benchmark_max_regression_percent < 0
    ):
        raise ValueError(
            "--benchmark-max-regression-percent must be finite and non-negative"
        )
    if (
        not math.isfinite(args.benchmark_max_noise_percent)
        or not 0 < args.benchmark_max_noise_percent <= 100
    ):
        raise ValueError("--benchmark-max-noise-percent must be in (0, 100]")
    targets = [] if args.no_default_cross_targets else default_cross_targets(args.platform)
    for target in args.cross_target:
        if target not in targets:
            targets.append(target)
    args.cross_target = targets


def build_run_plan(args: argparse.Namespace) -> dict[str, Any]:
    commands = [
        {"name": "format", "arguments": ["fmt", "--all", "--", "--check"]},
        {
            "name": "helper-tests",
            "arguments": helper_test_arguments(),
        },
        {
            "name": "single-binary",
            "arguments": [sys.executable, "scripts/check-single-binary.py"],
        },
        {
            "name": "docs",
            "arguments": [
                sys.executable,
                "scripts/check-docs.py",
                "--report",
                "<artifact-dir>/docs-consistency.json",
            ],
        },
        {
            "name": "check",
            "arguments": ["check", "--workspace", "--all-targets", "--all-features"],
        },
        {"name": "test", "arguments": ["test", "--workspace", "--all-features"]},
        {
            "name": "clippy",
            "arguments": [
                "clippy",
                "--workspace",
                "--all-targets",
                "--all-features",
                "--",
                "-D",
                "warnings",
            ],
        },
        {
            "name": "release-build",
            "arguments": ["build", "--release", "--workspace", "--all-features"],
        },
    ]
    for target in args.cross_target:
        commands.append(
            {
                "name": f"cross-check-{target}",
                "arguments": [
                    "check",
                    "--workspace",
                    "--all-targets",
                    "--all-features",
                    "--target",
                    target,
                ],
            }
        )
    benchmark = None
    if args.benchmark != "none":
        arguments = [
            "bench",
            "-p",
            "fuckipv6-secure-session",
            "--features",
            "pq",
            "--bench",
            "dataplane",
            "--",
        ]
        if args.benchmark == "quick":
            arguments.append("--quick")
        arguments.append("--noplot")
        benchmark = {"name": "benchmark", "arguments": arguments}
    comparison = None
    if args.benchmark_baseline:
        comparison = {
            "name": "benchmark-comparison",
            "arguments": benchmark_comparison_arguments(
                args,
                baselines=[Path(path).resolve() for path in args.benchmark_baseline],
                candidate=Path("<benchmark-candidate-report>"),
                output=Path("<benchmark-comparison-report>"),
            ),
        }
    return {
        "platform": args.platform,
        "toolchain": args.toolchain or "PATH cargo",
        "cargo_target_dir": str(
            PROJECT_ROOT
            / "target"
            / "release-gate-build"
            / f"{args.platform}-{args.toolchain or 'path'}"
        ),
        "require_clean": args.require_clean,
        "cross_targets": list(args.cross_target),
        "benchmark": args.benchmark,
        "benchmark_baselines": [str(Path(path).resolve()) for path in args.benchmark_baseline],
        "benchmark_quality": {
            "max_regression_percent": args.benchmark_max_regression_percent,
            "max_noise_percent": args.benchmark_max_noise_percent,
            "min_samples": args.benchmark_min_samples,
        },
        "commands": commands,
        "benchmark_command": benchmark,
        "benchmark_comparison_command": comparison,
    }


def benchmark_comparison_arguments(
    args: argparse.Namespace,
    *,
    baselines: Sequence[Path],
    candidate: Path,
    output: Path,
) -> list[str]:
    arguments = [
        sys.executable,
        str(PROJECT_ROOT / "scripts" / "criterion_compare.py"),
    ]
    for baseline in baselines:
        arguments.extend(("--baseline", str(baseline)))
    arguments.extend(
        (
            "--candidate",
            str(candidate),
            "--output",
            str(output),
            "--max-regression-percent",
            str(args.benchmark_max_regression_percent),
            "--max-noise-percent",
            str(args.benchmark_max_noise_percent),
            "--min-samples",
            str(args.benchmark_min_samples),
        )
    )
    return arguments


def hardware_info() -> dict[str, Any]:
    return {
        "os_release": platform.release(),
        "machine": platform.machine(),
        "processor": platform.processor() or None,
        "cpu_count": os.cpu_count(),
    }


def run_gate(args: argparse.Namespace) -> int:
    if args.print_plan:
        print(json.dumps(build_run_plan(args), indent=2, sort_keys=True))
        return 0

    source_start = source_checkpoint()
    report_path = (
        Path(args.report).resolve()
        if args.report
        else PROJECT_ROOT / "target" / "release-gate" / f"{args.platform}.json"
    )
    artifact_root = report_path.parent / f"{args.platform}-artifacts"
    if artifact_root.exists():
        shutil.rmtree(artifact_root)
    artifact_root.mkdir(parents=True, exist_ok=True)

    env = toolchain_environment(args.toolchain)
    env["CARGO_TARGET_DIR"] = str(
        PROJECT_ROOT
        / "target"
        / "release-gate-build"
        / f"{args.platform}-{args.toolchain or 'path'}"
    )
    cargo = cargo_prefix(args.toolchain, env)
    checks: list[dict[str, Any]] = []

    def cargo_check(name: str, arguments: Sequence[str]) -> None:
        checks.append(
            run_logged(
                name,
                [*cargo, *arguments],
                artifact_root / f"{name}.log",
                env=env,
            )
        )

    cargo_check("format", ("fmt", "--all", "--", "--check"))
    checks.append(
        run_logged(
            "helper-tests",
            helper_test_arguments(),
            artifact_root / "helper-tests.log",
            env=env,
        )
    )
    checks.append(
        run_logged(
            "single-binary",
            (sys.executable, "scripts/check-single-binary.py"),
            artifact_root / "single-binary.log",
            env=env,
        )
    )
    checks.append(
        run_logged(
            "docs",
            (
                sys.executable,
                "scripts/check-docs.py",
                "--report",
                str(artifact_root / "docs-consistency.json"),
            ),
            artifact_root / "docs.log",
            env=env,
        )
    )
    cargo_check(
        "check",
        ("check", "--workspace", "--all-targets", "--all-features"),
    )
    cargo_check("test", ("test", "--workspace", "--all-features"))
    cargo_check(
        "clippy",
        (
            "clippy",
            "--workspace",
            "--all-targets",
            "--all-features",
            "--",
            "-D",
            "warnings",
        ),
    )
    cargo_check(
        "release-build",
        ("build", "--release", "--workspace", "--all-features"),
    )
    for target in args.cross_target:
        cargo_check(
            f"cross-check-{target}",
            (
                "check",
                "--workspace",
                "--all-targets",
                "--all-features",
                "--target",
                target,
            ),
        )

    benchmark: dict[str, Any] = {
        "status": "not_run",
        "mode": args.benchmark,
        "samples": [],
        "comparison": {
            "status": "not_run",
            "baseline": None,
            "reason": "no Criterion baseline configured",
        },
    }
    if args.benchmark != "none":
        criterion_dir = artifact_root / "criterion"
        criterion_dir.mkdir(parents=True, exist_ok=True)
        benchmark_env = env.copy()
        benchmark_env["CRITERION_HOME"] = str(criterion_dir)
        benchmark_args: list[str] = [
            *cargo,
            "bench",
            "-p",
            "fuckipv6-secure-session",
            "--features",
            "pq",
            "--bench",
            "dataplane",
            "--",
        ]
        if args.benchmark == "quick":
            benchmark_args.append("--quick")
        benchmark_args.append("--noplot")
        benchmark_result = run_logged(
            "benchmark",
            benchmark_args,
            artifact_root / "benchmark.log",
            env=benchmark_env,
        )
        benchmark.update(benchmark_result)
        benchmark["mode"] = args.benchmark
        benchmark["samples"] = parse_criterion_samples(criterion_dir)
        valid_samples = valid_benchmark_samples(benchmark)
        benchmark["valid_sample_count"] = len(valid_samples)
        if benchmark_result["status"] == "passed" and not valid_samples:
            benchmark["status"] = "failed"
            benchmark["error"] = "Criterion produced no valid machine-readable samples"

        if valid_samples:
            candidate_document = {
                "schema": EXPECTED_SCHEMA,
                "schema_version": SCHEMA_VERSION,
                "platform": {"label": args.platform},
                "hardware": hardware_info(),
                "benchmark": benchmark,
            }
            candidate_path = artifact_root / "benchmark-candidate.json"
            write_json(candidate_path, candidate_document)
            if args.benchmark_baseline:
                comparison_path = artifact_root / "benchmark-comparison.json"
                comparison_result = run_logged(
                    "benchmark-comparison",
                    benchmark_comparison_arguments(
                        args,
                        baselines=[
                            Path(path).resolve()
                            for path in args.benchmark_baseline
                        ],
                        candidate=candidate_path,
                        output=comparison_path,
                    ),
                    artifact_root / "benchmark-comparison.log",
                    env=env,
                )
                checks.append(comparison_result)
                comparison: dict[str, Any] = {
                    "status": comparison_result["status"],
                    "baselines": [
                        str(Path(path).resolve())
                        for path in args.benchmark_baseline
                    ],
                    "candidate": str(candidate_path.relative_to(PROJECT_ROOT)),
                    "report": str(comparison_path.relative_to(PROJECT_ROOT)),
                    "command": comparison_result["command"],
                    "exit_code": comparison_result["exit_code"],
                }
                try:
                    comparison_report = json.loads(
                        comparison_path.read_text(encoding="utf-8")
                    )
                    comparison["report_summary"] = comparison_report.get("summary")
                    comparison["selected_baseline"] = comparison_report.get(
                        "baseline"
                    )
                except (OSError, json.JSONDecodeError) as error:
                    comparison["report_error"] = str(error)
                    comparison["status"] = "failed"
                benchmark["comparison"] = comparison
                if comparison["status"] != "passed":
                    benchmark["status"] = "failed"
                    benchmark["error"] = (
                        "Criterion baseline comparison failed"
                    )

    source_end = source_checkpoint()
    dirty = source_end["dirty"]
    source = {
        "commit": source_end["commit"],
        "commit_at_start": source_start["commit"],
        "branch": source_end["branch"],
        "dirty": dirty,
    }
    checks.append(source_stability_check(source_start, source_end))
    if args.require_clean and dirty:
        checks.append(
            {
                "name": "clean-worktree",
                "command": ["git", "status", "--porcelain"],
                "status": "failed",
                "exit_code": 1,
                "duration_seconds": 0.0,
                "log": None,
                "error": "release reports require a clean Git worktree",
            }
        )
    else:
        checks.append(
            {
                "name": "clean-worktree",
                "command": ["git", "status", "--porcelain"],
                "status": "passed" if not dirty else "skipped",
                "exit_code": 0 if not dirty else None,
                "duration_seconds": 0.0,
                "log": None,
            }
        )

    rustc_path = (
        rustup_tool_path(args.toolchain, "rustc")
        if args.toolchain
        else shutil.which("rustc", path=env.get("PATH"))
    )
    cargo_path = (
        rustup_tool_path(args.toolchain, "cargo")
        if args.toolchain
        else shutil.which("cargo", path=env.get("PATH"))
    )
    rustc = command_output((rustc_path, "-vV"), env=env) if rustc_path else None
    cargo_version = command_output((cargo_path, "-V"), env=env) if cargo_path else None
    clippy_version = command_output((*cargo, "clippy", "-V"), env=env)
    failed_checks = [check["name"] for check in checks if check["status"] == "failed"]
    if benchmark["status"] == "failed":
        failed_checks.append("benchmark")
    overall_passed = not failed_checks
    check_statuses = {
        str(check.get("name")): check.get("status")
        for check in checks
        if isinstance(check, dict)
    }
    required_checks = required_check_names(args.platform)
    required_checks_not_passed = [
        name for name in required_checks if check_statuses.get(name) != "passed"
    ]
    report = {
        "schema": EXPECTED_SCHEMA,
        "schema_version": SCHEMA_VERSION,
        "generated_at": utc_now(),
        "platform": {
            "label": args.platform,
            "system": platform.system(),
            "python": platform.python_version(),
        },
        "hardware": hardware_info(),
        "source": source,
        "toolchain": {
            "requested": args.toolchain,
            "rustc": rustc,
            "cargo": cargo_version,
            "clippy": clippy_version,
        },
        "checks": checks,
        "benchmark": benchmark,
        "summary": {
            "status": "passed" if overall_passed else "failed",
            "commands_passed": overall_passed,
            "worktree_clean": not dirty,
            "required_checks": required_checks,
            "required_checks_not_passed": required_checks_not_passed,
            "release_ready": overall_passed
            and not dirty
            and not required_checks_not_passed,
            "failed_checks": failed_checks,
            "check_count": len(checks),
        },
        "release_policy": {
            "engineering_gate_qualified": overall_passed
            and not dirty
            and not required_checks_not_passed,
            "security_release_qualified": False,
            "independent_security_audit_required": True,
            "blockers": list(SECURITY_RELEASE_BLOCKERS),
        },
    }
    write_json(report_path, report)
    print(f"[gate] report: {report_path}", flush=True)
    return 0 if overall_passed else 1


def load_report(path: Path) -> dict[str, Any]:
    document = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(document, dict):
        raise ValueError(f"{path}: report root must be a JSON object")
    if document.get("schema") != EXPECTED_SCHEMA:
        raise ValueError(f"{path}: unsupported report schema")
    if document.get("schema_version") != SCHEMA_VERSION:
        raise ValueError(f"{path}: unsupported schema version")
    return document


def parse_upstream_results(values: Iterable[str]) -> tuple[dict[str, str], list[str]]:
    """Parse repeated ``JOB=RESULT`` arguments supplied by the CI aggregator."""
    results: dict[str, str] = {}
    errors: list[str] = []
    for value in values:
        name, separator, result = value.partition("=")
        if not separator or not name or not result:
            errors.append(f"invalid upstream result {value!r}; expected JOB=RESULT")
            continue
        if name in results:
            errors.append(f"duplicate upstream result for {name}")
            continue
        results[name] = result
    return results, errors


def validate_upstream_results(results: dict[str, str]) -> list[str]:
    """Require every workflow prerequisite to finish successfully."""
    errors: list[str] = []
    for name in UPSTREAM_JOBS:
        result = results.get(name)
        if result is None:
            errors.append(f"missing upstream job result: {name}")
        elif result != "success":
            errors.append(f"upstream job {name} did not succeed: {result}")
    unknown = sorted(set(results) - set(UPSTREAM_JOBS))
    if unknown:
        errors.append("unknown upstream job results: " + ", ".join(unknown))
    return errors


def aggregate_documents(
    reports: Iterable[tuple[Path, dict[str, Any]]],
    expected_platforms: Sequence[str],
    upstream_results: dict[str, str] | None = None,
) -> tuple[dict[str, Any], list[str]]:
    errors: list[str] = []
    upstream = upstream_results or {}
    errors.extend(validate_upstream_results(upstream))
    platform_reports: dict[str, dict[str, Any]] = {}
    commits: set[str] = set()
    for path, report in reports:
        platform_info = report.get("platform")
        label = platform_info.get("label") if isinstance(platform_info, dict) else None
        if not isinstance(label, str) or not label:
            errors.append(f"{path}: missing platform label")
            continue
        if label in platform_reports:
            errors.append(f"{path}: duplicate platform report for {label}")
            continue
        platform_reports[label] = report
        summary_info = report.get("summary")
        summary = summary_info if isinstance(summary_info, dict) else {}
        summary_status = summary.get("status")
        if summary_status != "passed":
            errors.append(f"{path}: platform checks did not pass")
        if summary.get("release_ready") is not True:
            errors.append(f"{path}: platform report is not release-ready")
        source_info = report.get("source")
        source = source_info if isinstance(source_info, dict) else {}
        commit = source.get("commit")
        if not isinstance(commit, str) or not commit:
            errors.append(f"{path}: missing Git commit")
        else:
            commits.add(commit)
        if source.get("dirty") is not False:
            errors.append(f"{path}: source worktree is not clean")

        checks = report.get("checks")
        if not isinstance(checks, list) or not checks:
            errors.append(f"{path}: report contains no checks")
        else:
            check_statuses = {
                str(check.get("name")): check.get("status")
                for check in checks
                if isinstance(check, dict)
            }
            for required in required_check_names(label):
                if check_statuses.get(required) != "passed":
                    errors.append(
                        f"{path}: required check {required} is missing or did not pass"
                    )
            for check in checks:
                if not isinstance(check, dict) or check.get("status") != "passed":
                    name = check.get("name") if isinstance(check, dict) else None
                    errors.append(f"{path}: check {name or '<unknown>'} did not pass")

    missing = [name for name in expected_platforms if name not in platform_reports]
    if missing:
        errors.append("missing platform reports: " + ", ".join(missing))
    if len(commits) != 1:
        errors.append("platform reports were generated from different or unknown commits")

    benchmarks: list[tuple[str, dict[str, Any]]] = []
    for label, report in platform_reports.items():
        benchmark_info = report.get("benchmark")
        benchmark = benchmark_info if isinstance(benchmark_info, dict) else {}
        if benchmark.get("status") == "passed" and valid_benchmark_samples(benchmark):
            benchmarks.append((label, benchmark))
    if not benchmarks:
        errors.append("no platform contains a successful machine-readable benchmark")

    macos = platform_reports.get("macos")
    if macos is not None:
        macos_checks = macos.get("checks")
        cross_checks = {
            check.get("name"): check.get("status")
            for check in macos_checks
            if isinstance(macos_checks, list)
            and isinstance(check, dict)
            and str(check.get("name", "")).startswith("cross-check-")
        }
        for target in ("x86_64-apple-darwin", "aarch64-apple-darwin"):
            if cross_checks.get(f"cross-check-{target}") != "passed":
                errors.append(f"macOS report is missing passed cross-check-{target}")

    aggregate = {
        "schema": "fuckipv6-release-gate-aggregate",
        "schema_version": SCHEMA_VERSION,
        "generated_at": utc_now(),
        "expected_platforms": list(expected_platforms),
        "upstream_jobs": {
            name: {
                "result": upstream.get(name),
                "passed": upstream.get(name) == "success",
            }
            for name in UPSTREAM_JOBS
        },
        "platforms": {
            label: {
                "summary": report.get("summary"),
                "source": report.get("source"),
                "toolchain": report.get("toolchain"),
                "benchmark": {
                    "status": (
                        report.get("benchmark", {}).get("status")
                        if isinstance(report.get("benchmark"), dict)
                        else None
                    ),
                    "mode": (
                        report.get("benchmark", {}).get("mode")
                        if isinstance(report.get("benchmark"), dict)
                        else None
                    ),
                    "sample_count": (
                        len(report.get("benchmark", {}).get("samples", []))
                        if isinstance(report.get("benchmark"), dict)
                        and isinstance(report.get("benchmark", {}).get("samples"), list)
                        else 0
                    ),
                },
            }
            for label, report in sorted(platform_reports.items())
        },
        "benchmarked_platforms": sorted(label for label, _ in benchmarks),
        "summary": {
            "status": "passed" if not errors else "failed",
            "release_ready": not errors,
            "errors": errors,
        },
        "release_policy": {
            "engineering_gate_qualified": not errors,
            "security_release_qualified": False,
            "independent_security_audit_required": True,
            "blockers": list(SECURITY_RELEASE_BLOCKERS),
        },
    }
    return aggregate, errors


def aggregate_gate(args: argparse.Namespace) -> int:
    loaded: list[tuple[Path, dict[str, Any]]] = []
    upstream_results, errors = parse_upstream_results(args.upstream_result)
    for value in args.reports:
        path = Path(value)
        try:
            loaded.append((path, load_report(path)))
        except (OSError, ValueError, json.JSONDecodeError) as exc:
            errors.append(str(exc))
    aggregate, aggregate_errors = aggregate_documents(
        loaded,
        args.require_platform,
        upstream_results,
    )
    all_errors = [*errors, *aggregate_errors]
    aggregate["summary"]["errors"] = all_errors
    aggregate["summary"]["status"] = "failed" if all_errors else "passed"
    aggregate["summary"]["release_ready"] = not all_errors
    aggregate["release_policy"]["engineering_gate_qualified"] = not all_errors
    output = Path(args.output).resolve()
    write_json(output, aggregate)
    print(f"[gate] aggregate report: {output}", flush=True)
    for error in aggregate["summary"]["errors"]:
        print(f"[gate] FAIL: {error}", file=sys.stderr, flush=True)
    return 0 if not aggregate["summary"]["errors"] else 1


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    subparsers = parser.add_subparsers(dest="command", required=True)

    run_parser = subparsers.add_parser("run", help="run checks for this host platform")
    run_parser.add_argument(
        "--platform",
        default=None,
        help="report platform label (default: auto-detect)",
    )
    run_parser.add_argument("--report", help="JSON report path")
    run_parser.add_argument("--toolchain", help="rustup toolchain, for example stable")
    run_parser.add_argument(
        "--cross-target",
        action="append",
        default=[],
        help="additional Cargo target to check; may be repeated",
    )
    run_parser.add_argument(
        "--benchmark",
        choices=("quick", "full", "none"),
        default="quick",
        help="Criterion benchmark mode (default: quick)",
    )
    run_parser.add_argument(
        "--benchmark-baseline",
        action="append",
        default=[],
        help=(
            "accepted release-gate report, bench-compare snapshot, or Criterion "
            "directory; may be repeated"
        ),
    )
    run_parser.add_argument(
        "--benchmark-max-regression-percent",
        type=float,
        default=criterion_compare.DEFAULT_MAX_REGRESSION_PERCENT,
        help="maximum allowed mean latency regression (default: 10)",
    )
    run_parser.add_argument(
        "--benchmark-max-noise-percent",
        type=float,
        default=criterion_compare.DEFAULT_MAX_NOISE_PERCENT,
        help="maximum accepted relative standard error (default: 5)",
    )
    run_parser.add_argument(
        "--benchmark-min-samples",
        type=int,
        default=criterion_compare.DEFAULT_MIN_SAMPLES,
        help="minimum Criterion samples per benchmark (default: 10)",
    )
    clean_group = run_parser.add_mutually_exclusive_group()
    clean_group.add_argument(
        "--require-clean",
        dest="require_clean",
        action="store_true",
        help="fail if the Git worktree contains changes (default)",
    )
    clean_group.add_argument(
        "--allow-dirty",
        dest="require_clean",
        action="store_false",
        help="diagnostic run that permits changes but never marks the report release-ready",
    )
    run_parser.set_defaults(require_clean=True)
    run_parser.add_argument(
        "--no-default-cross-targets",
        action="store_true",
        help="do not add the platform's default cross-compilation checks",
    )
    run_parser.add_argument(
        "--print-plan",
        action="store_true",
        help="print the resolved JSON command plan without executing it",
    )
    run_parser.set_defaults(func=run_gate)

    aggregate_parser = subparsers.add_parser(
        "aggregate", help="combine platform reports into one release decision"
    )
    aggregate_parser.add_argument("--reports", nargs="+", required=True)
    aggregate_parser.add_argument("--output", required=True)
    aggregate_parser.add_argument(
        "--upstream-result",
        action="append",
        required=True,
        metavar="JOB=RESULT",
        help=(
            "GitHub Actions job result; required for docs, supply-chain, "
            "gpui-smoke, linux, windows, and macos"
        ),
    )
    aggregate_parser.add_argument(
        "--require-platform",
        action="append",
        default=[],
        help="required platform label; defaults to linux, windows, and macos",
    )
    aggregate_parser.set_defaults(func=aggregate_gate)
    return parser


def main() -> int:
    args = build_parser().parse_args()
    if args.command == "run":
        apply_run_defaults(args)
    if args.command == "aggregate" and not args.require_platform:
        args.require_platform = list(DEFAULT_PLATFORMS)
    try:
        return args.func(args)
    except (OSError, ValueError) as exc:
        print(f"release-gate: {exc}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
