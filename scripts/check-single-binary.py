#!/usr/bin/env python3
"""Enforce the unified-node release boundary.

The workspace may contain libraries, tests, examples, and benchmarks, but it may
only expose one user-facing executable: ``fuckipv6``.
"""

from __future__ import annotations

import argparse
import json
import subprocess
import sys
from pathlib import Path
from typing import Any


EXPECTED_BINARY = "fuckipv6"


def binary_targets(metadata: dict[str, Any]) -> list[tuple[str, str]]:
    """Return ``(package, binary)`` pairs for every Cargo binary target."""
    targets: list[tuple[str, str]] = []
    for package in metadata.get("packages", []):
        package_name = str(package.get("name", "<unknown>"))
        for target in package.get("targets", []):
            if "bin" in target.get("kind", []):
                targets.append((package_name, str(target.get("name", "<unknown>"))))
    return sorted(targets)


def validate(metadata: dict[str, Any]) -> list[str]:
    """Return human-readable violations of the single-node binary boundary."""
    binaries = binary_targets(metadata)
    names = [name for _, name in binaries]
    errors: list[str] = []

    if names.count(EXPECTED_BINARY) != 1:
        errors.append(
            f"expected exactly one {EXPECTED_BINARY!r} binary target, found {names.count(EXPECTED_BINARY)}"
        )

    unexpected = [f"{package}:{name}" for package, name in binaries if name != EXPECTED_BINARY]
    if unexpected:
        errors.append("unexpected user-facing binary targets: " + ", ".join(unexpected))

    return errors


def load_metadata(root: Path) -> dict[str, Any]:
    result = subprocess.run(
        ["cargo", "metadata", "--no-deps", "--format-version", "1"],
        cwd=root,
        check=True,
        capture_output=True,
        text=True,
    )
    return json.loads(result.stdout)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--root",
        type=Path,
        default=Path(__file__).resolve().parent.parent,
        help="Cargo workspace root",
    )
    args = parser.parse_args()

    try:
        errors = validate(load_metadata(args.root))
    except (OSError, subprocess.CalledProcessError, json.JSONDecodeError) as error:
        print(f"check-single-binary: failed to inspect Cargo targets: {error}", file=sys.stderr)
        return 1

    if errors:
        for error in errors:
            print(f"check-single-binary: {error}", file=sys.stderr)
        return 1

    print(f"check-single-binary: passed (only {EXPECTED_BINARY} is exposed)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
