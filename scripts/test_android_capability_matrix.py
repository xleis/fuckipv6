#!/usr/bin/env python3
"""Regression tests for scripts/android-capability-matrix.py."""

from __future__ import annotations

import importlib.util
import json
import tempfile
import unittest
from pathlib import Path

SCRIPT = Path(__file__).with_name("android-capability-matrix.py")
SPEC = importlib.util.spec_from_file_location("android_capability_matrix", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
MATRIX = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MATRIX)

BRIDGE_DIR = "cc/xlei/fuckipv6/android/bridge"


def manifest(actions: list[dict]) -> dict:
    return {"schema_version": 1, "pages": [{"id": "networks", "actions": actions}]}


def action(action_id: str, command: str, status: str = "implemented") -> dict:
    return {
        "id": action_id,
        "command": command,
        "permission": "view_state",
        "audit": {"policy": "none"},
        "backend_status": status,
        "availability": "always",
    }


class AndroidCapabilityMatrixTest(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name)
        self.manifest = self.root / "node-manifest.json"
        self.android = self.root / "kotlin"
        self.bridge = self.android / BRIDGE_DIR
        self.bridge.mkdir(parents=True)
        self.output = self.root / "matrix.md"
        MATRIX.PROJECT_ROOT = self.root
        MATRIX._TEXT_CACHE.clear()

    def tearDown(self) -> None:
        MATRIX._TEXT_CACHE.clear()
        self.temporary.cleanup()

    def write(self, actions: list[dict], bridge_source: str) -> None:
        self.manifest.write_text(json.dumps(manifest(actions)), encoding="utf-8")
        # Every fixture carries the manifest-external capabilities so a test that
        # is not about them does not fail on their account.
        extras = "\n".join(token for _, _, token in MATRIX.EXTRA_CAPABILITIES)
        (self.bridge / "Surface.kt").write_text(bridge_source + "\n" + extras + "\n", encoding="utf-8")
        MATRIX._TEXT_CACHE.clear()

    def test_a_command_sent_from_a_bridge_is_mounted(self) -> None:
        self.write(
            [action("create_invitation", "create_invitation")],
            'JSONObject().put("command", "create_invitation")\n',
        )
        report = MATRIX.check(self.manifest, self.android)
        self.assertEqual(report["status"], "passed", report["errors"])
        self.assertEqual(report["rows"][0]["android"], "mounted")
        self.assertIn(BRIDGE_DIR, report["rows"][0]["android_evidence"])

    def test_a_command_only_mentioned_in_ui_or_comment_is_not_mounted(self) -> None:
        self.write(
            [action("create_invitation", "create_invitation")],
            '// create_invitation appears only in this comment\nval unrelated = "create_invitation"\n',
        )
        report = MATRIX.check(self.manifest, self.android)
        self.assertEqual(report["rows"][0]["android"], "desktop-only")

    def test_a_command_android_sends_but_the_manifest_lacks_fails(self) -> None:
        self.write(
            [action("create_invitation", "create_invitation")],
            'JSONObject().put("command", "ghost_command")\n',
        )
        report = MATRIX.check(self.manifest, self.android)
        self.assertEqual(report["status"], "failed")
        self.assertTrue(any("ghost_command" in error for error in report["errors"]))

    def test_unavailable_actions_stay_visible_as_blocked(self) -> None:
        self.write(
            [action("get_traffic_policy", "get_traffic_policy", status="unavailable")],
            "",
        )
        report = MATRIX.check(self.manifest, self.android)
        self.assertEqual(report["rows"][0]["desktop"], "blocked:unavailable")

    def test_extra_capabilities_must_be_present_in_android_sources(self) -> None:
        self.write([action("create_invitation", "create_invitation")], '"create_invitation"\n')
        # Remove one capability to prove the check is not vacuous.
        surface = self.bridge / "Surface.kt"
        surface.write_text(
            surface.read_text(encoding="utf-8").replace("ban_client", ""),
            encoding="utf-8",
        )
        MATRIX._TEXT_CACHE.clear()
        report = MATRIX.check(self.manifest, self.android)
        self.assertEqual(report["status"], "failed")
        self.assertTrue(
            any("extra capability" in error for error in report["errors"]),
            report["errors"],
        )

    def test_stale_output_is_rejected_then_accepted_after_regeneration(self) -> None:
        self.write([action("create_invitation", "create_invitation")], 'put("command", "create_invitation")\n')
        arguments = ["--manifest", str(self.manifest), "--android", str(self.android), "--output", str(self.output)]
        self.assertEqual(MATRIX.main([*arguments, "--check"]), 1)
        self.assertEqual(MATRIX.main(arguments), 0)
        self.assertEqual(MATRIX.main([*arguments, "--check"]), 0)


if __name__ == "__main__":
    unittest.main()
