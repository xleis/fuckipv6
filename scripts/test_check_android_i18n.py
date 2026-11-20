#!/usr/bin/env python3
"""Tests for check-android-i18n.py."""

from __future__ import annotations

import importlib.util
import json
import tempfile
import unittest
from pathlib import Path

SCRIPT = Path(__file__).with_name("check-android-i18n.py")
SPEC = importlib.util.spec_from_file_location("check_android_i18n", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
CHECK = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(CHECK)


def write_catalog(path: Path, zh: dict[str, str], en: dict[str, str]) -> None:
    path.write_text(
        json.dumps({"schema_version": 1, "default_locale": "en", "messages": {"zh-CN": zh, "en": en}}),
        encoding="utf-8",
    )


class AndroidI18nCheckTest(unittest.TestCase):
    def test_passes_when_every_literal_key_is_in_both_locales(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            sources = root / "kotlin"
            sources.mkdir()
            (sources / "Page.kt").write_text(
                'val a = contracts.i18n.translate(locale, "page.files")\n'
                'FileDetailRow(labelKey = "member.row.lifecycle")\n'
                'val b = SurfaceState(SurfaceStatus.ERROR)\n'
                'val c = "state.error.detail"\n',
                encoding="utf-8",
            )
            catalog = root / "i18n.json"
            keys = {"page.files": "文件", "member.row.lifecycle": "生命周期", "state.error.detail": "错误"}
            write_catalog(catalog, keys, keys)

            report = CHECK.check(sources, catalog)
            self.assertEqual(report["status"], "passed")
            self.assertEqual(report["keys_checked"], 3)

    def test_interpolated_keys_are_not_treated_as_literals(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            sources = root / "kotlin"
            sources.mkdir()
            (sources / "Page.kt").write_text(
                'val a = contracts.i18n.translate(locale, "status.capability.$state")\n'
                'val b = contracts.i18n.translate(locale, labelKey)\n',
                encoding="utf-8",
            )
            catalog = root / "i18n.json"
            write_catalog(catalog, {}, {})
            report = CHECK.check(sources, catalog)
            # No literal keys, but an empty catalog still has to fail loudly.
            self.assertEqual(report["status"], "failed")

    def test_missing_locale_entry_fails_with_the_using_file(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            sources = root / "kotlin"
            sources.mkdir()
            (sources / "FilePage.kt").write_text(
                'val a = contracts.i18n.translate(locale, "file.page.upload")\n',
                encoding="utf-8",
            )
            catalog = root / "i18n.json"
            write_catalog(catalog, {"file.page.upload": "上传"}, {})
            report = CHECK.check(sources, catalog)
            self.assertEqual(report["status"], "failed")
            self.assertIn("file.page.upload", report["missing_en"])
            self.assertTrue(any("FilePage.kt" in error for error in report["errors"]))

    def test_one_sided_catalog_keys_are_reported_even_without_usage(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            sources = root / "kotlin"
            sources.mkdir()
            (sources / "Page.kt").write_text("// no keys\n", encoding="utf-8")
            catalog = root / "i18n.json"
            write_catalog(catalog, {"only.zh": "中文"}, {"only.en": "english"})
            report = CHECK.check(sources, catalog)
            self.assertEqual(report["status"], "failed")
            self.assertEqual(report["unpaired_catalog_keys"], ["only.en", "only.zh"])

    def test_missing_sources_directory_is_an_operational_error(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            catalog = root / "i18n.json"
            write_catalog(catalog, {}, {})
            exit_code = CHECK.main(["--sources", str(root / "missing"), "--catalog", str(catalog)])
            self.assertEqual(exit_code, 2)


if __name__ == "__main__":
    unittest.main()
