#!/usr/bin/env python3
"""Regression tests for scripts/check-platform-matrix.py."""

from __future__ import annotations

import importlib.util
import tempfile
import unittest
from pathlib import Path

SCRIPT = Path(__file__).with_name("check-platform-matrix.py")
SPEC = importlib.util.spec_from_file_location("check_platform_matrix", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
CHECK = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(CHECK)

HEADER = "| 平台 / 架构 | 分类 | 证据或排除理由 |\n|---|---|---|\n"


class PlatformMatrixTest(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name)
        self.matrix = self.root / "matrix.md"
        self.workflow = self.root / "release.yml"
        self.doc = self.root / "doc.md"
        self.doc.write_text("# doc\n", encoding="utf-8")
        CHECK.PROJECT_ROOT = self.root

    def tearDown(self) -> None:
        self.temporary.cleanup()

    def write(self, rows: str, targets: list[str]) -> None:
        self.matrix.write_text(HEADER + rows, encoding="utf-8")
        body = "\n".join(f"          --target {target} \\" for target in targets)
        self.workflow.write_text(f"run: |\n{body}\n", encoding="utf-8")

    def test_shipped_target_with_resolving_evidence_passes(self) -> None:
        self.write(
            "| `x86_64-unknown-linux-gnu` | shipped | doc:doc.md#证据 |\n",
            ["x86_64-unknown-linux-gnu"],
        )
        report = CHECK.check(self.matrix, self.workflow)
        self.assertEqual(report["status"], "passed", report["errors"])

    def test_a_release_target_without_a_row_fails(self) -> None:
        self.write(
            "| `x86_64-unknown-linux-gnu` | shipped | doc:doc.md |\n",
            ["x86_64-unknown-linux-gnu", "aarch64-apple-darwin"],
        )
        report = CHECK.check(self.matrix, self.workflow)
        self.assertEqual(report["status"], "failed")
        self.assertTrue(any("aarch64-apple-darwin" in error for error in report["errors"]))

    def test_a_shipped_target_marked_unsupported_fails(self) -> None:
        self.write(
            "| `x86_64-pc-windows-gnu` | unsupported | unsupported:no packaging path at all |\n",
            ["x86_64-pc-windows-gnu"],
        )
        report = CHECK.check(self.matrix, self.workflow)
        self.assertEqual(report["status"], "failed")
        self.assertTrue(any("classified 'unsupported'" in error for error in report["errors"]))

    def test_missing_evidence_file_fails(self) -> None:
        self.write(
            "| `x86_64-unknown-linux-gnu` | shipped | path:missing/file.txt |\n",
            ["x86_64-unknown-linux-gnu"],
        )
        report = CHECK.check(self.matrix, self.workflow)
        self.assertEqual(report["status"], "failed")
        self.assertTrue(any("does not resolve" in error for error in report["errors"]))

    def test_vague_exclusion_reason_fails(self) -> None:
        self.write(
            "| `aarch64-unknown-linux-gnu` | unsupported | unsupported:no |\n",
            [],
        )
        report = CHECK.check(self.matrix, self.workflow)
        self.assertEqual(report["status"], "failed")
        self.assertTrue(any("does not resolve" in error for error in report["errors"]))

    def test_empty_evidence_cell_fails(self) -> None:
        self.write("| `aarch64-unknown-linux-gnu` | unsupported |  |\n", [])
        report = CHECK.check(self.matrix, self.workflow)
        self.assertEqual(report["status"], "failed")
        self.assertTrue(any("empty evidence" in error for error in report["errors"]))

    def test_unknown_category_fails(self) -> None:
        self.write(
            "| `aarch64-unknown-linux-gnu` | maybe | unsupported:no packaged artifact for this arch |\n",
            [],
        )
        report = CHECK.check(self.matrix, self.workflow)
        self.assertEqual(report["status"], "failed")
        self.assertTrue(any("unknown category" in error for error in report["errors"]))

    def test_missing_inputs_are_operational_errors(self) -> None:
        exit_code = CHECK.main(
            ["--matrix", str(self.root / "missing.md"), "--workflow", str(self.workflow)]
        )
        self.assertEqual(exit_code, 2)


if __name__ == "__main__":
    unittest.main()
