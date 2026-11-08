#!/usr/bin/env python3
"""Collect reproducible W10-09 governance evidence.

The report binds every security/privacy regression check to the checked-out
commit and to the policy/contract versions discovered in source. It is release
evidence for automated tests, not a substitute for an independent security
audit or live cross-platform validation.
"""

from __future__ import annotations

import argparse
import json
import os
import platform
import re
import subprocess
import sys
import time
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Sequence

REPORT_SCHEMA = "fuckipv6-governance-evidence-report"
REPORT_SCHEMA_VERSION = 1
PROJECT_ROOT = Path(__file__).resolve().parent.parent
DEFAULT_OUTPUT = PROJECT_ROOT / "target" / "governance-evidence" / "report.json"

ANSI_ESCAPE = re.compile(r"\x1b\[[0-9;]*m")
CARGO_RESULT = re.compile(
    r"test result: ok\. (\d+) passed; (\d+) failed; (\d+) ignored;"
)


@dataclass(frozen=True)
class VersionSource:
    key: str
    path: str
    pattern: str


@dataclass(frozen=True)
class CheckSpec:
    name: str
    category: str
    cwd: str
    command: tuple[str, ...]
    covers: tuple[str, ...]
    parser: str = "none"
    minimum_passed: int = 0


@dataclass(frozen=True)
class CoverageSpec:
    criterion: str
    checks: tuple[str, ...]
    requires_version_alignment: bool = False
    requires_stable_source: bool = False


VERSION_SOURCES = (
    VersionSource(
        "node_contract",
        "crates/client/src/node/contract.rs",
        r"pub const NODE_CONTRACT_SCHEMA_VERSION: u16 = (\d+)",
    ),
    VersionSource(
        "node_http",
        "crates/client/src/node/http_adapter.rs",
        r"pub const NODE_HTTP_SCHEMA_VERSION: u16 = (\d+)",
    ),
    VersionSource(
        "node_ui_manifest",
        "crates/client/src/node/ui_manifest.rs",
        r"pub const NODE_UI_MANIFEST_SCHEMA_VERSION: u16 = (\d+)",
    ),
    VersionSource(
        "policy_transaction",
        "crates/client/src/policy_transaction.rs",
        r"const SCHEMA_VERSION: u16 = (\d+)",
    ),
    VersionSource(
        "audit_chain",
        "crates/common/src/domain/audit.rs",
        r"pub const AUDIT_CHAIN_SCHEMA_VERSION: u32 = (\d+)",
    ),
    VersionSource(
        "typescript_contract",
        "contracts/typescript/node-contract-v1.ts",
        r"export const NODE_CONTRACT_SCHEMA_VERSION = (\d+)",
    ),
    VersionSource(
        "webui_http",
        "webui/src/contract/invariants.ts",
        r"export const HTTP_SCHEMA_VERSION = (\d+)",
    ),
    VersionSource(
        "webui_manifest",
        "webui/src/contract/invariants.ts",
        r"export const MANIFEST_SCHEMA_VERSION = (\d+)",
    ),
    VersionSource(
        "android_contract",
        "contracts/android/src/main/java/cc/xlei/fuckipv6/contract/NodeContractV1.java",
        r"public static final int SCHEMA_VERSION = (\d+)",
    ),
)

WIRE_VERSION_KEYS = (
    "node_contract",
    "node_http",
    "node_ui_manifest",
    "typescript_contract",
    "webui_http",
    "webui_manifest",
    "android_contract",
)

CHECKS = (
    CheckSpec(
        "access-control-combinations",
        "authorization",
        ".",
        (
            "cargo",
            "test",
            "-p",
            "fuckipv6-common",
            "--lib",
            "domain::access_control",
        ),
        ("role-group-local-veto-deny-precedence",),
        "cargo",
        1,
    ),
    CheckSpec(
        "coordinator-freeze-recovery",
        "authorization",
        ".",
        (
            "cargo",
            "test",
            "-p",
            "fuckipv6-common",
            "--lib",
            "domain::coordinator",
        ),
        ("temporary-coordinator-freeze-recovery",),
        "cargo",
        1,
    ),
    CheckSpec(
        "field-privacy-domain",
        "privacy",
        ".",
        (
            "cargo",
            "test",
            "-p",
            "fuckipv6-common",
            "--lib",
            "domain::field_privacy",
        ),
        ("field-level-read-search-export-activity",),
        "cargo",
        1,
    ),
    CheckSpec(
        "field-privacy-serialization",
        "privacy",
        ".",
        (
            "cargo",
            "test",
            "-p",
            "fuckipv6-client",
            "--all-features",
            "--lib",
            "node::privacy_projection",
        ),
        ("field-level-read-search-export-activity",),
        "cargo",
        1,
    ),
    CheckSpec(
        "policy-transaction-consistency",
        "policy",
        ".",
        (
            "cargo",
            "test",
            "-p",
            "fuckipv6-client",
            "--all-features",
            "--lib",
            "policy_transaction",
        ),
        ("concurrent-policy-apply-rollback-disconnect",),
        "cargo",
        1,
    ),
    CheckSpec(
        "policy-runtime-conflict",
        "policy",
        ".",
        (
            "cargo",
            "test",
            "-p",
            "fuckipv6-client",
            "--all-features",
            "--lib",
            "stale_policy_apply_returns_conflict_without_overwriting_the_winner",
        ),
        ("concurrent-policy-apply-rollback-disconnect",),
        "cargo",
        1,
    ),
    CheckSpec(
        "policy-runtime-concurrency",
        "policy",
        ".",
        (
            "cargo",
            "test",
            "-p",
            "fuckipv6-client",
            "--all-features",
            "--lib",
            "concurrent_policy_apply_and_rollback_cannot_move_the_revision_backwards",
        ),
        ("concurrent-policy-apply-rollback-disconnect",),
        "cargo",
        1,
    ),
    CheckSpec(
        "policy-runtime-disconnect",
        "policy",
        ".",
        (
            "cargo",
            "test",
            "-p",
            "fuckipv6-client",
            "--all-features",
            "--lib",
            "policy_apply_and_rollback_resume_after_disconnect_without_losing_revisions",
        ),
        ("concurrent-policy-apply-rollback-disconnect",),
        "cargo",
        1,
    ),
    CheckSpec(
        "audit-integrity-rotation-export",
        "audit",
        ".",
        (
            "cargo",
            "test",
            "-p",
            "fuckipv6-client",
            "--all-features",
            "--lib",
            "owner_security::audit",
        ),
        ("audit-tamper-disk-full-rotation-export",),
        "cargo",
        1,
    ),
    CheckSpec(
        "activity-object-links-gpui",
        "activity",
        ".",
        (
            "cargo",
            "test",
            "-p",
            "fuckipv6-client",
            "--all-features",
            "--lib",
            "activity_object_links_require_current_permission_existence_and_network_ownership",
        ),
        ("object-deletion-link-permission-checks",),
        "cargo",
        1,
    ),
    CheckSpec(
        "webui-privacy-activity-contract",
        "frontend",
        "webui",
        (
            "npm",
            "test",
            "--",
            "--run",
            "src/activity.test.ts",
            "src/members.test.ts",
            "src/contract/invariants.test.ts",
        ),
        (
            "field-level-read-search-export-activity",
            "object-deletion-link-permission-checks",
        ),
        "vitest",
        1,
    ),
    CheckSpec(
        "cross-surface-contract-alignment",
        "contracts",
        ".",
        ("./scripts/check-contracts.sh",),
        ("backend-gpui-webui-android-contract-alignment",),
        "none",
        0,
    ),
)

COVERAGE = (
    CoverageSpec(
        "role-group-local-veto-deny-precedence",
        ("access-control-combinations",),
    ),
    CoverageSpec(
        "temporary-coordinator-freeze-recovery",
        ("coordinator-freeze-recovery",),
    ),
    CoverageSpec(
        "field-level-read-search-export-activity",
        ("field-privacy-domain", "field-privacy-serialization", "webui-privacy-activity-contract"),
    ),
    CoverageSpec(
        "concurrent-policy-apply-rollback-disconnect",
        (
            "policy-transaction-consistency",
            "policy-runtime-conflict",
            "policy-runtime-concurrency",
            "policy-runtime-disconnect",
        ),
    ),
    CoverageSpec(
        "audit-tamper-disk-full-rotation-export",
        ("audit-integrity-rotation-export",),
    ),
    CoverageSpec(
        "object-deletion-link-permission-checks",
        ("activity-object-links-gpui", "webui-privacy-activity-contract"),
    ),
    CoverageSpec(
        "backend-gpui-webui-android-contract-alignment",
        ("cross-surface-contract-alignment",),
        requires_version_alignment=True,
    ),
    CoverageSpec(
        "evidence-bound-to-policy-version-and-commit",
        (),
        requires_version_alignment=True,
        requires_stable_source=True,
    ),
)


def utc_now() -> str:
    return datetime.now(timezone.utc).isoformat(timespec="seconds").replace("+00:00", "Z")


def write_json(path: Path, document: dict[str, Any]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_suffix(path.suffix + ".tmp")
    temporary.write_text(
        json.dumps(document, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    temporary.replace(path)


def report_path(root: Path, path: Path) -> str:
    try:
        return str(path.resolve().relative_to(root.resolve()))
    except ValueError:
        return str(path)


def command_output(
    root: Path, command: Sequence[str], *, timeout: int = 30
) -> str | None:
    try:
        completed = subprocess.run(
            list(command),
            cwd=root,
            check=False,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
            timeout=timeout,
        )
    except (OSError, subprocess.TimeoutExpired):
        return None
    if completed.returncode != 0:
        return None
    return completed.stdout.strip() or None


def source_checkpoint(root: Path) -> dict[str, Any]:
    return {
        "commit": command_output(root, ("git", "rev-parse", "HEAD")),
        "branch": command_output(root, ("git", "branch", "--show-current")),
        "dirty": bool(
            command_output(
                root,
                ("git", "status", "--porcelain", "--untracked-files=normal"),
            )
        ),
    }


def discover_policy_versions(root: Path) -> tuple[dict[str, int], list[str]]:
    versions: dict[str, int] = {}
    errors: list[str] = []
    for source in VERSION_SOURCES:
        path = root / source.path
        try:
            text = path.read_text(encoding="utf-8")
        except OSError as error:
            errors.append(f"{source.path}: {error}")
            continue
        match = re.search(source.pattern, text)
        if match is None:
            errors.append(f"{source.path}: version pattern not found")
            continue
        versions[source.key] = int(match.group(1))

    for path in sorted((root / "contracts" / "schema").glob("*.schema.json")):
        relative = path.relative_to(root).as_posix()
        try:
            document = json.loads(path.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError) as error:
            errors.append(f"{relative}: {error}")
            continue
        version = document.get("x-fuckipv6-contract-version")
        if not isinstance(version, int):
            errors.append(f"{relative}: x-fuckipv6-contract-version is missing")
            continue
        versions[f"schema:{path.name}"] = version

    return versions, errors


def version_report(versions: dict[str, int], errors: list[str]) -> dict[str, Any]:
    wire_versions = {key: versions[key] for key in WIRE_VERSION_KEYS if key in versions}
    schema_versions = {
        key: value for key, value in versions.items() if key.startswith("schema:")
    }
    wire_values = sorted(set(wire_versions.values()) | set(schema_versions.values()))
    aligned = not errors and len(wire_values) == 1 and len(wire_versions) == len(WIRE_VERSION_KEYS)
    if len(schema_versions) != 7:
        aligned = False
    return {
        "wire_contract": {
            "aligned": aligned,
            "versions": wire_versions,
            "schemas": schema_versions,
            "expected": wire_values[0] if len(wire_values) == 1 else None,
        },
        "policy_transaction": {
            "version": versions.get("policy_transaction"),
            "source": "crates/client/src/policy_transaction.rs",
        },
        "audit_chain": {
            "version": versions.get("audit_chain"),
            "source": "crates/common/src/domain/audit.rs",
        },
        "errors": errors,
    }


def parse_counts(parser: str, output: str) -> dict[str, int] | None:
    clean = ANSI_ESCAPE.sub("", output)
    if parser == "cargo":
        counts = {"passed": 0, "failed": 0, "ignored": 0}
        matches = CARGO_RESULT.findall(clean)
        if not matches:
            return None
        for passed, failed, ignored in matches:
            counts["passed"] += int(passed)
            counts["failed"] += int(failed)
            counts["ignored"] += int(ignored)
        return counts
    if parser == "vitest":
        counts = {"passed": 0, "failed": 0, "ignored": 0}
        match = re.search(r"^\s*Tests\s+(.+)$", clean, re.MULTILINE)
        if match is None:
            return None
        summary = match.group(1)
        for key in counts:
            value = re.search(rf"(\d+)\s+{key}", summary)
            if value is not None:
                counts[key] = int(value.group(1))
        return counts
    return None


def run_check(
    spec: CheckSpec,
    root: Path,
    log_dir: Path,
    environment: dict[str, str],
) -> dict[str, Any]:
    cwd = root / spec.cwd
    log_path = log_dir / f"{spec.name}.log"
    log_path.parent.mkdir(parents=True, exist_ok=True)
    print(f"[governance] {spec.name}: {' '.join(spec.command)}", flush=True)
    started = time.monotonic()
    exit_code: int | None = None
    output = ""
    error: str | None = None
    try:
        completed = subprocess.run(
            list(spec.command),
            cwd=cwd,
            env=environment,
            check=False,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
            timeout=3_600,
        )
        output = completed.stdout
        exit_code = completed.returncode
    except (OSError, subprocess.TimeoutExpired) as exc:
        error = str(exc)
    duration = round(time.monotonic() - started, 3)
    log_path.write_text(output, encoding="utf-8")
    if output:
        print(output, end="" if output.endswith("\n") else "\n", flush=True)

    counts = parse_counts(spec.parser, output)
    count_ok = spec.parser == "none" or (
        counts is not None and counts["passed"] >= spec.minimum_passed
    )
    passed = exit_code == 0 and count_ok and error is None
    result: dict[str, Any] = {
        "name": spec.name,
        "category": spec.category,
        "covers": list(spec.covers),
        "cwd": spec.cwd,
        "command": list(spec.command),
        "status": "passed" if passed else "failed",
        "exit_code": exit_code,
        "duration_seconds": duration,
        "log": report_path(root, log_path),
        "tests": counts,
    }
    if not count_ok:
        result["error"] = (
            f"expected at least {spec.minimum_passed} parsed passing tests, "
            f"observed {counts}"
        )
    elif error:
        result["error"] = error
    print(
        f"[governance] {spec.name}: {'PASS' if passed else 'FAIL'} ({duration:.3f}s)",
        flush=True,
    )
    return result


def coverage_report(
    results: list[dict[str, Any]],
    version_alignment: bool,
    source_stable: bool,
) -> list[dict[str, Any]]:
    status_by_name = {result["name"]: result["status"] == "passed" for result in results}
    coverage = []
    for spec in COVERAGE:
        passed = all(status_by_name.get(check, False) for check in spec.checks)
        if spec.requires_version_alignment:
            passed = passed and version_alignment
        if spec.requires_stable_source:
            passed = passed and source_stable
        coverage.append(
            {
                "criterion": spec.criterion,
                "status": "passed" if passed else "failed",
                "checks": list(spec.checks),
                "requires_version_alignment": spec.requires_version_alignment,
                "requires_stable_source": spec.requires_stable_source,
            }
        )
    return coverage


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, default=PROJECT_ROOT)
    parser.add_argument("--output", type=Path, default=DEFAULT_OUTPUT)
    parser.add_argument(
        "--require-clean",
        action="store_true",
        help="fail evidence collection unless the worktree is clean before and after",
    )
    return parser


def main(argv: list[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    root = args.root.resolve()
    output = args.output.resolve()
    log_dir = output.parent / "logs"
    started_at = utc_now()
    source_start = source_checkpoint(root)
    versions, version_errors = discover_policy_versions(root)
    versions_report = version_report(versions, version_errors)

    if args.require_clean and source_start["dirty"]:
        document = {
            "schema": REPORT_SCHEMA,
            "schema_version": REPORT_SCHEMA_VERSION,
            "status": "failed",
            "generated_at": utc_now(),
            "started_at": started_at,
            "source": {"start": source_start, "end": source_start, "stable": False},
            "require_clean": True,
            "policy_versions": versions_report,
            "checks": [],
            "coverage": coverage_report([], False, False),
            "error": "worktree must be clean before collecting governance evidence",
        }
        write_json(output, document)
        print(document["error"], file=sys.stderr)
        return 1

    environment = os.environ.copy()
    environment.setdefault("CARGO_TERM_COLOR", "never")
    environment.setdefault("CARGO_INCREMENTAL", "0")
    results = [run_check(spec, root, log_dir, environment) for spec in CHECKS]
    source_end = source_checkpoint(root)
    source_stable = (
        isinstance(source_start["commit"], str)
        and source_start["commit"] == source_end["commit"]
    )
    clean_ok = not args.require_clean or (
        not source_start["dirty"] and not source_end["dirty"]
    )
    checks_ok = all(result["status"] == "passed" for result in results)
    alignment_ok = versions_report["wire_contract"]["aligned"]
    coverage = coverage_report(results, alignment_ok, source_stable)
    coverage_ok = all(item["status"] == "passed" for item in coverage)
    status = "passed" if checks_ok and clean_ok and source_stable and coverage_ok else "failed"

    document = {
        "schema": REPORT_SCHEMA,
        "schema_version": REPORT_SCHEMA_VERSION,
        "status": status,
        "generated_at": utc_now(),
        "started_at": started_at,
        "require_clean": args.require_clean,
        "source": {
            "start": source_start,
            "end": source_end,
            "stable": source_stable,
            "clean": not source_start["dirty"] and not source_end["dirty"],
        },
        "policy_versions": versions_report,
        "checks": results,
        "coverage": coverage,
        "platform": {
            "system": platform.system(),
            "release": platform.release(),
            "machine": platform.machine(),
            "python": platform.python_version(),
        },
    }
    write_json(output, document)
    print(f"[governance] report: {output}")
    print(f"[governance] status: {status}")
    return 0 if status == "passed" else 1


if __name__ == "__main__":
    raise SystemExit(main())
