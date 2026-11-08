#!/usr/bin/env python3
"""Regression tests for scripts/governance-evidence.py."""

from __future__ import annotations

import importlib.util
import json
import sys
import tempfile
import unittest
from pathlib import Path

SCRIPT = Path(__file__).with_name("governance-evidence.py")
SPEC = importlib.util.spec_from_file_location("governance_evidence", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
EVIDENCE = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = EVIDENCE
SPEC.loader.exec_module(EVIDENCE)


def passing_results() -> list[dict]:
    names = {check for spec in EVIDENCE.CHECKS for check in spec.covers}
    names.update(check for spec in EVIDENCE.COVERAGE for check in spec.checks)
    return [
        {"name": name, "status": "passed"}
        for name in sorted(names)
    ]


class GovernanceEvidenceTests(unittest.TestCase):
    def test_cargo_counts_sum_suites_and_keep_ignored(self) -> None:
        output = (
            "test result: ok. 7 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out\n"
            "test result: ok. 3 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out\n"
        )

        self.assertEqual(
            EVIDENCE.parse_counts("cargo", output),
            {"passed": 10, "failed": 0, "ignored": 3},
        )

    def test_vitest_counts_are_parsed_without_ansi_noise(self) -> None:
        output = "\x1b[32m Test Files  3 passed (3)\x1b[0m\n Tests  12 passed (12)\n"

        self.assertEqual(
            EVIDENCE.parse_counts("vitest", output),
            {"passed": 12, "failed": 0, "ignored": 0},
        )

    def test_wire_versions_require_every_consumer_and_all_schemas(self) -> None:
        versions = {key: 1 for key in EVIDENCE.WIRE_VERSION_KEYS}
        versions.update(
            {
                "policy_transaction": 1,
                "audit_chain": 1,
                **{f"schema:node-{index}.schema.json": 1 for index in range(7)},
            }
        )

        report = EVIDENCE.version_report(versions, [])

        self.assertTrue(report["wire_contract"]["aligned"])
        self.assertEqual(report["wire_contract"]["expected"], 1)
        self.assertEqual(report["policy_transaction"]["version"], 1)
        self.assertEqual(report["audit_chain"]["version"], 1)

    def test_wire_version_mismatch_fails_closed(self) -> None:
        versions = {key: 1 for key in EVIDENCE.WIRE_VERSION_KEYS}
        versions["android_contract"] = 2
        versions.update(
            {f"schema:node-{index}.schema.json": 1 for index in range(7)}
        )

        report = EVIDENCE.version_report(versions, [])

        self.assertFalse(report["wire_contract"]["aligned"])
        self.assertIsNone(report["wire_contract"]["expected"])

    def test_coverage_requires_policy_disconnect_and_stable_source(self) -> None:
        results = passing_results()
        coverage = EVIDENCE.coverage_report(results, True, False)
        by_name = {item["criterion"]: item for item in coverage}

        self.assertEqual(
            by_name["concurrent-policy-apply-rollback-disconnect"]["status"],
            "passed",
        )
        self.assertEqual(
            by_name["evidence-bound-to-policy-version-and-commit"]["status"],
            "failed",
        )

        for result in results:
            if result["name"] == "policy-runtime-disconnect":
                result["status"] = "failed"
        coverage = EVIDENCE.coverage_report(results, True, True)
        by_name = {item["criterion"]: item for item in coverage}

        self.assertEqual(
            by_name["concurrent-policy-apply-rollback-disconnect"]["status"],
            "failed",
        )
        self.assertEqual(
            by_name["evidence-bound-to-policy-version-and-commit"]["status"],
            "passed",
        )

    def test_report_json_is_written_atomically(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            output = Path(temporary) / "nested" / "report.json"
            EVIDENCE.write_json(output, {"schema": EVIDENCE.REPORT_SCHEMA})

            self.assertEqual(
                json.loads(output.read_text(encoding="utf-8"))["schema"],
                EVIDENCE.REPORT_SCHEMA,
            )
            self.assertFalse(output.with_suffix(".json.tmp").exists())


if __name__ == "__main__":
    unittest.main()
