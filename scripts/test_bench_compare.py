#!/usr/bin/env python3
"""Regression tests for scripts/bench-compare.py."""

from __future__ import annotations

import contextlib
import importlib.util
import io
import json
import tempfile
import unittest
from argparse import Namespace
from pathlib import Path

SCRIPT = Path(__file__).with_name("bench-compare.py")
SPEC = importlib.util.spec_from_file_location("bench_compare", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
COMPARE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(COMPARE)


def record(
    mean: float,
    *,
    standard_error: float = 0.5,
    samples: int = 20,
) -> dict:
    return {
        "mean_ns": mean,
        "mean_ci_ns": [mean - standard_error, mean + standard_error],
        "mean_standard_error_ns": standard_error,
        "median_ns": mean,
        "median_ci_ns": [mean - standard_error, mean + standard_error],
        "median_standard_error_ns": standard_error,
        "std_dev_ns": standard_error,
        "sample_count": samples,
    }


def policy(**overrides) -> dict:
    document = {
        "max_regression_percent": 5.0,
        "noise_percent": 1.0,
        "min_samples": 10,
        "noise_sigmas": 3.0,
        "missing_baseline": "fail",
    }
    document.update(overrides)
    return document


def write_criterion(root: Path, name: str, value: dict) -> None:
    estimate_path = root / name / "new" / "estimates.json"
    estimate_path.parent.mkdir(parents=True)
    estimate_path.write_text(
        json.dumps(
            {
                "mean": {
                    "point_estimate": value["mean_ns"],
                    "standard_error": value["mean_standard_error_ns"],
                    "confidence_interval": {
                        "lower_bound": value["mean_ci_ns"][0],
                        "upper_bound": value["mean_ci_ns"][1],
                    },
                },
                "median": {
                    "point_estimate": value["median_ns"],
                    "standard_error": value["median_standard_error_ns"],
                    "confidence_interval": {
                        "lower_bound": value["median_ci_ns"][0],
                        "upper_bound": value["median_ci_ns"][1],
                    },
                },
                "std_dev": {"point_estimate": value["std_dev_ns"]},
            }
        ),
        encoding="utf-8",
    )
    estimate_path.with_name("sample.json").write_text(
        json.dumps(
            {
                "sampling_mode": "Linear",
                "iters": [1.0] * value["sample_count"],
                "times": [value["mean_ns"]] * value["sample_count"],
            }
        ),
        encoding="utf-8",
    )


class BenchCompareTests(unittest.TestCase):
    def test_parser_reads_criterion_samples_and_estimate_fields(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            write_criterion(root, "group/benchmark", record(1000.0))
            benchmarks, errors = COMPARE.parse_criterion_directory(root)

        self.assertEqual(errors, [])
        self.assertEqual(benchmarks["group/benchmark"]["mean_ns"], 1000.0)
        self.assertEqual(benchmarks["group/benchmark"]["sample_count"], 20)
        self.assertEqual(benchmarks["group/benchmark"]["mean_ci_ns"], [999.5, 1000.5])

    def test_comparison_passes_within_threshold_and_noise_floor(self) -> None:
        document = COMPARE.compare_snapshots(
            {"bench": record(1000.0)}, {"bench": record(1055.0)}, policy()
        )

        self.assertEqual(document["summary"]["status"], "passed")
        self.assertEqual(document["results"][0]["status"], "passed")
        self.assertAlmostEqual(document["results"][0]["regression_percent"], 5.5)

    def test_comparison_rejects_material_regression(self) -> None:
        document = COMPARE.compare_snapshots(
            {"bench": record(1000.0)}, {"bench": record(1100.0)}, policy()
        )

        self.assertEqual(document["summary"]["status"], "failed")
        self.assertEqual(document["results"][0]["status"], "regression")
        self.assertIn("exceeds allowed delta", document["results"][0]["error"])

    def test_statistical_noise_suppresses_small_apparent_regression(self) -> None:
        document = COMPARE.compare_snapshots(
            {"bench": record(1000.0, standard_error=20.0)},
            {"bench": record(1080.0, standard_error=20.0)},
            policy(max_regression_percent=5.0, noise_percent=0.0),
        )

        self.assertEqual(document["summary"]["status"], "passed")
        self.assertGreater(
            document["results"][0]["allowed_delta_ns"],
            document["results"][0]["delta_ns"],
        )

    def test_insufficient_samples_fail_closed(self) -> None:
        document = COMPARE.compare_snapshots(
            {"bench": record(1000.0, samples=20)},
            {"bench": record(1001.0, samples=2)},
            policy(),
        )

        self.assertEqual(document["summary"]["status"], "failed")
        self.assertEqual(document["results"][0]["status"], "insufficient_samples")

    def test_missing_baseline_fails_by_default_and_can_warn(self) -> None:
        current = {"base": record(1000.0), "new": record(10.0)}

        strict = COMPARE.compare_snapshots({"base": record(1000.0)}, current, policy())
        tolerant = COMPARE.compare_snapshots(
            {"base": record(1000.0)}, current, policy(missing_baseline="warn")
        )

        self.assertEqual(strict["summary"]["status"], "failed")
        self.assertEqual(tolerant["summary"]["status"], "passed")
        self.assertEqual(tolerant["summary"]["warning_count"], 1)

    def test_disappearing_baseline_benchmark_always_fails(self) -> None:
        document = COMPARE.compare_snapshots(
            {"base": record(1000.0), "removed": record(10.0)},
            {"base": record(1000.0)},
            policy(),
        )

        self.assertEqual(document["summary"]["status"], "failed")
        statuses = {result["benchmark"]: result["status"] for result in document["results"]}
        self.assertEqual(statuses["removed"], "missing_current")

    def test_snapshot_can_be_saved_and_loaded_as_a_baseline(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            criterion = root / "criterion"
            snapshot = root / "baseline.json"
            write_criterion(criterion, "bench", record(1000.0))
            document = COMPARE.make_snapshot(criterion)
            COMPARE.write_json(snapshot, document)
            benchmarks, errors = COMPARE.load_source(snapshot)[:2]

        self.assertEqual(errors, [])
        self.assertEqual(benchmarks["bench"]["mean_ns"], 1000.0)

    def test_snapshot_command_rejects_invalid_criterion_data(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            estimate = root / "broken" / "new" / "estimates.json"
            estimate.parent.mkdir(parents=True)
            estimate.write_text("{}\n", encoding="utf-8")
            output = root / "snapshot.json"
            args = Namespace(criterion=root, output=output)

            with self.assertRaisesRegex(ValueError, "invalid Criterion estimate"):
                COMPARE.snapshot_command(args)

    def test_cli_reports_failure_and_writes_machine_readable_output(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            baseline = root / "baseline"
            current = root / "current"
            output = root / "comparison.json"
            write_criterion(baseline, "bench", record(1000.0))
            write_criterion(current, "bench", record(1100.0))
            args = Namespace(
                baseline=baseline,
                current=current,
                output=output,
                max_regression_percent=5.0,
                noise_percent=1.0,
                min_samples=10,
                noise_sigmas=3.0,
                missing_baseline="fail",
            )
            with contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(
                io.StringIO()
            ):
                exit_code = COMPARE.compare_command(args)
            document = json.loads(output.read_text(encoding="utf-8"))

        self.assertEqual(exit_code, 1)
        self.assertEqual(document["schema"], COMPARE.COMPARISON_SCHEMA)
        self.assertEqual(document["summary"]["status"], "failed")

    def test_parser_rejects_negative_noise_policy(self) -> None:
        args = Namespace(
            max_regression_percent=5.0,
            noise_percent=-1.0,
            min_samples=10,
            noise_sigmas=3.0,
            missing_baseline="fail",
        )
        with self.assertRaisesRegex(ValueError, "noise-percent"):
            COMPARE.validate_policy(args)


if __name__ == "__main__":
    unittest.main()
