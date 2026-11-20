#!/usr/bin/env python3
"""Verify every literal Android UI string key exists in both locales.

Android's ``I18nCatalog.translate`` falls back to the key itself, so a typo or a
key that was only added to one locale renders a raw identifier on the screen.
This checker scans the Kotlin sources for literal user-facing keys
(``translate(locale, "...")``, ``labelKey = "..."`` and ``state.*`` literals) and
fails when one is missing from ``zh-CN`` or ``en``. Interpolated keys
(``"status.capability.$state"``) are runtime-composed and are intentionally not
checked here; their prefixes are covered by the unit tests that enumerate the
closed value sets.

It is a static consistency check: it does not render anything and does not
replace device testing.
"""

from __future__ import annotations

import argparse
import json
import re
import sys
from collections import defaultdict
from pathlib import Path
from typing import Iterable

PROJECT_ROOT = Path(__file__).resolve().parent.parent
DEFAULT_SOURCES = PROJECT_ROOT / "android" / "app" / "src" / "main" / "kotlin"
DEFAULT_CATALOG = PROJECT_ROOT / "crates" / "common" / "assets" / "i18n.json"

PATTERNS = (
    re.compile(r'translate\(\s*locale\s*,\s*"([^"$]+)"'),
    re.compile(r'labelKey\s*=\s*"([^"$]+)"'),
    re.compile(r'"(state\.[a-z_.]+)"'),
)

REPORT_SCHEMA = "fuckipv6-android-i18n-report"


def collect_keys(sources: Path) -> dict[str, set[str]]:
    """Map each literal key to the files that use it."""
    found: dict[str, set[str]] = defaultdict(set)
    for path in sorted(sources.rglob("*.kt")):
        text = path.read_text(encoding="utf-8")
        try:
            label = str(path.relative_to(PROJECT_ROOT))
        except ValueError:
            # Sources outside the checkout (tests use temp trees).
            label = str(path.relative_to(sources))
        for pattern in PATTERNS:
            for match in pattern.finditer(text):
                found[match.group(1)].add(label)
    return found


def check(sources: Path, catalog: Path) -> dict:
    messages = json.loads(catalog.read_text(encoding="utf-8"))["messages"]
    zh = set(messages.get("zh-CN", {}))
    en = set(messages.get("en", {}))
    if not zh and not en:
        return {
            "schema": REPORT_SCHEMA,
            "status": "failed",
            "errors": ["i18n catalog is missing the zh-CN or en locale"],
            "keys_checked": 0,
            "missing_zh": [],
            "missing_en": [],
            "unpaired_catalog_keys": [],
        }

    keys = collect_keys(sources)
    missing_zh = sorted(key for key in keys if key not in zh)
    missing_en = sorted(key for key in keys if key not in en)
    unpaired = sorted(key for key in zh.symmetric_difference(en))

    errors: list[str] = []
    for key in missing_zh:
        errors.append(f"{key} is missing from zh-CN (used by {', '.join(sorted(keys[key]))})")
    for key in missing_en:
        errors.append(f"{key} is missing from en (used by {', '.join(sorted(keys[key]))})")
    for key in unpaired:
        errors.append(f"{key} exists in only one locale of the catalog")

    return {
        "schema": REPORT_SCHEMA,
        "status": "failed" if errors else "passed",
        "keys_checked": len(keys),
        "missing_zh": missing_zh,
        "missing_en": missing_en,
        "unpaired_catalog_keys": unpaired,
        "errors": errors,
    }


def main(argv: Iterable[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--sources", type=Path, default=DEFAULT_SOURCES)
    parser.add_argument("--catalog", type=Path, default=DEFAULT_CATALOG)
    parser.add_argument("--report", type=Path, default=None)
    parser.add_argument("--json", action="store_true")
    args = parser.parse_args(list(argv) if argv is not None else None)

    if not args.sources.is_dir():
        print(f"android sources not found: {args.sources}", file=sys.stderr)
        return 2

    report = check(args.sources, args.catalog)
    if args.report:
        args.report.parent.mkdir(parents=True, exist_ok=True)
        args.report.write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    if args.json:
        print(json.dumps(report, ensure_ascii=False, indent=2))
    for error in report["errors"]:
        print(f"[FAIL] {error}", file=sys.stderr)
    print(
        f"check-android-i18n: {report['status']} ({report.get('keys_checked', 0)} literal keys, "
        f"{len(report['errors'])} issues)",
        file=sys.stderr,
    )
    return 0 if report["status"] == "passed" else 1


if __name__ == "__main__":
    raise SystemExit(main())
