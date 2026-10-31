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

COMPLETE_UPSTREAM_RESULTS = {name: "success" for name in GATE.UPSTREAM_JOBS}


def report(
    platform: str,
    *,
    commit: str = "a" * 40,
    status: str = "passed",
    dirty: bool = False,
    benchmark: bool = True,
) -> dict:
    checks = [
        {"name": name, "status": "passed"}
        for name in GATE.required_check_names(platform)
    ]
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
            COMPLETE_UPSTREAM_RESULTS,
        )

        self.assertEqual(errors, [])
        self.assertEqual(aggregate["summary"]["status"], "passed")
        self.assertEqual(aggregate["benchmarked_platforms"], ["linux", "macos", "windows"])
        self.assertTrue(all(job["passed"] for job in aggregate["upstream_jobs"].values()))
        self.assertFalse(aggregate["release_policy"]["security_release_qualified"])
        self.assertTrue(aggregate["release_policy"]["independent_security_audit_required"])

    def test_aggregate_rejects_missing_required_platform_check(self) -> None:
        linux = report("linux")
        linux["checks"] = [
            check
            for check in linux["checks"]
            if check["name"] != "cross-check-x86_64-unknown-linux-musl"
        ]

        _, errors = GATE.aggregate_documents(
            loaded(
                ("linux", linux),
                ("windows", report("windows")),
                ("macos", report("macos")),
            ),
            GATE.DEFAULT_PLATFORMS,
            COMPLETE_UPSTREAM_RESULTS,
        )

        self.assertIn(
            "/reports/linux.json: required check "
            "cross-check-x86_64-unknown-linux-musl is missing or did not pass",
            errors,
        )

    def test_aggregate_rejects_failed_upstream_job(self) -> None:
        results = COMPLETE_UPSTREAM_RESULTS.copy()
        results["gpui-smoke"] = "failure"

        _, errors = GATE.aggregate_documents(
            loaded(
                ("linux", report("linux")),
                ("windows", report("windows")),
                ("macos", report("macos")),
            ),
            GATE.DEFAULT_PLATFORMS,
            results,
        )

        self.assertIn("upstream job gpui-smoke did not succeed: failure", errors)

    def test_aggregate_rejects_missing_upstream_job(self) -> None:
        results = COMPLETE_UPSTREAM_RESULTS.copy()
        del results["docs"]

        _, errors = GATE.aggregate_documents(
            loaded(
                ("linux", report("linux")),
                ("windows", report("windows")),
                ("macos", report("macos")),
            ),
            GATE.DEFAULT_PLATFORMS,
            results,
        )

        self.assertIn("missing upstream job result: docs", errors)

    def test_upstream_result_parser_rejects_malformed_and_duplicate_values(self) -> None:
        results, errors = GATE.parse_upstream_results(
            ["docs=success", "docs=failure", "broken"]
        )

        self.assertEqual(results, {"docs": "success"})
        self.assertIn("duplicate upstream result for docs", errors)
        self.assertTrue(any("expected JOB=RESULT" in error for error in errors))

    def test_aggregate_gate_fails_cleanly_for_duplicate_upstream_results(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            report_paths = []
            for platform_name in GATE.DEFAULT_PLATFORMS:
                path = root / f"{platform_name}.json"
                GATE.write_json(path, report(platform_name))
                report_paths.append(path)
            output = root / "aggregate.json"
            arguments = [
                "aggregate",
                "--reports",
                *(str(path) for path in report_paths),
                "--output",
                str(output),
            ]
            for name, result in COMPLETE_UPSTREAM_RESULTS.items():
                arguments.extend(("--upstream-result", f"{name}={result}"))
                if name == "docs":
                    arguments.extend(("--upstream-result", "docs=failure"))
            for platform_name in GATE.DEFAULT_PLATFORMS:
                arguments.extend(("--require-platform", platform_name))

            args = GATE.build_parser().parse_args(arguments)
            with (
                contextlib.redirect_stdout(io.StringIO()),
                contextlib.redirect_stderr(io.StringIO()),
            ):
                exit_code = GATE.aggregate_gate(args)

            aggregate = json.loads(output.read_text(encoding="utf-8"))
            self.assertEqual(exit_code, 1)
            self.assertEqual(aggregate["summary"]["status"], "failed")
            self.assertFalse(aggregate["summary"]["release_ready"])
            self.assertFalse(
                aggregate["release_policy"]["engineering_gate_qualified"]
            )
            self.assertIn(
                "duplicate upstream result for docs",
                aggregate["summary"]["errors"],
            )

    def test_upstream_result_validation_rejects_unknown_jobs(self) -> None:
        results = {**COMPLETE_UPSTREAM_RESULTS, "unexpected": "success"}

        errors = GATE.validate_upstream_results(results)

        self.assertIn("unknown upstream job results: unexpected", errors)

    def test_aggregate_rejects_missing_platform(self) -> None:
        aggregate, errors = GATE.aggregate_documents(
            loaded(("linux", report("linux")), ("windows", report("windows"))),
            GATE.DEFAULT_PLATFORMS,
            COMPLETE_UPSTREAM_RESULTS,
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
            COMPLETE_UPSTREAM_RESULTS,
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
            COMPLETE_UPSTREAM_RESULTS,
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
            COMPLETE_UPSTREAM_RESULTS,
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
            COMPLETE_UPSTREAM_RESULTS,
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
            COMPLETE_UPSTREAM_RESULTS,
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
            COMPLETE_UPSTREAM_RESULTS,
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
            COMPLETE_UPSTREAM_RESULTS,
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
            COMPLETE_UPSTREAM_RESULTS,
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

    def test_source_stability_check_accepts_an_unchanged_head(self) -> None:
        source = {"commit": "a" * 40, "branch": "main", "dirty": False}

        check = GATE.source_stability_check(source, source.copy())

        self.assertEqual(check["status"], "passed")
        self.assertEqual(check["start_commit"], "a" * 40)
        self.assertEqual(check["end_commit"], "a" * 40)

    def test_source_stability_check_rejects_head_changes_during_gate(self) -> None:
        start = {"commit": "a" * 40, "branch": "main", "dirty": False}
        end = {"commit": "b" * 40, "branch": "main", "dirty": False}

        check = GATE.source_stability_check(start, end)

        self.assertEqual(check["status"], "failed")
        self.assertIn("HEAD changed", check["error"])

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

    def test_benchmark_comparison_policy_is_part_of_the_run_plan(self) -> None:
        parser = GATE.build_parser()
        args = parser.parse_args(
            [
                "run",
                "--platform",
                "linux",
                "--benchmark",
                "full",
                "--benchmark-baseline",
                "target/linux.json",
                "--benchmark-baseline",
                "target/windows.json",
                "--benchmark-max-regression-percent",
                "4",
                "--benchmark-max-noise-percent",
                "2",
                "--benchmark-min-samples",
                "25",
            ]
        )
        GATE.apply_run_defaults(args)
        plan = GATE.build_run_plan(args)

        comparison = plan["benchmark_comparison_command"]
        self.assertIsNotNone(comparison)
        self.assertIn("scripts/criterion_compare.py", comparison["arguments"][1])
        self.assertEqual(
            comparison["arguments"].count("--baseline"),
            2,
        )
        self.assertIn("4.0", comparison["arguments"])
        self.assertIn("2.0", comparison["arguments"])
        self.assertIn("25", comparison["arguments"])
        self.assertEqual(plan["benchmark_quality"]["min_samples"], 25)

    def test_benchmark_comparison_requires_benchmarking(self) -> None:
        parser = GATE.build_parser()
        args = parser.parse_args(
            [
                "run",
                "--benchmark",
                "none",
                "--benchmark-baseline",
                "target/baseline.json",
            ]
        )

        with self.assertRaisesRegex(ValueError, "requires Criterion benchmarking"):
            GATE.apply_run_defaults(args)

    def test_benchmark_comparison_rejects_too_few_samples(self) -> None:
        parser = GATE.build_parser()
        args = parser.parse_args(["run", "--benchmark-min-samples", "0"])

        with self.assertRaisesRegex(ValueError, "must be at least 1"):
            GATE.apply_run_defaults(args)

    def test_benchmark_comparison_rejects_invalid_noise_policy(self) -> None:
        parser = GATE.build_parser()
        args = parser.parse_args(["run", "--benchmark-max-noise-percent", "101"])

        with self.assertRaisesRegex(ValueError, "must be in"):
            GATE.apply_run_defaults(args)

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

    def test_plan_runs_single_binary_and_documentation_checks(self) -> None:
        parser = GATE.build_parser()
        args = parser.parse_args(["run", "--platform", "linux"])
        GATE.apply_run_defaults(args)
        plan = GATE.build_run_plan(args)

        commands = {command["name"]: command["arguments"] for command in plan["commands"]}
        self.assertIn("scripts/check-single-binary.py", commands["single-binary"])
        self.assertIn("scripts/check-docs.py", commands["docs"])
        self.assertIn("scripts/test_check_single_binary.py", commands["helper-tests"])
        self.assertIn("scripts/test_check_docs.py", commands["helper-tests"])

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
