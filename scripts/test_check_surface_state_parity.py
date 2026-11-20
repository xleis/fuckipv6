#!/usr/bin/env python3
"""Regression tests for scripts/check-surface-state-parity.py."""

from __future__ import annotations

import importlib.util
import json
import tempfile
import unittest
from pathlib import Path

SCRIPT = Path(__file__).with_name("check-surface-state-parity.py")
SPEC = importlib.util.spec_from_file_location("check_surface_state_parity", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
PARITY = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(PARITY)

ANDROID = '''package cc.xlei.fuckipv6.android.ui

enum class SurfaceStatus(val wireName: String) {
    ERROR("error"),
    READY("ready"),
    NOT_AVAILABLE_ON_DEVICE("not_available_on_device"),
}
'''

WEBUI = '''export const STATE_MATRIX = {
  error: {
    kind: "error",
  },
  ready: {
    kind: "ready",
  },
  options: {
    items: [],
  },
} as const;
'''


def catalog(states: dict[str, list[str]]) -> dict:
    messages = {}
    for locale, names in states.items():
        messages[locale] = {
            f"state.{name}.title": name for name in names
        } | {f"state.{name}.detail": name for name in names}
    return {"schema_version": 1, "default_locale": "en", "messages": messages}


class SurfaceStateParityTest(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name)
        self.android = self.root / "SurfaceState.kt"
        self.webui = self.root / "state-matrix.ts"
        self.catalog = self.root / "i18n.json"
        self.android.write_text(ANDROID, encoding="utf-8")
        self.webui.write_text(WEBUI, encoding="utf-8")

    def tearDown(self) -> None:
        self.temporary.cleanup()

    def write_catalog(self, states: list[str], *, en_states: list[str] | None = None) -> None:
        self.catalog.write_text(
            json.dumps(
                catalog(
                    {
                        "zh-CN": states,
                        "en": states if en_states is None else en_states,
                    }
                )
            ),
            encoding="utf-8",
        )

    def test_passes_when_both_surfaces_match_the_catalog(self) -> None:
        self.write_catalog(["error", "ready", "not_available_on_device"])
        report = PARITY.check(self.android, self.webui, self.catalog)
        self.assertEqual(report["status"], "passed")
        self.assertEqual(report["android_only"], ["not_available_on_device"])

    def test_non_state_blocks_in_the_matrix_are_ignored(self) -> None:
        self.write_catalog(["error", "ready", "not_available_on_device"])
        report = PARITY.check(self.android, self.webui, self.catalog)
        self.assertNotIn("options", report["webui_states"])

    def test_webui_state_missing_from_android_fails(self) -> None:
        self.webui.write_text(WEBUI.replace('  ready: {', '  stale: {'), encoding="utf-8")
        self.write_catalog(["error", "stale", "not_available_on_device"])
        report = PARITY.check(self.android, self.webui, self.catalog)
        self.assertEqual(report["status"], "failed")
        self.assertTrue(any("stale" in error and "Android" in error for error in report["errors"]))

    def test_android_state_missing_from_catalog_fails(self) -> None:
        self.write_catalog(["error", "ready"])
        report = PARITY.check(self.android, self.webui, self.catalog)
        self.assertEqual(report["status"], "failed")
        self.assertTrue(any("not_available_on_device" in error for error in report["errors"]))

    def test_locale_disagreement_is_reported(self) -> None:
        self.write_catalog(["error", "ready", "not_available_on_device"], en_states=["error", "ready"])
        with self.assertRaises(ValueError):
            PARITY.check(self.android, self.webui, self.catalog)

    def test_empty_parse_is_not_a_vacuous_pass(self) -> None:
        self.android.write_text("// no enum here\n", encoding="utf-8")
        self.write_catalog(["error", "ready", "not_available_on_device"])
        report = PARITY.check(self.android, self.webui, self.catalog)
        self.assertEqual(report["status"], "failed")
        self.assertTrue(any("extractor is out of date" in error for error in report["errors"]))

    def test_missing_input_is_an_operational_error(self) -> None:
        self.write_catalog(["error"])
        exit_code = PARITY.main(
            ["--android", str(self.root / "missing.kt"), "--webui", str(self.webui)]
        )
        self.assertEqual(exit_code, 2)


if __name__ == "__main__":
    unittest.main()
