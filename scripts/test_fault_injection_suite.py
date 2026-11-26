#!/usr/bin/env python3
"""Regression tests for scripts/fault-injection-suite.py."""

from __future__ import annotations

import importlib.util
import unittest
from pathlib import Path
from tempfile import TemporaryDirectory
from unittest import mock

SCRIPT = Path(__file__).with_name("fault-injection-suite.py")
SPEC = importlib.util.spec_from_file_location("fault_injection_suite", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
SUITE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(SUITE)


class FaultInjectionSuiteTest(unittest.TestCase):
    def test_exit_codes_map_to_the_four_reported_states(self) -> None:
        self.assertEqual(SUITE.classify(0, False), "passed")
        self.assertEqual(SUITE.classify(77, False), "skipped")
        self.assertEqual(SUITE.classify(1, False), "failed")
        self.assertEqual(SUITE.classify(None, True), "timeout")
        self.assertEqual(SUITE.classify(0, True), "timeout")

    def test_root_only_suites_are_skipped_without_root(self) -> None:
        suite = SUITE.Suite("x", "生命周期", "d", script="scripts/x.sh", root_required=True)
        self.assertIsNone(suite.command(False))
        self.assertEqual(suite.command(True), ["bash", "scripts/x.sh"])

    def test_rootless_suites_run_through_the_wrapper(self) -> None:
        suite = SUITE.Suite("y", "网络", "d", rootless_script="scripts/y.sh")
        self.assertEqual(
            suite.command(False),
            ["bash", "scripts/w12-rootless-netns.sh", "bash", "scripts/y.sh"],
        )

    def test_summary_blocks_on_failure_and_timeout_but_not_skip(self) -> None:
        results = [
            {"name": "a", "status": "passed", "tail": ""},
            {"name": "b", "status": "skipped", "reason": "root", "tail": ""},
            {"name": "c", "status": "failed", "tail": "boom"},
            {"name": "d", "status": "timeout", "tail": ""},
        ]
        status, problems = SUITE.summarize(results, require_full=False)
        self.assertEqual(status, "failed")
        self.assertTrue(any("c failed" in problem for problem in problems))
        self.assertTrue(any("d timed out" in problem for problem in problems))
        self.assertFalse(any(problem.startswith("b ") for problem in problems))

    def test_require_full_turns_skips_into_blockers(self) -> None:
        results = [{"name": "b", "status": "skipped", "reason": "root", "tail": ""}]
        status, problems = SUITE.summarize(results, require_full=True)
        self.assertEqual(status, "failed")
        self.assertTrue(any("skipped" in problem for problem in problems))

    def test_markdown_report_names_every_suite_and_the_counts(self) -> None:
        report = {
            "generated_at": "2026-01-01T00:00:00Z",
            "status": "passed",
            "is_root": False,
            "require_full": False,
            "counts": {"passed": 1, "skipped": 1, "failed": 0, "timeout": 0},
            "results": [
                {"name": "a", "domain": "网络", "status": "passed", "duration_seconds": 1.0,
                 "description": "ok", "reason": "", "tail": ""},
                {"name": "b", "domain": "路由", "status": "skipped", "duration_seconds": 0.0,
                 "description": "later", "reason": "root is required", "tail": ""},
            ],
            "problems": [],
        }
        markdown = SUITE.render_markdown(report)
        self.assertIn("`a`", markdown)
        self.assertIn("root is required", markdown)
        self.assertIn("passed=1", markdown)

    def test_every_declared_suite_points_at_a_real_script_or_cargo_test(self) -> None:
        for suite in SUITE.SUITES:
            if suite.cargo_test:
                package, target, test_name = suite.cargo_test
                self.assertTrue(package and target and test_name, suite.name)
                continue
            script = suite.script or suite.rootless_script
            self.assertIsNotNone(script, suite.name)
            path = SUITE.PROJECT_ROOT / str(script)
            self.assertTrue(path.is_file(), f"{suite.name}: {script} is missing")

    def test_release_build_check_looks_for_binary_not_deps_dir(self) -> None:
        suite = SUITE.Suite(
            "w15-file",
            "File",
            "d",
            rootless_script="scripts/test-w15-file-netns.sh",
            requires_release_build=True,
        )
        with TemporaryDirectory() as tmp:
            root = Path(tmp)
            release_dir = root / "target" / "release"
            release_dir.mkdir(parents=True)
            with mock.patch.object(SUITE, "PROJECT_ROOT", root):
                skipped = SUITE.run_suite(suite, is_root=False, timeout_override=1)
                self.assertEqual(skipped["status"], "skipped")
                self.assertIn("release build", skipped["reason"])

                # A .d dep-info file alone must not count as a built binary.
                (release_dir / "fuckipv6.d").write_text("", encoding="utf-8")
                still_skipped = SUITE.run_suite(suite, is_root=False, timeout_override=1)
                self.assertEqual(still_skipped["status"], "skipped")

                # A binary without a legacy `deps/` directory (Cargo 1.100 layout)
                # must proceed to execute the suite.
                (release_dir / "fuckipv6").write_text("", encoding="utf-8")
                completed = mock.Mock(returncode=0, stdout="ok\n", stderr="")
                with mock.patch.object(SUITE.subprocess, "run", return_value=completed) as run_mock:
                    passed = SUITE.run_suite(suite, is_root=False, timeout_override=1)
                self.assertEqual(passed["status"], "passed")
                run_mock.assert_called_once()

    def test_root_detection_uses_os_geteuid(self) -> None:
        with mock.patch.object(SUITE.os, "geteuid", return_value=0):
            self.assertTrue(SUITE.is_running_as_root())
        with mock.patch.object(SUITE.os, "geteuid", return_value=1000):
            self.assertFalse(SUITE.is_running_as_root())


if __name__ == "__main__":
    unittest.main()
