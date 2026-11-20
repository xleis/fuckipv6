#!/usr/bin/env python3
"""Keep the surface-state vocabulary identical across the three UIs.

The shared catalog defines ``state.<name>.title`` / ``.detail`` for every
surface state. Android declares its states in ``SurfaceStatus``, WebUI declares
the same vocabulary in ``state-matrix.ts``. A state that exists on one surface
and not the catalog renders a raw key; a state that exists on one surface and
not the other means the two products disagree about what a screen can be.

The check is static parsing, not rendering: it catches vocabulary drift and
cannot prove the surfaces look or behave the same.
"""

from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path
from typing import Iterable

PROJECT_ROOT = Path(__file__).resolve().parent.parent
DEFAULT_ANDROID = (
    PROJECT_ROOT / "android/app/src/main/kotlin/cc/xlei/fuckipv6/android/ui/SurfaceState.kt"
)
DEFAULT_WEBUI = PROJECT_ROOT / "webui/src/state-matrix.ts"
DEFAULT_CATALOG = PROJECT_ROOT / "crates/common/assets/i18n.json"

ANDROID_ENTRY = re.compile(r'^\s*[A-Z][A-Z0-9_]*\("([a-z][a-z0-9_]*)"\)', re.MULTILINE)
WEBUI_ENTRY = re.compile(r'^  ([a-z][a-z0-9_]*): \{', re.MULTILINE)
CATALOG_STATE = re.compile(r"^state\.([a-z][a-z0-9_]*)\.title$")

# `state-matrix.ts` also carries non-state blocks (for example the long-press
# options menu). They are configuration, not a screen state.
WEBUI_NON_STATE_BLOCKS = {"options"}

REPORT_SCHEMA = "fuckipv6-surface-state-parity"


def parse_android_states(path: Path) -> set[str]:
    return set(ANDROID_ENTRY.findall(path.read_text(encoding="utf-8")))


def parse_webui_states(path: Path) -> set[str]:
    found = set(WEBUI_ENTRY.findall(path.read_text(encoding="utf-8")))
    return {name for name in found if name not in WEBUI_NON_STATE_BLOCKS}


def parse_catalog_states(path: Path) -> set[str]:
    messages = json.loads(path.read_text(encoding="utf-8"))["messages"]
    states: set[str] | None = None
    for locale, entries in messages.items():
        keys = {match.group(1) for key in entries if (match := CATALOG_STATE.match(key))}
        if states is None:
            states = keys
        elif states != keys:
            raise ValueError(f"catalog locales disagree on the state vocabulary: {locale}")
    return states or set()


def check(android: Path, webui: Path, catalog: Path) -> dict:
    errors: list[str] = []
    android_states = parse_android_states(android)
    webui_states = parse_webui_states(webui)
    catalog_states = parse_catalog_states(catalog)

    if not android_states:
        errors.append("no Android surface states parsed; the extractor is out of date")
    if not webui_states:
        errors.append("no WebUI surface states parsed; the extractor is out of date")
    if not catalog_states:
        errors.append("the i18n catalog has no state.<name>.title keys")

    for name in sorted(catalog_states - android_states):
        errors.append(f"state {name} is in the catalog but missing from Android SurfaceStatus")
    for name in sorted(android_states - catalog_states):
        errors.append(f"Android state {name} has no state.{name}.title in the catalog")
    for name in sorted(webui_states - catalog_states):
        errors.append(f"WebUI state {name} has no state.{name}.title in the catalog")
    for name in sorted(webui_states - android_states):
        errors.append(
            f"WebUI state {name} is missing from Android SurfaceStatus "
            "(add it there and to the catalog, or stop using it)"
        )

    return {
        "schema": REPORT_SCHEMA,
        "status": "failed" if errors else "passed",
        "android_states": sorted(android_states),
        "webui_states": sorted(webui_states),
        "catalog_states": sorted(catalog_states),
        "android_only": sorted(android_states - webui_states),
        "errors": errors,
    }


def main(argv: Iterable[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--android", type=Path, default=DEFAULT_ANDROID)
    parser.add_argument("--webui", type=Path, default=DEFAULT_WEBUI)
    parser.add_argument("--catalog", type=Path, default=DEFAULT_CATALOG)
    parser.add_argument("--report", type=Path, default=None)
    parser.add_argument("--json", action="store_true")
    args = parser.parse_args(list(argv) if argv is not None else None)

    for path in (args.android, args.webui, args.catalog):
        if not path.is_file():
            print(f"input not found: {path}", file=sys.stderr)
            return 2

    try:
        report = check(args.android, args.webui, args.catalog)
    except ValueError as error:
        print(f"[FAIL] {error}", file=sys.stderr)
        return 1

    if args.report:
        args.report.parent.mkdir(parents=True, exist_ok=True)
        args.report.write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    if args.json:
        print(json.dumps(report, ensure_ascii=False, indent=2))
    for error in report["errors"]:
        print(f"[FAIL] {error}", file=sys.stderr)
    print(
        f"check-surface-state-parity: {report['status']} "
        f"(android={len(report['android_states'])}, webui={len(report['webui_states'])}, "
        f"catalog={len(report['catalog_states'])}, {len(report['errors'])} issues)",
        file=sys.stderr,
    )
    return 0 if report["status"] == "passed" else 1


if __name__ == "__main__":
    raise SystemExit(main())
