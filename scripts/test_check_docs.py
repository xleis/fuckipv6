#!/usr/bin/env python3
"""Regression tests for scripts/check-docs.py."""

from __future__ import annotations

import contextlib
import importlib.util
import io
import json
import tempfile
import unittest
from pathlib import Path

SCRIPT = Path(__file__).with_name("check-docs.py")
SPEC = importlib.util.spec_from_file_location("check_docs", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
DOCS = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(DOCS)


class CheckDocsTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name)
        (self.root / "README.md").write_text(
            "# Readme\n\n"
            "Secure Session v2 classic 已接入生产 UDP/TUN 数据面；尚未完成独立密码学审计，后量子组合尚未接入。\n\n"
            "当前结果不是超越 WireGuard 的证明。\n",
            encoding="utf-8",
        )
        (self.root / "SECURITY_PERFORMANCE_TODO.md").write_text(
            "Secure Session v2 已接入生产 `P2pEngine`，仍需独立密码学审计。\n",
            encoding="utf-8",
        )
        (self.root / "CHANGELOG.md").write_text(
            "不得据此宣称已具备后量子安全或超越 WireGuard。\n", encoding="utf-8"
        )
        (self.root / "EXECUTION_QUEUE.md").write_text(
            "- [x] **EQ-04 test**\n"
            "- [x] **EQ-05 test**\n"
            "- [x] **EQ-06 test**\n"
            "- [x] **EQ-09 test**\n"
            "- [ ] **EQ-13 test**\n",
            encoding="utf-8",
        )

    def tearDown(self) -> None:
        self.temporary.cleanup()

    def files(self) -> list[Path]:
        return sorted(self.root.glob("*.md"))

    def test_valid_links_and_existing_anchors_pass(self) -> None:
        (self.root / "GUIDE.md").write_text(
            "# 系统要求\n\n参见 [README](README.md#readme) 和 [本节](#系统要求)。\n",
            encoding="utf-8",
        )

        result = DOCS.check_documents(self.root, self.files())

        self.assertEqual(result["summary"]["status"], "passed")
        self.assertGreaterEqual(result["links_checked"], 2)

    def test_broken_file_and_anchor_links_fail(self) -> None:
        (self.root / "GUIDE.md").write_text(
            "[missing](missing.md)\n[bad anchor](README.md#missing-heading)\n",
            encoding="utf-8",
        )

        result = DOCS.check_documents(self.root, self.files())
        codes = {issue["code"] for issue in result["issues"]}

        self.assertEqual(result["summary"]["status"], "failed")
        self.assertIn("broken-link", codes)
        self.assertIn("broken-anchor", codes)

    def test_code_fences_external_urls_and_mailto_are_not_file_links(self) -> None:
        (self.root / "GUIDE.md").write_text(
            "```markdown\n[ignored](missing.md)\n```\n"
            "`[also ignored](missing-too.md)`\n"
            "[web](https://example.com/missing) [mail](mailto:test@example.com)\n",
            encoding="utf-8",
        )

        result = DOCS.check_documents(self.root, self.files())

        self.assertEqual(result["summary"]["status"], "passed")
        self.assertEqual(result["links_checked"], 2)

    def test_file_urls_and_paths_outside_root_are_rejected(self) -> None:
        (self.root / "GUIDE.md").write_text(
            "[local](file:///home/user/doc.md)\n[outside](../../outside.md)\n",
            encoding="utf-8",
        )

        result = DOCS.check_documents(self.root, self.files())
        codes = {issue["code"] for issue in result["issues"]}

        self.assertIn("nonportable-file-url", codes)
        self.assertIn("path-escapes-root", codes)

    def test_reference_definition_is_checked(self) -> None:
        (self.root / "GUIDE.md").write_text(
            "[guide][ref]\n\n[ref]: missing.md\n", encoding="utf-8"
        )

        result = DOCS.check_documents(self.root, self.files())

        self.assertEqual(result["issues"][0]["code"], "broken-link")
        self.assertEqual(result["issues"][0]["line"], 3)

    def test_missing_security_boundary_fails(self) -> None:
        (self.root / "README.md").write_text("# Readme\n", encoding="utf-8")

        result = DOCS.check_documents(self.root, self.files())
        codes = {issue["code"] for issue in result["issues"]}

        self.assertIn("readme-v2-production-boundary", codes)
        self.assertIn("readme-no-wireguard-performance-claim", codes)

    def test_completed_item_requires_completed_prerequisites(self) -> None:
        (self.root / "EXECUTION_QUEUE.md").write_text(
            "- [x] **EQ-04 test**\n"
            "- [ ] **EQ-05 test**\n"
            "- [x] **EQ-06 test**\n"
            "- [x] **EQ-09 test**\n"
            "- [x] **EQ-13 test**\n",
            encoding="utf-8",
        )

        result = DOCS.check_documents(self.root, self.files())

        self.assertIn(
            "premature-completion", {issue["code"] for issue in result["issues"]}
        )

    def test_cli_writes_machine_readable_report(self) -> None:
        output = self.root / "docs-report.json"
        with contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(
            io.StringIO()
        ):
            exit_code = DOCS.main(
                ["--root", str(self.root), "--report", str(output)]
            )
        document = json.loads(output.read_text(encoding="utf-8"))

        self.assertEqual(exit_code, 0)
        self.assertEqual(document["schema"], DOCS.REPORT_SCHEMA)
        self.assertEqual(document["summary"]["status"], "passed")


if __name__ == "__main__":
    unittest.main()
