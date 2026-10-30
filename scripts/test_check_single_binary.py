#!/usr/bin/env python3
"""Regression tests for scripts/check-single-binary.py."""

from __future__ import annotations

import importlib.util
import unittest
from pathlib import Path

SCRIPT = Path(__file__).with_name("check-single-binary.py")
SPEC = importlib.util.spec_from_file_location("check_single_binary", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
CHECK = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(CHECK)


def metadata(*targets: tuple[str, str, list[str]]) -> dict:
    packages = []
    for package, target, kinds in targets:
        packages.append(
            {"name": package, "targets": [{"name": target, "kind": kinds}]}
        )
    return {"packages": packages}


class CheckSingleBinaryTests(unittest.TestCase):
    def test_accepts_only_node_binary(self) -> None:
        data = metadata(
            ("fuckipv6-client", "fuckipv6", ["bin"]),
            ("fuckipv6-coordinator", "fuckipv6_coordinator", ["lib"]),
            ("fuckipv6-common", "protocol", ["bench"]),
        )

        self.assertEqual(CHECK.validate(data), [])

    def test_rejects_standalone_coordinator_binary(self) -> None:
        data = metadata(
            ("fuckipv6-client", "fuckipv6", ["bin"]),
            ("fuckipv6-coordinator", "fuckipv6-coordinator", ["bin"]),
        )

        errors = CHECK.validate(data)
        self.assertTrue(any("unexpected user-facing binary" in error for error in errors))

    def test_requires_node_binary(self) -> None:
        data = metadata(("fuckipv6-coordinator", "fuckipv6_coordinator", ["lib"]))

        errors = CHECK.validate(data)
        self.assertTrue(any("expected exactly one" in error for error in errors))


if __name__ == "__main__":
    unittest.main()
