from __future__ import annotations

import importlib.util
import json
import sys
import tempfile
import unittest
from pathlib import Path

SCRIPT = Path(__file__).with_name("criterion_compare.py")
SPEC = importlib.util.spec_from_file_location("criterion_compare", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
COMPARE = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = COMPARE
SPEC.loader.exec_module(COMPARE)


def measurement(
    name: str,
    *,
    mean: float = 1_000.0,
    error: float | None = 10.0,
    samples: int | None = 100,
) -> dict:
    result = {
        "benchmark": name,
        "mean_ns": mean,
    }
    if error is not None:
        result["standard_error_ns"] = error
    if samples is not None:
        result["sample_count"] = samples
    return result


def report(
    path: Path,
    samples: list[dict],
    *,
    platform: str = "linux",
    machine: str = "x86_64",
    mode: str = "full",
) -> None:
    path.write_text(
        json.dumps(
            {
                "schema": COMPARE.EXPECTED_GATE_SCHEMA,
                "platform": {"label": platform},
                "hardware": {"machine": machine},
                "benchmark": {"mode": mode, "samples": samples},
            }
        ),
        encoding="utf-8",
    )


class CriterionCompareTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name)

    def tearDown(self) -> None:
        self.temporary.cleanup()

    def path(self, name: str) -> Path:
        return self.root / name

    def compare(
        self,
        baseline: dict,
        candidate: dict,
        **options,
    ) -> dict:
        baseline_path = self.path("baseline.json")
        candidate_path = self.path("candidate.json")
        report(baseline_path, [baseline])
        report(candidate_path, [candidate])
        return COMPARE.compare_paths(
            [baseline_path],
            candidate_path,
            COMPARE.ComparisonOptions(**options),
        )

    def test_equal_samples_pass_and_regression_is_measured(self) -> None:
        result = self.compare(
            measurement("seal"),
            measurement("seal", mean=1_050.0),
            max_regression_percent=10.0,
        )
        self.assertEqual(result["summary"]["status"], "passed")
        self.assertAlmostEqual(result["comparisons"][0]["regression_percent"], 5.0)

    def test_regression_threshold_fails(self) -> None:
        result = self.compare(
            measurement("seal"),
            measurement("seal", mean=1_111.0),
            max_regression_percent=10.0,
        )
        self.assertEqual(result["summary"]["status"], "failed")
        self.assertIn("exceeds 10.000%", result["summary"]["errors"][0])

    def test_missing_required_candidate_benchmark_fails(self) -> None:
        result = self.compare(measurement("seal"), measurement("open"))
        self.assertEqual(result["summary"]["status"], "failed")
        self.assertIn(
            "seal: required benchmark is absent from candidate: seal",
            result["summary"]["errors"],
        )

    def test_noisy_measurement_fails(self) -> None:
        result = self.compare(
            measurement("seal"),
            measurement("seal", error=80.0),
            max_noise_percent=5.0,
        )
        self.assertEqual(result["summary"]["status"], "failed")
        self.assertIn("candidate noise 8.000%", result["summary"]["errors"][0])

    def test_insufficient_sample_count_fails(self) -> None:
        result = self.compare(
            measurement("seal", samples=20),
            measurement("seal", samples=1),
            min_samples=10,
        )
        self.assertEqual(result["summary"]["status"], "failed")
        self.assertIn("candidate has 1 samples", result["summary"]["errors"][0])

    def test_unknown_sample_count_always_fails_closed(self) -> None:
        result = self.compare(
            measurement("seal", samples=None),
            measurement("seal"),
            min_samples=1,
        )
        self.assertEqual(result["summary"]["status"], "failed")
        self.assertIn("baseline sample count is unavailable", result["summary"]["errors"][0])

    def test_unknown_standard_error_always_fails_closed(self) -> None:
        result = self.compare(
            measurement("seal", error=None),
            measurement("seal"),
        )
        self.assertEqual(result["summary"]["status"], "failed")
        self.assertIn("baseline standard error is unavailable", result["summary"]["errors"][0])

    def test_explicit_required_benchmark_must_exist(self) -> None:
        result = self.compare(
            measurement("seal"),
            measurement("seal"),
            require_benchmarks=("open",),
        )
        self.assertEqual(result["summary"]["status"], "failed")
        self.assertIn(
            "open: required benchmark is absent from baseline: open",
            result["summary"]["errors"],
        )

    def test_candidate_only_benchmark_requires_a_baseline(self) -> None:
        result = self.compare(measurement("seal"), measurement("new_path"))
        self.assertEqual(result["summary"]["status"], "failed")
        self.assertIn(
            "new_path: required benchmark is absent from baseline: new_path",
            result["summary"]["errors"],
        )

    def test_platform_mismatch_fails_by_default(self) -> None:
        baseline_path = self.path("baseline.json")
        candidate_path = self.path("candidate.json")
        report(baseline_path, [measurement("seal")], platform="linux")
        report(candidate_path, [measurement("seal")], platform="windows")
        with self.assertRaisesRegex(ValueError, "platform mismatch"):
            COMPARE.compare_paths(
                [baseline_path], candidate_path, COMPARE.ComparisonOptions()
            )

    def test_best_matching_baseline_is_selected(self) -> None:
        candidate_path = self.path("candidate.json")
        linux_path = self.path("linux.json")
        windows_path = self.path("windows.json")
        report(candidate_path, [measurement("seal")], platform="linux")
        report(linux_path, [measurement("seal")], platform="linux")
        report(windows_path, [measurement("seal")], platform="windows")
        result = COMPARE.compare_paths(
            [windows_path, linux_path], candidate_path, COMPARE.ComparisonOptions()
        )
        self.assertEqual(Path(result["baseline"]), linux_path)

    def test_bench_compare_snapshot_is_accepted(self) -> None:
        baseline_path = self.path("snapshot.json")
        candidate_path = self.path("candidate.json")
        report(candidate_path, [measurement("seal")])
        baseline_path.write_text(
            json.dumps(
                {
                    "schema": COMPARE.EXPECTED_SNAPSHOT_SCHEMA,
                    "schema_version": 1,
                    "benchmarks": {
                        "seal": {
                            "mean_ns": 1_000.0,
                            "mean_standard_error_ns": 10.0,
                            "sample_count": 100,
                        }
                    },
                }
            ),
            encoding="utf-8",
        )
        result = COMPARE.compare_paths(
            [baseline_path], candidate_path, COMPARE.ComparisonOptions()
        )
        self.assertEqual(result["summary"]["status"], "passed")
        self.assertEqual(result["baseline_source"], "snapshot")

    def test_raw_criterion_directory_records_sample_count(self) -> None:
        benchmark = self.root / "criterion" / "group" / "function" / "new"
        benchmark.mkdir(parents=True)
        (benchmark / "estimates.json").write_text(
            json.dumps(
                {
                    "mean": {
                        "point_estimate": 1000.0,
                        "standard_error": 10.0,
                        "confidence_interval": {
                            "lower_bound": 990.0,
                            "upper_bound": 1010.0,
                        },
                    }
                }
            ),
            encoding="utf-8",
        )
        (benchmark / "sample.json").write_text(
            json.dumps(
                {
                    "sampling_mode": "Linear",
                    "iters": [1.0, 2.0, 3.0],
                    "times": [1000.0, 2000.0, 3000.0],
                }
            ),
            encoding="utf-8",
        )
        loaded = COMPARE.load_report(self.root / "criterion")
        self.assertEqual(loaded.measurements["group/function"].sample_count, 3)

    def test_cli_writes_machine_readable_output(self) -> None:
        baseline_path = self.path("baseline.json")
        candidate_path = self.path("candidate.json")
        output_path = self.path("comparison.json")
        report(baseline_path, [measurement("seal")])
        report(candidate_path, [measurement("seal")])
        code = COMPARE.main(
            [
                "--baseline",
                str(baseline_path),
                "--candidate",
                str(candidate_path),
                "--output",
                str(output_path),
            ]
        )
        self.assertEqual(code, 0)
        result = json.loads(output_path.read_text(encoding="utf-8"))
        self.assertEqual(result["summary"]["status"], "passed")


if __name__ == "__main__":
    unittest.main()
