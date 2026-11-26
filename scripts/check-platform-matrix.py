#!/usr/bin/env python3
"""Keep the platform support table honest and in sync with the release workflow.

Rules enforced here:

1. Every target the release workflow builds has a row classified `shipped`.
2. Every row carries an evidence token that resolves, or an explicit
   `unsupported:`/`unverified:` reason. Empty promises are rejected.
3. No row may leave the classification or evidence cell blank.
"""

from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path
from typing import Iterable

PROJECT_ROOT = Path(__file__).resolve().parent.parent
DEFAULT_MATRIX = PROJECT_ROOT / "docs" / "PLATFORM_SUPPORT.zh-CN.md"
DEFAULT_RELEASE_WORKFLOW = PROJECT_ROOT / ".github" / "workflows" / "release.yml"

ROW = re.compile(r"^\|\s*`?([^|`]+?)`?\s*\|\s*([^|]+?)\s*\|\s*([^|]+?)\s*\|\s*$")
CATEGORIES = {"shipped", "unsupported", "unverified"}
REPORT_SCHEMA = "fuckipv6-platform-matrix"

TARGET_PATTERNS = (
    re.compile(r"--target\s+([a-z0-9_]+-[a-z0-9_]+-[a-z0-9_]+)"),
    re.compile(r"targets:\s*([a-z0-9_]+-[a-z0-9_]+-[a-z0-9_]+)"),
    re.compile(r"target:\s*([a-z0-9_]+-[a-z0-9_]+-[a-z0-9_]+)"),
)


def release_targets(workflow: Path) -> set[str]:
    text = workflow.read_text(encoding="utf-8")
    targets: set[str] = set()
    for pattern in TARGET_PATTERNS:
        targets.update(pattern.findall(text))
    return targets


def matrix_rows(matrix: Path) -> list[dict]:
    rows: list[dict] = []
    for line in matrix.read_text(encoding="utf-8").splitlines():
        match = ROW.match(line)
        if not match:
            continue
        platform, category, evidence = (part.strip() for part in match.groups())
        if platform.lower() in {"平台 / 架构", "平台/架构"} or set(platform) <= {"-"}:
            continue
        rows.append({"platform": platform, "category": category, "evidence": evidence})
    return rows


def evidence_is_valid(evidence: str) -> bool:
    if evidence.startswith("path:"):
        return (PROJECT_ROOT / evidence[len("path:") :].strip()).exists()
    if evidence.startswith("doc:"):
        reference = evidence[len("doc:") :].strip()
        path = PROJECT_ROOT / reference.split("#", 1)[0].strip()
        return path.is_file()
    if evidence.startswith("unsupported:"):
        return len(evidence[len("unsupported:") :].strip()) >= 20
    if evidence.startswith("unverified:"):
        return len(evidence[len("unverified:") :].strip()) >= 20
    return False


def check(matrix: Path, workflow: Path) -> dict:
    errors: list[str] = []
    rows = matrix_rows(matrix)
    targets = release_targets(workflow)

    if not rows:
        errors.append("no matrix rows parsed; the table format changed")
    if not targets:
        errors.append("no release targets parsed; the workflow format changed")

    by_platform: dict[str, dict] = {}
    for row in rows:
        platform = row["platform"].strip()
        if platform in by_platform:
            errors.append(f"duplicate row for {platform}")
        by_platform[platform] = row
        if row["category"] not in CATEGORIES:
            errors.append(f"{platform}: unknown category {row['category']!r}")
        if not row["evidence"]:
            errors.append(f"{platform}: empty evidence cell")
        elif not evidence_is_valid(row["evidence"]):
            errors.append(f"{platform}: evidence does not resolve: {row['evidence']}")

    for target in sorted(targets):
        row = by_platform.get(target) or next(
            (value for key, value in by_platform.items() if target in key),
            None,
        )
        if row is None:
            errors.append(f"release target {target} has no row in the support table")
        elif row["category"] != "shipped":
            errors.append(
                f"release target {target} is classified {row['category']!r} but the workflow ships it"
            )

    return {
        "schema": REPORT_SCHEMA,
        "status": "failed" if errors else "passed",
        "release_targets": sorted(targets),
        "rows": rows,
        "errors": errors,
    }


def main(argv: Iterable[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--matrix", type=Path, default=DEFAULT_MATRIX)
    parser.add_argument("--workflow", type=Path, default=DEFAULT_RELEASE_WORKFLOW)
    parser.add_argument("--report", type=Path, default=None)
    parser.add_argument("--json", action="store_true")
    args = parser.parse_args(list(argv) if argv is not None else None)

    for path in (args.matrix, args.workflow):
        if not path.is_file():
            print(f"input not found: {path}", file=sys.stderr)
            return 2

    report = check(args.matrix, args.workflow)
    if args.report:
        args.report.parent.mkdir(parents=True, exist_ok=True)
        args.report.write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    if args.json:
        print(json.dumps(report, ensure_ascii=False, indent=2))
    for error in report["errors"]:
        print(f"[FAIL] {error}", file=sys.stderr)
    print(
        f"check-platform-matrix: {report['status']} "
        f"({len(report['rows'])} rows, {len(report['release_targets'])} release targets, "
        f"{len(report['errors'])} issues)",
        file=sys.stderr,
    )
    return 0 if report["status"] == "passed" else 1


if __name__ == "__main__":
    raise SystemExit(main())
