#!/usr/bin/env python3
"""Regression tests for scripts/release-notes.py."""

from __future__ import annotations

import importlib.util
import io
import subprocess
import sys
import tempfile
import unittest
from contextlib import redirect_stdout
from pathlib import Path

MODULE_ROOT = Path(__file__).resolve().parent.parent
SCRIPT = Path(__file__).with_name("release-notes.py")
SPEC = importlib.util.spec_from_file_location("release_notes", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
NOTES = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(NOTES)


class ReleaseNotesTest(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name)
        (self.root / "docs" / "release-notes").mkdir(parents=True)
        (self.root / "docs" / "RELEASE_NOTES_TEMPLATE.md").write_text(
            "# fuckipv6 {{VERSION}}\n\n构建自提交 `{{COMMIT}}`，工具链 `{{TOOLCHAIN}}`。\n\n"
            "## 能力\n\n<!-- 能力 -->\n\n## 限制\n\n<!-- 限制 -->\n\n## 迁移\n\n<!-- 迁移 -->\n",
            encoding="utf-8",
        )
        (self.root / "CHANGELOG.md").write_text(
            "# Changelog\n\n## 9.9.9 - 2026-01-01\n\n### 能力\n- 条目\n",
            encoding="utf-8",
        )
        self.git("init")
        self.git("config", "user.email", "test@example.com")
        self.git("config", "user.name", "Test")
        self.git("add", ".")
        self.git("commit", "-m", "fixture")

    def tearDown(self) -> None:
        self.temporary.cleanup()

    def git(self, *arguments: str) -> None:
        subprocess.run(["git", *arguments], cwd=self.root, check=True, capture_output=True)

    def run_script(self, *arguments: str) -> tuple[int, str]:
        """Runs the module against the fixture root instead of the real checkout."""
        NOTES.PROJECT_ROOT = self.root
        NOTES.TEMPLATE = self.root / "docs" / "RELEASE_NOTES_TEMPLATE.md"
        NOTES.VERSIONED_NOTES = self.root / "docs" / "release-notes"
        previous = sys.argv
        sys.argv = ["release-notes.py", *arguments]
        buffer = io.StringIO()
        try:
            with redirect_stdout(buffer):
                exit_code = NOTES.main()
        finally:
            sys.argv = previous
        return exit_code, buffer.getvalue()

    def test_the_generic_template_still_needs_human_sections(self) -> None:
        output = self.root / "notes.md"
        exit_code, stdout = self.run_script("--version", "9.9.9", "--output", str(output))
        self.assertEqual(exit_code, 1)
        self.assertIn("unfilled section", stdout)
        self.assertFalse(output.exists())

    def test_a_draft_can_be_emitted_explicitly(self) -> None:
        output = self.root / "notes.md"
        exit_code, _ = self.run_script(
            "--version", "9.9.9", "--output", str(output), "--allow-placeholder-migration"
        )
        self.assertEqual(exit_code, 0)
        self.assertTrue(output.exists())

    def test_a_versioned_file_replaces_the_template(self) -> None:
        (self.root / "docs" / "release-notes" / "9.9.9.md").write_text(
            "# fuckipv6 {{VERSION}}\n\n构建自提交 `{{COMMIT}}`，工具链 `{{TOOLCHAIN}}`。\n\n"
            "## 能力\n\n- 真实能力\n\n## 限制\n\n- 真实限制\n\n## 迁移\n\n无。\n",
            encoding="utf-8",
        )
        output = self.root / "notes.md"
        exit_code, stdout = self.run_script("--version", "9.9.9", "--output", str(output))
        self.assertEqual(exit_code, 0, stdout)
        text = output.read_text(encoding="utf-8")
        self.assertIn("真实限制", text)
        self.assertIn("无。", text)
        self.assertIn("9.9.9", text)

    def test_the_shipped_notes_for_the_current_version_are_complete(self) -> None:
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        output = Path(temporary.name) / "notes.md"
        result = subprocess.run(
            [
                sys.executable,
                str(MODULE_ROOT / "scripts" / "release-notes.py"),
                "--version",
                "0.4.0",
                "--output",
                str(output),
            ],
            cwd=MODULE_ROOT,
            capture_output=True,
            text=True,
        )
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        text = output.read_text(encoding="utf-8")
        for section in ("## 能力", "## 限制", "## 迁移"):
            self.assertIn(section, text)


if __name__ == "__main__":
    unittest.main()
