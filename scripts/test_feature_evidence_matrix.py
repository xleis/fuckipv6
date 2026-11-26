#!/usr/bin/env python3
"""Regression tests for scripts/feature-evidence-matrix.py."""

from __future__ import annotations

import importlib.util
import json
import tempfile
import unittest
from pathlib import Path

SCRIPT = Path(__file__).with_name("feature-evidence-matrix.py")
SPEC = importlib.util.spec_from_file_location("feature_evidence_matrix", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
MATRIX = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MATRIX)


def manifest(actions: list[dict]) -> dict:
    return {
        "schema_version": 1,
        "pages": [
            {
                "id": "networks",
                "actions": actions,
            }
        ],
    }


def action(action_id: str, command: str, status: str = "implemented") -> dict:
    return {
        "id": action_id,
        "command": command,
        "permission": "view_state",
        "audit": {"policy": "none"},
        "backend_status": status,
        "availability": "always",
    }


class FeatureEvidenceMatrixTest(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name)
        self.manifest_path = self.root / "node-manifest.json"
        self.output = self.root / "FEATURE_EVIDENCE.md"
        self.source = self.root / "command.rs"
        MATRIX.PROJECT_ROOT = self.root
        MATRIX.EVIDENCE_SOURCES = ("command.rs",)

    def tearDown(self) -> None:
        self.temporary.cleanup()

    def write(self, actions: list[dict], source: str) -> None:
        self.manifest_path.write_text(json.dumps(manifest(actions)), encoding="utf-8")
        self.source.write_text(source, encoding="utf-8")

    def test_every_action_needs_an_evidence_source_mentioning_it(self) -> None:
        self.write([action("create_network", "create_network")], 'NodeCommandKind::CreateNetwork => "create_network"\n')
        report = MATRIX.check(self.manifest_path)
        self.assertEqual(report["status"], "passed", report["errors"])
        self.assertEqual(report["rows"][0]["evidence"], "command.rs")

    def test_a_command_without_evidence_fails(self) -> None:
        self.write([action("ghost", "ghost_command")], '"create_network"\n')
        report = MATRIX.check(self.manifest_path)
        self.assertEqual(report["status"], "failed")
        self.assertTrue(any("ghost_command" in error for error in report["errors"]))

    def test_counts_split_implemented_and_unavailable(self) -> None:
        self.write(
            [
                action("a", "create_network"),
                action("b", "get_traffic_policy", status="unavailable"),
            ],
            '"create_network" "get_traffic_policy"\n',
        )
        report = MATRIX.check(self.manifest_path)
        self.assertEqual(report["actions"], 2)
        self.assertEqual(report["implemented"], 1)
        self.assertEqual(report["not_implemented"], 1)

    def test_rendered_table_lists_every_action(self) -> None:
        self.write([action("a", "create_network")], '"create_network"\n')
        rendered = MATRIX.render(MATRIX.check(self.manifest_path))
        self.assertIn("| `networks` | `a` | `create_network` |", rendered)
        self.assertIn("可见动作：**1**", rendered)

    def test_stale_output_is_rejected(self) -> None:
        self.write([action("a", "create_network")], '"create_network"\n')
        exit_code = MATRIX.main(
            ["--manifest", str(self.manifest_path), "--output", str(self.output), "--check"]
        )
        self.assertEqual(exit_code, 1)  # nothing written yet, so stale
        exit_code = MATRIX.main(["--manifest", str(self.manifest_path), "--output", str(self.output)])
        self.assertEqual(exit_code, 0)
        exit_code = MATRIX.main(
            ["--manifest", str(self.manifest_path), "--output", str(self.output), "--check"]
        )
        self.assertEqual(exit_code, 0)

    def test_missing_manifest_is_operational_error(self) -> None:
        exit_code = MATRIX.main(["--manifest", str(self.root / "missing.json")])
        self.assertEqual(exit_code, 2)


if __name__ == "__main__":
    unittest.main()
