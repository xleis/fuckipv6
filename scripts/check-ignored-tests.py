#!/usr/bin/env python3
"""Keep every skipped test honest.

The roadmap forbids "green by disabling tests". A skipped test is acceptable
only when it says why, and when the runner it names actually exists. This checker
enforces two objective rules over the Rust workspace:

1. `#[ignore]` must carry a reason string (`#[ignore = "..."]`), long enough to
   describe the prerequisite rather than a placeholder such as `"todo"`.
2. Every `scripts/<name>.sh|py` referenced by a reason must exist, so a reason
   cannot point at a runner that was deleted or renamed.

It cannot judge whether the prerequisite itself is fair; it only makes the claim
checkable.
"""

from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path
from typing import Iterable

PROJECT_ROOT = Path(__file__).resolve().parent.parent
DEFAULT_SOURCES = PROJECT_ROOT / "crates"

BARE_IGNORE = re.compile(r"#\[ignore\]")
REASONED_IGNORE = re.compile(r'#\[ignore\s*=\s*("(?:[^"\\]|\\.)*")')
SCRIPT_REFERENCE = re.compile(r"scripts/[\w.-]+\.(?:sh|py)")

MIN_REASON_CHARS = 20
REPORT_SCHEMA = "fuckipv6-ignored-tests"


def rust_sources(root: Path) -> list[Path]:
    return sorted(root.rglob("*.rs"))


def check(sources: Path) -> dict:
    errors: list[str] = []
    files: list[str] = []
    skipped = 0
    referenced_scripts: set[str] = set()

    for path in rust_sources(sources):
        text = path.read_text(encoding="utf-8")
        label = str(path.relative_to(PROJECT_ROOT)) if path.is_relative_to(PROJECT_ROOT) else str(path)

        for match in REASONED_IGNORE.finditer(text):
            skipped += 1
            reason = match.group(1)
            try:
                reason_text = json.loads(reason)
            except json.JSONDecodeError:
                errors.append(f"{label}: unreadable ignore reason {reason}")
                continue
            if len(reason_text.strip()) < MIN_REASON_CHARS:
                errors.append(
                    f"{label}: ignore reason is too short ({reason_text!r}); "
                    "name the prerequisite or the runner"
                )
            for script in SCRIPT_REFERENCE.findall(reason_text):
                referenced_scripts.add(script)
                if not (PROJECT_ROOT / script).is_file():
                    errors.append(f"{label}: ignore reason references a missing runner {script}")

        for _ in BARE_IGNORE.finditer(text):
            skipped += 1
            errors.append(f"{label}: bare #[ignore] without a reason")

        if REASONED_IGNORE.search(text) or BARE_IGNORE.search(text):
            files.append(label)

    return {
        "schema": REPORT_SCHEMA,
        "status": "failed" if errors else "passed",
        "ignored_tests": skipped,
        "files": files,
        "runners": sorted(referenced_scripts),
        "errors": errors,
    }


def main(argv: Iterable[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--sources", type=Path, default=DEFAULT_SOURCES)
    parser.add_argument("--report", type=Path, default=None)
    parser.add_argument("--json", action="store_true")
    args = parser.parse_args(list(argv) if argv is not None else None)

    if not args.sources.is_dir():
        print(f"sources not found: {args.sources}", file=sys.stderr)
        return 2

    report = check(args.sources)
    if args.report:
        args.report.parent.mkdir(parents=True, exist_ok=True)
        args.report.write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    if args.json:
        print(json.dumps(report, ensure_ascii=False, indent=2))
    for error in report["errors"]:
        print(f"[FAIL] {error}", file=sys.stderr)
    print(
        f"check-ignored-tests: {report['status']} "
        f"({report['ignored_tests']} skipped tests in {len(report['files'])} files, "
        f"{len(report['errors'])} issues)",
        file=sys.stderr,
    )
    return 0 if report["status"] == "passed" else 1


if __name__ == "__main__":
    raise SystemExit(main())
