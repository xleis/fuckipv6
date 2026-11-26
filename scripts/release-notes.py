#!/usr/bin/env python3
"""Assemble release notes from the template and the changelog.

The release gate requires notes that name capabilities, limits and migration steps.
Capabilities and limits come from the changelog entry for this version; migration is
the one section a human must fill, so the script refuses to emit notes that still
carry the template placeholder there.
"""

from __future__ import annotations

import argparse
import re
import subprocess
from pathlib import Path

PROJECT_ROOT = Path(__file__).resolve().parent.parent
TEMPLATE = PROJECT_ROOT / "docs" / "RELEASE_NOTES_TEMPLATE.md"
VERSIONED_NOTES = PROJECT_ROOT / "docs" / "release-notes"


def versioned_source(version: str) -> Path | None:
    """Per-release notes filled by a human, kept next to the template.

    A published release needs its capability, limitation and migration sections
    written for that version; the generic template can only ever hold
    placeholders for two of them.
    """
    candidate = VERSIONED_NOTES / f"{version}.md"
    return candidate if candidate.is_file() else None


def git(*arguments: str) -> str:
    return subprocess.run(
        ["git", *arguments], cwd=PROJECT_ROOT, capture_output=True, text=True, check=True
    ).stdout.strip()


def changelog_section(version: str) -> str:
    text = (PROJECT_ROOT / "CHANGELOG.md").read_text(encoding="utf-8")
    match = re.search(
        rf"^## {re.escape(version)}\b.*?$(.*?)(?=^## |\Z)",
        text,
        re.MULTILINE | re.DOTALL,
    )
    if not match:
        return ""
    return match.group(1).strip()


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--version", required=True)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--toolchain", default="rustc 1.100.0")
    parser.add_argument("--allow-placeholder-migration", action="store_true")
    args = parser.parse_args()

    source = versioned_source(args.version)
    template = (source or TEMPLATE).read_text(encoding="utf-8")
    notes = (
        template.replace("{{VERSION}}", args.version)
        .replace("{{COMMIT}}", git("rev-parse", "--short=12", "HEAD"))
        .replace("{{TOOLCHAIN}}", args.toolchain)
    )
    section = changelog_section(args.version)
    if section:
        notes = notes.replace(
            "<!-- 本版本实际可用、且在目标平台有证据支撑的能力。逐条给出证据位置。 -->",
            section,
        )
    if source is None and "<!--" in notes and not args.allow_placeholder_migration:
        print(
            "release-notes: the template still has an unfilled section; fill 限制 and 迁移 "
            "before publishing (or pass --allow-placeholder-migration for a draft)",
            flush=True,
        )
        return 1
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(notes, encoding="utf-8")
    print(f"release-notes: wrote {args.output}", flush=True)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
