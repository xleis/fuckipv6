#!/usr/bin/env python3
"""Regression tests for scripts/check-ignored-tests.py."""

from __future__ import annotations

import importlib.util
import json
import tempfile
import unittest
from pathlib import Path

SCRIPT = Path(__file__).with_name("check-ignored-tests.py")
SPEC = importlib.util.spec_from_file_location("check_ignored_tests", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
CHECK = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(CHECK)


class IgnoredTestsCheckTest(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name)
        self.sources = self.root / "crates"
        self.sources.mkdir()

    def tearDown(self) -> None:
        self.temporary.cleanup()

    def write(self, name: str, content: str) -> None:
        path = self.sources / f"{name}.rs"
        path.write_text(content, encoding="utf-8")

    def test_a_reasoned_ignore_with_an_existing_runner_passes(self) -> None:
        runner = self.root / "scripts" / "runner.sh"
        runner.parent.mkdir()
        runner.write_text("#!/bin/sh\n", encoding="utf-8")
        CHECK.PROJECT_ROOT = self.root
        self.write(
            "a",
            '#[ignore = "requires root and scripts/runner.sh"]\n'
            "fn skipped() {}\n",
        )
        report = CHECK.check(self.sources)
        self.assertEqual(report["status"], "passed")
        self.assertEqual(report["ignored_tests"], 1)
        self.assertEqual(report["runners"], ["scripts/runner.sh"])

    def test_a_bare_ignore_fails(self) -> None:
        CHECK.PROJECT_ROOT = self.root
        self.write("b", "#[ignore]\nfn skipped() {}\n")
        report = CHECK.check(self.sources)
        self.assertEqual(report["status"], "failed")
        self.assertTrue(any("bare #[ignore]" in error for error in report["errors"]))

    def test_a_placeholder_reason_fails(self) -> None:
        CHECK.PROJECT_ROOT = self.root
        self.write("c", '#[ignore = "todo"]\nfn skipped() {}\n')
        report = CHECK.check(self.sources)
        self.assertEqual(report["status"], "failed")
        self.assertTrue(any("too short" in error for error in report["errors"]))

    def test_a_reason_naming_a_deleted_runner_fails(self) -> None:
        CHECK.PROJECT_ROOT = self.root
        self.write(
            "d",
            '#[ignore = "requires root and scripts/deleted-runner.sh"]\nfn skipped() {}\n',
        )
        report = CHECK.check(self.sources)
        self.assertEqual(report["status"], "failed")
        self.assertTrue(any("missing runner" in error for error in report["errors"]))

    def test_unescaped_quotes_are_reported_not_swallowed(self) -> None:
        CHECK.PROJECT_ROOT = self.root
        self.write("e", '#[ignore = "requires root"]\nfn skipped() {}\n')
        report = CHECK.check(self.sources)
        self.assertEqual(report["status"], "failed")

    def test_plain_tests_are_not_counted(self) -> None:
        CHECK.PROJECT_ROOT = self.root
        self.write("f", "#[test]\nfn runs() {}\n")
        report = CHECK.check(self.sources)
        self.assertEqual(report["status"], "passed")
        self.assertEqual(report["ignored_tests"], 0)

    def test_missing_sources_directory_is_operational_error(self) -> None:
        exit_code = CHECK.main(["--sources", str(self.root / "missing")])
        self.assertEqual(exit_code, 2)


if __name__ == "__main__":
    unittest.main()
