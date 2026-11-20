#!/usr/bin/env python3
"""Collect reproducible evidence for the W14 Coordinator takeover protocol.

This harness re-runs the protocol, election, persistence, client-decision and
fault-injection checks that the external audit package points at
(`docs/W14_COORDINATOR_AUDIT_PACKAGE.zh-CN.md`), binds the run to the exact Git
commit and writes one JSON report plus per-check logs.

It produces *evidence*, never an audit verdict: the report says
`verdict: "collected"` and explicitly refuses to claim the protocol has been
independently audited.
"""

from __future__ import annotations

import argparse
import json
import os
import platform
import re
import shutil
import subprocess
import sys
import time
from dataclasses import dataclass, field
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Iterable, Sequence

REPORT_SCHEMA = "fuckipv6-w14-coordinator-audit-evidence"
REPORT_SCHEMA_VERSION = 1
PROJECT_ROOT = Path(__file__).resolve().parent.parent
DEFAULT_OUTPUT = PROJECT_ROOT / "target" / "w14-coordinator-audit" / "evidence.json"

CARGO_RESULT = re.compile(
    r"test result: (ok|FAILED)\. (\d+) passed; (\d+) failed; (\d+) ignored;"
)
ANSI_ESCAPE = re.compile(r"\x1b\[[0-9;]*m")


@dataclass(frozen=True)
class CheckSpec:
    """One reproducible audit check."""

    check_id: str
    evidence_ids: tuple[str, ...]
    purpose: str
    command: tuple[str, ...]
    env: tuple[tuple[str, str], ...] = ()
    expect_passed: int = 1
    timeout_seconds: int = 900
    slow: bool = False
    requires_root: bool = False


def checks(depth: int, release: bool) -> list[CheckSpec]:
    profile = ("--release",) if release else ()
    sweep_env = (("FUCKIPV6_W14_INTERLEAVING_DEPTH", str(depth)),)
    return [
        CheckSpec(
            check_id="domain-invariants",
            evidence_ids=("E-D1", "E-D2", "E-D3", "E-S5", "E-S6"),
            purpose="domain validation: signatures, declarations, majority, term monotonicity, leases",
            command=(
                "cargo",
                "test",
                *profile,
                "-p",
                "fuckipv6-common",
                "domain::coordinator",
                "--lib",
            ),
            expect_passed=20,
            timeout_seconds=1800,
        ),
        CheckSpec(
            check_id="candidate-store",
            evidence_ids=(
                "E-D4",
                "E-D5",
                "E-D6",
                "E-S1",
                "E-S2",
                "E-S3",
                "E-S4",
                "E-E1",
                "E-E2",
                "E-G1",
                "E-F1",
            ),
            purpose="persistence CAS, election transactions, term install, emergency lease, governance gate",
            command=(
                "cargo",
                "test",
                *profile,
                "-p",
                "fuckipv6-coordinator",
                "signaling::coordinator_candidates",
                "--lib",
            ),
            expect_passed=30,
            timeout_seconds=3600,
        ),
        CheckSpec(
            check_id="interleaving-sweep",
            evidence_ids=("E-X2",),
            purpose=(
                "bounded exhaustive interleavings over the real store with depth "
                f"{depth} (invariants: strict majority, monotonic term/fencing, "
                "single committed election, emergency never writable, rejected actions change nothing)"
            ),
            command=(
                "cargo",
                "test",
                *profile,
                "-p",
                "fuckipv6-coordinator",
                "signaling::coordinator_candidates::tests::bounded_exhaustive",
                "--lib",
            ),
            env=sweep_env,
            expect_passed=1,
            timeout_seconds=7200,
        ),
        CheckSpec(
            check_id="client-takeover",
            evidence_ids=("E-K1", "E-K2"),
            purpose="client takeover decisions, poll backoff, reconnect wakeup, data-plane preservation",
            command=(
                "cargo",
                "test",
                *profile,
                "-p",
                "fuckipv6-client",
                "node::coordinator_takeover",
                "--lib",
            ),
            expect_passed=10,
            timeout_seconds=3600,
        ),
        CheckSpec(
            check_id="client-runtime",
            evidence_ids=("E-K2",),
            purpose="runtime authority refresh and reconnect wakeup",
            command=(
                "cargo",
                "test",
                *profile,
                "-p",
                "fuckipv6-client",
                "node::runtime_control::tests::coordinator_takeover",
                "--lib",
            ),
            expect_passed=1,
            timeout_seconds=3600,
        ),
        CheckSpec(
            check_id="fault-process",
            evidence_ids=("E-C1", "E-F1"),
            purpose="independent-process SIGKILL/SIGSTOP matrix, concurrent term install single winner",
            command=(
                "cargo",
                "test",
                *profile,
                "-p",
                "fuckipv6-coordinator",
                "--test",
                "w14_coordinator_fault_process",
            ),
            expect_passed=1,
            timeout_seconds=3600,
        ),
        CheckSpec(
            check_id="admin-recovery",
            evidence_ids=("E-A1", "E-A2"),
            purpose="administrator threshold recovery across real processes and offline trustees",
            command=(
                "cargo",
                "test",
                *profile,
                "-p",
                "fuckipv6-coordinator",
                "--test",
                "w14_admin_recovery_multinode",
            ),
            expect_passed=2,
            timeout_seconds=1800,
        ),
        CheckSpec(
            check_id="admin-recovery-websocket",
            evidence_ids=("E-A1",),
            purpose="real WebSocket Ack ordering and old-session invalidation",
            command=(
                "cargo",
                "test",
                *profile,
                "-p",
                "fuckipv6-coordinator",
                "--test",
                "w14_admin_recovery_flow",
            ),
            expect_passed=1,
            timeout_seconds=1800,
        ),
        CheckSpec(
            check_id="netns-partition",
            evidence_ids=("E-F2",),
            purpose="real network namespaces plus nftables partition and recovery",
            command=("./scripts/test-w14-coordinator-fault-netns.sh",),
            expect_passed=1,
            timeout_seconds=3600,
            slow=True,
            requires_root=True,
        ),
        CheckSpec(
            check_id="powerloss-vm",
            evidence_ids=("E-C2",),
            purpose="QEMU/KVM power cut after commit, then fsck and reboot on the same disk",
            command=("./scripts/test-w14-coordinator-powerloss-vm.sh",),
            expect_passed=1,
            timeout_seconds=3600,
            slow=True,
            requires_root=True,
        ),
    ]


def utc_now() -> str:
    return datetime.now(timezone.utc).isoformat(timespec="seconds").replace("+00:00", "Z")


def run_command(
    command: Sequence[str],
    cwd: Path,
    env: dict[str, str],
    timeout_seconds: int,
) -> tuple[int, str, float]:
    started = time.monotonic()
    try:
        completed = subprocess.run(
            list(command),
            cwd=cwd,
            env=env,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            timeout=timeout_seconds,
            check=False,
            text=True,
            errors="replace",
        )
        output = completed.stdout or ""
        exit_code = completed.returncode
    except subprocess.TimeoutExpired as expired:
        output = (expired.stdout or "") + f"\n[timeout after {timeout_seconds}s]"
        if isinstance(output, bytes):
            output = output.decode("utf-8", "replace")
        exit_code = 124
    except FileNotFoundError as missing:
        output = f"[missing executable] {missing}"
        exit_code = 127
    return exit_code, ANSI_ESCAPE.sub("", output), round(time.monotonic() - started, 3)


def parse_counts(output: str) -> tuple[int, int, int]:
    passed = failed = ignored = 0
    for match in CARGO_RESULT.finditer(output):
        passed += int(match.group(2))
        failed += int(match.group(3))
        ignored += int(match.group(4))
    return passed, failed, ignored


def git_state(root: Path) -> dict[str, Any]:
    def capture(*args: str) -> str:
        completed = subprocess.run(
            ["git", *args],
            cwd=root,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            text=True,
            check=False,
        )
        return completed.stdout.strip()

    status = capture("status", "--porcelain")
    return {
        "commit": capture("rev-parse", "HEAD") or None,
        "branch": capture("rev-parse", "--abbrev-ref", "HEAD") or None,
        "dirty": bool(status),
        "dirty_entries": status.splitlines()[:50],
    }


def rust_version(root: Path) -> str | None:
    completed = subprocess.run(
        ["cargo", "--version"],
        cwd=root,
        stdout=subprocess.PIPE,
        stderr=subprocess.DEVNULL,
        text=True,
        check=False,
    )
    return completed.stdout.strip() or None


def select_checks(args: argparse.Namespace) -> list[CheckSpec]:
    selected = checks(args.depth, args.release)
    if args.only:
        wanted = {item.strip() for item in args.only.split(",") if item.strip()}
        unknown = wanted.difference(spec.check_id for spec in selected)
        if unknown:
            raise SystemExit(f"unknown check ids: {sorted(unknown)}")
        selected = [spec for spec in selected if spec.check_id in wanted]
    if args.skip_slow:
        selected = [spec for spec in selected if not spec.slow]
    is_root = hasattr(os, "geteuid") and os.geteuid() == 0
    runnable = []
    for spec in selected:
        if spec.requires_root and not is_root:
            runnable.append(spec)
            continue
        runnable.append(spec)
    return runnable


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, default=DEFAULT_OUTPUT)
    parser.add_argument(
        "--depth",
        type=int,
        default=3,
        choices=(0, 1, 2, 3),
        help="interleaving sweep depth; 3 enumerates every ordered triple",
    )
    parser.add_argument("--release", action="store_true", help="run cargo checks in release mode")
    parser.add_argument(
        "--skip-slow",
        action="store_true",
        help="skip the root-only netns and QEMU power-loss drills",
    )
    parser.add_argument("--only", default="", help="comma separated check ids")
    parser.add_argument(
        "--require-clean",
        action="store_true",
        help="fail unless the worktree is clean before and after the run",
    )
    parser.add_argument("--list", action="store_true", help="print the check matrix and exit")
    parser.add_argument("--json", action="store_true", help="print the report to stdout")
    args = parser.parse_args(argv)

    selected = select_checks(args)
    if args.list:
        for spec in selected:
            print(f"{spec.check_id:24s} {','.join(spec.evidence_ids):28s} {spec.purpose}")
        return 0

    if not shutil.which("cargo"):
        print("cargo is required", file=sys.stderr)
        return 2

    before = git_state(PROJECT_ROOT)
    if args.require_clean and before["dirty"]:
        print("worktree is dirty; rerun after committing", file=sys.stderr)
        return 2

    output_path: Path = args.output
    output_path.parent.mkdir(parents=True, exist_ok=True)
    log_dir = output_path.parent / "logs"
    log_dir.mkdir(parents=True, exist_ok=True)

    base_env = os.environ.copy()
    base_env.setdefault("CARGO_TERM_COLOR", "never")

    results: list[dict[str, Any]] = []
    is_root = hasattr(os, "geteuid") and os.geteuid() == 0
    failures = 0
    for spec in selected:
        if spec.requires_root and not is_root:
            results.append(
                {
                    "check_id": spec.check_id,
                    "evidence_ids": list(spec.evidence_ids),
                    "purpose": spec.purpose,
                    "command": list(spec.command),
                    "status": "skipped",
                    "skip_reason": "requires root",
                    "passed": 0,
                    "failed": 0,
                    "ignored": 0,
                    "exit_code": None,
                }
            )
            print(f"[skip] {spec.check_id}: requires root", file=sys.stderr)
            continue

        env = base_env.copy()
        for key, value in spec.env:
            env[key] = value
        print(f"[run ] {spec.check_id}: {' '.join(spec.command)}", file=sys.stderr)
        exit_code, output, duration = run_command(
            spec.command, PROJECT_ROOT, env, spec.timeout_seconds
        )
        passed, failed, ignored = parse_counts(output)
        log_path = log_dir / f"{spec.check_id}.log"
        log_path.write_text(output, encoding="utf-8")
        status = "passed" if exit_code == 0 and failed == 0 else "failed"
        if status == "failed":
            failures += 1
        results.append(
            {
                "check_id": spec.check_id,
                "evidence_ids": list(spec.evidence_ids),
                "purpose": spec.purpose,
                "command": list(spec.command),
                "env": {key: value for key, value in spec.env},
                "status": status,
                "passed": passed,
                "failed": failed,
                "ignored": ignored,
                "expect_passed_at_least": spec.expect_passed,
                "meets_baseline": passed >= spec.expect_passed,
                "exit_code": exit_code,
                "duration_seconds": duration,
                "log": str(log_path.relative_to(PROJECT_ROOT)),
            }
        )
        print(
            f"[{'ok  ' if status == 'passed' else 'fail'}] {spec.check_id}: "
            f"{passed} passed, {failed} failed, {ignored} ignored ({duration:.1f}s)",
            file=sys.stderr,
        )

    after = git_state(PROJECT_ROOT)
    if args.require_clean and after["dirty"]:
        failures += 1
        print("worktree became dirty during the run", file=sys.stderr)

    totals = {
        "checks": len(results),
        "passed_checks": sum(1 for item in results if item["status"] == "passed"),
        "failed_checks": sum(1 for item in results if item["status"] == "failed"),
        "skipped_checks": sum(1 for item in results if item["status"] == "skipped"),
        "tests_passed": sum(item.get("passed", 0) for item in results),
        "tests_failed": sum(item.get("failed", 0) for item in results),
        "tests_ignored": sum(item.get("ignored", 0) for item in results),
    }
    coverage: dict[str, list[str]] = {}
    for item in results:
        for evidence_id in item["evidence_ids"]:
            coverage.setdefault(evidence_id, []).append(item["check_id"])

    report = {
        "schema": REPORT_SCHEMA,
        "schema_version": REPORT_SCHEMA_VERSION,
        "generated_at": utc_now(),
        "verdict": "collected",
        "verdict_note": (
            "Evidence collection only. This report does not audit the protocol and must not be "
            "quoted as an independent security audit."
        ),
        "git": {"before": before, "after": after},
        "toolchain": {"cargo": rust_version(PROJECT_ROOT)},
        "host": {
            "platform": platform.platform(),
            "python": platform.python_version(),
            "root": is_root,
        },
        "sweep_depth": args.depth,
        "release_profile": bool(args.release),
        "totals": totals,
        "coverage": coverage,
        "checks": results,
    }

    output_path.write_text(json.dumps(report, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
    if args.json:
        print(json.dumps(report, indent=2, ensure_ascii=False))
    print(f"report: {output_path}", file=sys.stderr)
    return 0 if failures == 0 else 1


if __name__ == "__main__":
    raise SystemExit(main())
