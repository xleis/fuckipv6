#!/usr/bin/env python3
"""Regression tests for scripts/release-gate.py."""

from __future__ import annotations

import contextlib
import importlib.util
import io
import json
import os
import tempfile
import unittest
from pathlib import Path
from unittest import mock

SCRIPT = Path(__file__).with_name("release-gate.py")
SPEC = importlib.util.spec_from_file_location("release_gate", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
GATE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(GATE)


def report(
    platform: str,
    *,
    commit: str = "a" * 40,
    status: str = "passed",
    dirty: bool = False,
    benchmark: bool = True,
) -> dict:
    checks = [
        {
            "name": "cross-check-x86_64-apple-darwin" if platform == "macos" else "check",
            "status": "passed",
        }
    ]
    if platform == "macos":
        checks.append({"name": "cross-check-aarch64-apple-darwin", "status": "passed"})
    return {
        "schema": GATE.EXPECTED_SCHEMA,
        "schema_version": GATE.SCHEMA_VERSION,
        "platform": {"label": platform},
        "source": {"commit": commit, "dirty": dirty},
        "checks": checks,
        "benchmark": {
            "status": "passed" if benchmark else "not_run",
            "mode": "quick" if benchmark else "none",
            "samples": [
                {"benchmark": "secure_session_v2_1420/seal", "mean_ns": 2_500.0}
            ]
            if benchmark
            else [],
        },
        "summary": {
            "status": status,
            "release_ready": status == "passed" and not dirty,
            "errors": [],
        },
        "release_policy": {
            "engineering_gate_qualified": status == "passed" and not dirty,
            "security_release_qualified": False,
        },
    }


def loaded(*documents: tuple[str, dict]) -> list[tuple[Path, dict]]:
    return [(Path(f"/reports/{name}.json"), document) for name, document in documents]


class ReleaseGateTests(unittest.TestCase):
    def test_aggregate_accepts_complete_platform_matrix(self) -> None:
        aggregate, errors = GATE.aggregate_documents(
            loaded(
                ("linux", report("linux")),
                ("windows", report("windows")),
                ("macos", report("macos")),
            ),
            GATE.DEFAULT_PLATFORMS,
        )

        self.assertEqual(errors, [])
        self.assertEqual(aggregate["summary"]["status"], "passed")
        self.assertEqual(aggregate["benchmarked_platforms"], ["linux", "macos", "windows"])
        self.assertFalse(aggregate["release_policy"]["security_release_qualified"])
        self.assertTrue(aggregate["release_policy"]["independent_security_audit_required"])

    def test_aggregate_rejects_missing_platform(self) -> None:
        aggregate, errors = GATE.aggregate_documents(
            loaded(("linux", report("linux")), ("windows", report("windows"))),
            GATE.DEFAULT_PLATFORMS,
        )

        self.assertEqual(aggregate["summary"]["status"], "failed")
        self.assertIn("missing platform reports: macos", errors)

    def test_aggregate_rejects_mixed_commits_and_dirty_reports(self) -> None:
        _, errors = GATE.aggregate_documents(
            loaded(
                ("linux", report("linux")),
                ("windows", report("windows", commit="b" * 40, dirty=True)),
                ("macos", report("macos")),
            ),
            GATE.DEFAULT_PLATFORMS,
        )

        self.assertIn("platform reports were generated from different or unknown commits", errors)
        self.assertTrue(any("source worktree is not clean" in error for error in errors))

    def test_aggregate_rejects_a_passing_but_non_release_ready_report(self) -> None:
        linux = report("linux")
        linux["summary"]["release_ready"] = False
        _, errors = GATE.aggregate_documents(
            loaded(
                ("linux", linux),
                ("windows", report("windows")),
                ("macos", report("macos")),
            ),
            GATE.DEFAULT_PLATFORMS,
        )

        self.assertIn(
            "/reports/linux.json: platform report is not release-ready", errors
        )

    def test_aggregate_requires_a_successful_benchmark(self) -> None:
        _, errors = GATE.aggregate_documents(
            loaded(
                ("linux", report("linux", benchmark=False)),
                ("windows", report("windows", benchmark=False)),
                ("macos", report("macos", benchmark=False)),
            ),
            GATE.DEFAULT_PLATFORMS,
        )

        self.assertIn("no platform contains a successful machine-readable benchmark", errors)

    def test_aggregate_rejects_error_only_benchmark_samples(self) -> None:
        linux = report("linux")
        linux["benchmark"]["samples"] = [
            {"benchmark": "secure_session_v2_1420/seal", "error": "invalid estimate"}
        ]
        _, errors = GATE.aggregate_documents(
            loaded(
                ("linux", linux),
                ("windows", report("windows", benchmark=False)),
                ("macos", report("macos", benchmark=False)),
            ),
            GATE.DEFAULT_PLATFORMS,
        )

        self.assertIn("no platform contains a successful machine-readable benchmark", errors)

    def test_aggregate_requires_both_macos_architectures(self) -> None:
        macos = report("macos")
        macos["checks"] = [{"name": "cross-check-x86_64-apple-darwin", "status": "passed"}]
        _, errors = GATE.aggregate_documents(
            loaded(
                ("linux", report("linux")),
                ("windows", report("windows")),
                ("macos", macos),
            ),
            GATE.DEFAULT_PLATFORMS,
        )

        self.assertIn(
            "macOS report is missing passed cross-check-aarch64-apple-darwin", errors
        )

    def test_aggregate_rejects_duplicate_platform(self) -> None:
        _, errors = GATE.aggregate_documents(
            loaded(
                ("linux", report("linux")),
                ("linux-again", report("linux")),
                ("windows", report("windows")),
                ("macos", report("macos")),
            ),
            GATE.DEFAULT_PLATFORMS,
        )

        self.assertTrue(any("duplicate platform report for linux" in error for error in errors))

    def test_aggregate_rejects_failed_check_even_if_summary_claims_success(self) -> None:
        linux = report("linux")
        linux["checks"].append({"name": "clippy", "status": "failed"})
        _, errors = GATE.aggregate_documents(
            loaded(
                ("linux", linux),
                ("windows", report("windows")),
                ("macos", report("macos")),
            ),
            GATE.DEFAULT_PLATFORMS,
        )

        self.assertIn("/reports/linux.json: check clippy did not pass", errors)

    def test_aggregate_tolerates_malformed_optional_sections(self) -> None:
        linux = report("linux")
        linux["checks"] = "not-a-list"
        linux["benchmark"] = None
        windows = report("windows")
        windows["source"] = None
        macos = report("macos")
        macos["summary"] = None

        _, errors = GATE.aggregate_documents(
            loaded(
                ("linux", linux),
                ("windows", windows),
                ("macos", macos),
            ),
            GATE.DEFAULT_PLATFORMS,
        )

        self.assertTrue(any("report contains no checks" in error for error in errors))
        self.assertTrue(any("missing Git commit" in error for error in errors))
        self.assertTrue(any("platform checks did not pass" in error for error in errors))

    def test_load_report_rejects_non_object_root(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "report.json"
            path.write_text("[]\n", encoding="utf-8")

            with self.assertRaisesRegex(ValueError, "root must be a JSON object"):
                GATE.load_report(path)

    def test_criterion_samples_are_machine_readable(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            criterion = Path(directory)
            estimates = criterion / "group" / "function" / "new" / "estimates.json"
            estimates.parent.mkdir(parents=True)
            estimates.write_text(
                json.dumps(
                    {
                        "mean": {
                            "point_estimate": 2800.0,
                            "standard_error": 2.0,
                            "confidence_interval": {
                                "lower_bound": 2790.0,
                                "upper_bound": 2810.0,
                            },
                        },
                        "median": {
                            "point_estimate": 2801.0,
                            "confidence_interval": {
                                "lower_bound": 2791.0,
                                "upper_bound": 2811.0,
                            },
                        },
                    }
                ),
                encoding="utf-8",
            )

            samples = GATE.parse_criterion_samples(criterion)

        self.assertEqual(len(samples), 1)
        self.assertEqual(samples[0]["benchmark"], "group/function")
        self.assertEqual(samples[0]["mean_ns"], 2800.0)
        self.assertEqual(samples[0]["mean_ci_ns"], [2790.0, 2810.0])
        self.assertEqual(len(GATE.valid_benchmark_samples({"samples": samples})), 1)

    def test_invalid_criterion_samples_are_not_benchmark_success(self) -> None:
        samples = [{"benchmark": "broken", "error": "invalid Criterion estimate file"}]

        self.assertEqual(GATE.valid_benchmark_samples({"samples": samples}), [])

    def test_toolchain_environment_pins_rustc_and_rustdoc(self) -> None:
        tools = {
            "cargo": "/toolchain/bin/cargo",
            "rustc": "/toolchain/bin/rustc",
            "rustdoc": "/toolchain/bin/rustdoc",
        }
        with mock.patch.object(GATE, "base_environment", return_value={"PATH": "/bin"}):
            with mock.patch.object(
                GATE, "rustup_tool_path", side_effect=lambda _, tool: tools[tool]
            ):
                env = GATE.toolchain_environment("stable")

        self.assertEqual(env["CARGO"], tools["cargo"])
        self.assertEqual(env["RUSTC"], tools["rustc"])
        self.assertEqual(env["RUSTDOC"], tools["rustdoc"])
        self.assertEqual(env["RUSTUP_TOOLCHAIN"], "stable")
        self.assertTrue(env["PATH"].startswith("/toolchain/bin" + GATE.os.pathsep))

    def test_parser_rejects_unknown_benchmark_mode(self) -> None:
        parser = GATE.build_parser()
        with self.assertRaises(SystemExit), contextlib.redirect_stderr(io.StringIO()):
            parser.parse_args(["run", "--benchmark", "unbounded"])

    def test_run_defaults_require_a_clean_worktree(self) -> None:
        parser = GATE.build_parser()
        args = parser.parse_args(["run"])
        GATE.apply_run_defaults(args)

        self.assertTrue(args.require_clean)

    def test_allow_dirty_is_explicitly_available_for_diagnostics(self) -> None:
        parser = GATE.build_parser()
        args = parser.parse_args(["run", "--allow-dirty"])
        GATE.apply_run_defaults(args)

        self.assertFalse(args.require_clean)

    def test_linux_defaults_include_musl_and_windows_cross_checks(self) -> None:
        parser = GATE.build_parser()
        args = parser.parse_args(["run", "--platform", "linux"])
        GATE.apply_run_defaults(args)
        plan = GATE.build_run_plan(args)

        self.assertEqual(
            plan["cross_targets"],
            ["x86_64-unknown-linux-musl", "x86_64-pc-windows-gnu"],
        )
        names = {command["name"] for command in plan["commands"]}
        self.assertIn("cross-check-x86_64-unknown-linux-musl", names)
        self.assertIn("cross-check-x86_64-pc-windows-gnu", names)

    def test_cross_targets_can_be_disabled_for_host_only_diagnostics(self) -> None:
        parser = GATE.build_parser()
        args = parser.parse_args(
            ["run", "--platform", "linux", "--no-default-cross-targets"]
        )
        GATE.apply_run_defaults(args)

        self.assertEqual(args.cross_target, [])

    def test_requested_toolchain_owns_cargo_rustc_and_clippy_lookup(self) -> None:
        def tool_path(toolchain: str, tool: str) -> str:
            self.assertEqual(toolchain, "stable")
            return f"/toolchain/bin/{tool}"

        with (
            mock.patch.object(GATE, "base_environment", return_value={"PATH": "/usr/bin"}),
            mock.patch.object(GATE, "rustup_tool_path", side_effect=tool_path),
        ):
            env = GATE.toolchain_environment("stable")

        self.assertEqual(env["CARGO"], "/toolchain/bin/cargo")
        self.assertEqual(env["RUSTC"], "/toolchain/bin/rustc")
        self.assertEqual(env["RUSTDOC"], "/toolchain/bin/rustdoc")
        self.assertEqual(env["RUSTUP_TOOLCHAIN"], "stable")
        self.assertEqual(
            env["PATH"].split(os.pathsep)[0],
            str(Path("/toolchain/bin/rustc").parent),
        )


if __name__ == "__main__":
    unittest.main()
