#!/usr/bin/env python3
"""Compare Criterion benchmark reports under explicit quality thresholds.

Accepted inputs are raw Criterion output, release-gate reports, and snapshots
written by ``bench-compare.py``. The comparator rejects missing benchmarks,
unknown or insufficient sample counts, noisy measurements, and material mean
latency regressions. It selects one matching baseline when several platform
snapshots are supplied and fails if that choice is ambiguous.

This helper only detects relative regressions under comparable conditions. It
does not establish cross-implementation parity or a WireGuard performance claim.
"""

from __future__ import annotations

import argparse
import json
import math
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Iterable, Sequence

DEFAULT_MAX_REGRESSION_PERCENT = 10.0
DEFAULT_MAX_NOISE_PERCENT = 5.0
DEFAULT_MIN_SAMPLES = 10
EXPECTED_GATE_SCHEMA = "fuckipv6-release-gate-report"
EXPECTED_SNAPSHOT_SCHEMA = "fuckipv6-criterion-snapshot"


@dataclass(frozen=True)
class Measurement:
    benchmark: str
    mean_ns: float
    standard_error_ns: float | None
    sample_count: int | None


@dataclass(frozen=True)
class BenchmarkReport:
    path: Path
    measurements: dict[str, Measurement]
    platform: str | None
    machine: str | None
    mode: str | None
    source_kind: str


@dataclass(frozen=True)
class ComparisonOptions:
    max_regression_percent: float = DEFAULT_MAX_REGRESSION_PERCENT
    max_noise_percent: float = DEFAULT_MAX_NOISE_PERCENT
    min_samples: int = DEFAULT_MIN_SAMPLES
    require_benchmarks: tuple[str, ...] = ()
    include_prefixes: tuple[str, ...] = ()
    exclude_prefixes: tuple[str, ...] = ()
    allow_platform_mismatch: bool = False


def _number(value: Any, *, positive: bool = False) -> float | None:
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        return None
    number = float(value)
    if not math.isfinite(number) or (positive and number <= 0):
        return None
    return number


def _measurement_from_sample(sample: Any) -> Measurement | None:
    if not isinstance(sample, dict) or sample.get("error") is not None:
        return None
    name = sample.get("benchmark")
    mean = _number(sample.get("mean_ns"), positive=True)
    standard_error = _number(sample.get("standard_error_ns"))
    sample_count = sample.get("sample_count")
    if not isinstance(name, str) or not name or mean is None:
        return None
    if standard_error is not None and standard_error < 0:
        standard_error = None
    if (
        not isinstance(sample_count, int)
        or isinstance(sample_count, bool)
        or sample_count <= 0
    ):
        sample_count = None
    return Measurement(
        benchmark=name,
        mean_ns=mean,
        standard_error_ns=standard_error,
        sample_count=sample_count,
    )


def _read_criterion_directory(path: Path) -> BenchmarkReport:
    measurements: dict[str, Measurement] = {}
    for estimate_path in sorted(path.glob("**/new/estimates.json")):
        benchmark = "/".join(estimate_path.parent.parent.relative_to(path).parts)
        try:
            document = json.loads(estimate_path.read_text(encoding="utf-8"))
            mean = document["mean"]
            mean_ns = _number(mean["point_estimate"], positive=True)
            standard_error_ns = _number(mean.get("standard_error"))
            if mean_ns is None:
                continue
            sample_path = estimate_path.with_name("sample.json")
            sample_count = None
            if sample_path.is_file():
                sample = json.loads(sample_path.read_text(encoding="utf-8"))
                times = sample.get("times") if isinstance(sample, dict) else None
                if isinstance(times, list) and times:
                    sample_count = len(times)
        except (KeyError, OSError, TypeError, ValueError, json.JSONDecodeError):
            continue
        measurements[benchmark] = Measurement(
            benchmark=benchmark,
            mean_ns=mean_ns,
            standard_error_ns=standard_error_ns,
            sample_count=sample_count,
        )
    if not measurements:
        raise ValueError(f"Criterion directory contains no valid measurements: {path}")
    return BenchmarkReport(path, measurements, None, None, None, "criterion")


def _read_gate_report(path: Path, document: dict[str, Any]) -> BenchmarkReport:
    if document.get("schema") != EXPECTED_GATE_SCHEMA:
        raise ValueError(f"{path}: unsupported release-gate report schema")
    benchmark = document.get("benchmark")
    samples = benchmark.get("samples") if isinstance(benchmark, dict) else None
    if not isinstance(samples, list):
        raise ValueError(f"{path}: release-gate report has no benchmark samples")
    measurements: dict[str, Measurement] = {}
    for sample in samples:
        measurement = _measurement_from_sample(sample)
        if measurement is not None:
            measurements[measurement.benchmark] = measurement
    if not measurements:
        raise ValueError(f"{path}: benchmark report contains no valid measurements")
    platform_info = document.get("platform")
    hardware = document.get("hardware")
    return BenchmarkReport(
        path=path,
        measurements=measurements,
        platform=(
            platform_info.get("label") if isinstance(platform_info, dict) else None
        ),
        machine=(hardware.get("machine") if isinstance(hardware, dict) else None),
        mode=benchmark.get("mode") if isinstance(benchmark, dict) else None,
        source_kind="release_gate",
    )


def _read_snapshot(path: Path, document: dict[str, Any]) -> BenchmarkReport:
    if document.get("schema") != EXPECTED_SNAPSHOT_SCHEMA:
        raise ValueError(f"{path}: unsupported benchmark snapshot schema")
    if document.get("schema_version") != 1:
        raise ValueError(f"{path}: unsupported benchmark snapshot schema version")
    benchmarks = document.get("benchmarks")
    if not isinstance(benchmarks, dict):
        raise ValueError(f"{path}: snapshot benchmarks must be an object")
    measurements: dict[str, Measurement] = {}
    for name, record in benchmarks.items():
        if not isinstance(name, str) or not name or not isinstance(record, dict):
            continue
        mean_ns = _number(record.get("mean_ns"), positive=True)
        standard_error_ns = _number(record.get("mean_standard_error_ns"))
        sample_count = record.get("sample_count")
        if sample_count is not None and (
            not isinstance(sample_count, int)
            or isinstance(sample_count, bool)
            or sample_count <= 0
        ):
            sample_count = None
        if mean_ns is None:
            continue
        measurements[name] = Measurement(
            benchmark=name,
            mean_ns=mean_ns,
            standard_error_ns=standard_error_ns,
            sample_count=sample_count,
        )
    if not measurements:
        raise ValueError(f"{path}: snapshot contains no valid measurements")
    return BenchmarkReport(path, measurements, None, None, None, "snapshot")


def load_report(path: str | Path) -> BenchmarkReport:
    """Load a release-gate report, benchmark snapshot, or Criterion directory."""

    report_path = Path(path)
    if report_path.is_dir():
        return _read_criterion_directory(report_path)
    try:
        document = json.loads(report_path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as exc:
        raise ValueError(f"cannot read benchmark report {report_path}: {exc}") from exc
    if not isinstance(document, dict):
        raise ValueError(f"{report_path}: report root must be a JSON object")
    schema = document.get("schema")
    if schema == EXPECTED_GATE_SCHEMA:
        return _read_gate_report(report_path, document)
    if schema == EXPECTED_SNAPSHOT_SCHEMA:
        return _read_snapshot(report_path, document)
    raise ValueError(
        f"{report_path} is neither a release-gate report, benchmark snapshot, "
        "nor a Criterion directory"
    )


def _match_rank(candidate: str | None, baseline: str | None) -> int:
    if candidate is not None and baseline is not None:
        return 2 if candidate == baseline else 0
    return 1


def select_baseline(
    baselines: Sequence[BenchmarkReport],
    candidate: BenchmarkReport,
) -> BenchmarkReport:
    """Select the baseline with the best platform, machine, and name coverage.

    Multiple compatible baselines let a repository retain one accepted result
    per platform. Equal top scores are rejected instead of making an arbitrary
    choice that could hide a regression.
    """

    if not baselines:
        raise ValueError("at least one baseline is required")

    def score(report: BenchmarkReport) -> tuple[int, int, int, int]:
        common = set(report.measurements).intersection(candidate.measurements)
        return (
            _match_rank(candidate.platform, report.platform),
            _match_rank(candidate.machine, report.machine),
            len(common),
            -len(report.measurements),
        )

    ranked = sorted(baselines, key=score, reverse=True)
    if len(ranked) > 1 and score(ranked[0]) == score(ranked[1]):
        names = ", ".join(str(report.path) for report in ranked[:2])
        raise ValueError(
            f"baseline selection is ambiguous between {names}; pass one report"
        )
    return ranked[0]


def _selected(name: str, options: ComparisonOptions) -> bool:
    if options.include_prefixes and not any(
        name.startswith(prefix) for prefix in options.include_prefixes
    ):
        return False
    return not any(name.startswith(prefix) for prefix in options.exclude_prefixes)


def _measurement_quality_errors(
    label: str, measurement: Measurement, options: ComparisonOptions
) -> list[str]:
    errors: list[str] = []
    if measurement.sample_count is None:
        errors.append(f"{label} sample count is unavailable")
    elif measurement.sample_count < options.min_samples:
        errors.append(
            f"{label} has {measurement.sample_count} samples; "
            f"requires {options.min_samples}"
        )
    if measurement.standard_error_ns is None:
        errors.append(f"{label} standard error is unavailable")
    else:
        noise_percent = (
            measurement.standard_error_ns / measurement.mean_ns * 100.0
        )
        if noise_percent > options.max_noise_percent:
            errors.append(
                f"{label} noise {noise_percent:.3f}% exceeds "
                f"{options.max_noise_percent:.3f}%"
            )
    return errors


def compare_reports(
    baseline: BenchmarkReport,
    candidate: BenchmarkReport,
    options: ComparisonOptions,
) -> dict[str, Any]:
    """Compare two loaded reports without hiding failures behind averages."""

    if (
        not math.isfinite(options.max_regression_percent)
        or options.max_regression_percent < 0
    ):
        raise ValueError("max regression percent must be finite and non-negative")
    if (
        not math.isfinite(options.max_noise_percent)
        or not 0 < options.max_noise_percent <= 100
    ):
        raise ValueError("max noise percent must be in (0, 100]")
    if options.min_samples <= 0:
        raise ValueError("minimum samples must be greater than zero")
    if (
        not options.allow_platform_mismatch
        and baseline.platform is not None
        and candidate.platform is not None
        and baseline.platform != candidate.platform
    ):
        raise ValueError(
            f"platform mismatch: baseline={baseline.platform}, "
            f"candidate={candidate.platform}"
        )

    selected_baseline = {
        name: measurement
        for name, measurement in baseline.measurements.items()
        if _selected(name, options)
    }
    if not selected_baseline:
        raise ValueError("no baseline benchmarks match the requested filters")
    selected_candidate = {
        name
        for name in candidate.measurements
        if _selected(name, options)
    }
    required = set(options.require_benchmarks)
    required.update(selected_baseline)
    required.update(selected_candidate)
    comparisons: list[dict[str, Any]] = []
    errors: list[str] = []

    for name in sorted(required):
        baseline_measurement = baseline.measurements.get(name)
        candidate_measurement = candidate.measurements.get(name)
        if baseline_measurement is None:
            message = f"required benchmark is absent from baseline: {name}"
            errors.append(f"{name}: {message}")
            comparisons.append(
                {"benchmark": name, "status": "failed", "errors": [message]}
            )
            continue
        if candidate_measurement is None:
            message = f"required benchmark is absent from candidate: {name}"
            errors.append(f"{name}: {message}")
            comparisons.append(
                {"benchmark": name, "status": "failed", "errors": [message]}
            )
            continue

        result: dict[str, Any] = {
            "benchmark": name,
            "status": "passed",
            "baseline_mean_ns": baseline_measurement.mean_ns,
            "candidate_mean_ns": candidate_measurement.mean_ns,
            "regression_percent": round(
                (candidate_measurement.mean_ns / baseline_measurement.mean_ns - 1.0)
                * 100.0,
                6,
            ),
            "baseline_sample_count": baseline_measurement.sample_count,
            "candidate_sample_count": candidate_measurement.sample_count,
        }
        measurement_errors = [
            *_measurement_quality_errors(
                "baseline", baseline_measurement, options
            ),
            *_measurement_quality_errors(
                "candidate", candidate_measurement, options
            ),
        ]
        if result["regression_percent"] > options.max_regression_percent:
            measurement_errors.append(
                f"regression {result['regression_percent']:.3f}% exceeds "
                f"{options.max_regression_percent:.3f}%"
            )

        for label, measurement in (
            ("baseline", baseline_measurement),
            ("candidate", candidate_measurement),
        ):
            if measurement.standard_error_ns is not None:
                result[f"{label}_noise_percent"] = round(
                    measurement.standard_error_ns
                    / measurement.mean_ns
                    * 100.0,
                    6,
                )
        if measurement_errors:
            result["status"] = "failed"
            result["errors"] = measurement_errors
            errors.extend(f"{name}: {message}" for message in measurement_errors)
        comparisons.append(result)

    return {
        "schema": "fuckipv6-criterion-comparison",
        "schema_version": 1,
        "baseline": str(baseline.path),
        "candidate": str(candidate.path),
        "baseline_source": baseline.source_kind,
        "candidate_source": candidate.source_kind,
        "baseline_platform": baseline.platform,
        "candidate_platform": candidate.platform,
        "baseline_mode": baseline.mode,
        "candidate_mode": candidate.mode,
        "options": {
            "max_regression_percent": options.max_regression_percent,
            "max_noise_percent": options.max_noise_percent,
            "min_samples": options.min_samples,
            "require_benchmarks": sorted(options.require_benchmarks),
            "include_prefixes": list(options.include_prefixes),
            "exclude_prefixes": list(options.exclude_prefixes),
            "allow_platform_mismatch": options.allow_platform_mismatch,
        },
        "comparisons": comparisons,
        "summary": {
            "status": "passed" if not errors else "failed",
            "benchmark_count": len(comparisons),
            "failed_benchmark_count": sum(
                result["status"] == "failed" for result in comparisons
            ),
            "errors": errors,
        },
    }


def compare_paths(
    baseline_paths: Iterable[str | Path],
    candidate_path: str | Path,
    options: ComparisonOptions,
) -> dict[str, Any]:
    candidate = load_report(candidate_path)
    baselines = [load_report(path) for path in baseline_paths]
    baseline = select_baseline(baselines, candidate)
    return compare_reports(baseline, candidate, options)


def _build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--baseline", action="append", required=True)
    parser.add_argument("--candidate", required=True)
    parser.add_argument("--output")
    parser.add_argument(
        "--max-regression-percent", type=float, default=DEFAULT_MAX_REGRESSION_PERCENT
    )
    parser.add_argument(
        "--max-noise-percent", type=float, default=DEFAULT_MAX_NOISE_PERCENT
    )
    parser.add_argument("--min-samples", type=int, default=DEFAULT_MIN_SAMPLES)
    parser.add_argument("--benchmark", action="append", default=[])
    parser.add_argument("--include-prefix", action="append", default=[])
    parser.add_argument("--exclude-prefix", action="append", default=[])
    parser.add_argument("--allow-platform-mismatch", action="store_true")
    return parser


def main(argv: Sequence[str] | None = None) -> int:
    args = _build_parser().parse_args(argv)
    options = ComparisonOptions(
        max_regression_percent=args.max_regression_percent,
        max_noise_percent=args.max_noise_percent,
        min_samples=args.min_samples,
        require_benchmarks=tuple(args.benchmark),
        include_prefixes=tuple(args.include_prefix),
        exclude_prefixes=tuple(args.exclude_prefix),
        allow_platform_mismatch=args.allow_platform_mismatch,
    )
    try:
        result = compare_paths(args.baseline, args.candidate, options)
    except ValueError as exc:
        print(f"criterion-compare: {exc}", file=sys.stderr)
        return 2

    rendered = json.dumps(result, indent=2, sort_keys=True) + "\n"
    if args.output:
        output = Path(args.output)
        output.parent.mkdir(parents=True, exist_ok=True)
        output.write_text(rendered, encoding="utf-8")
    else:
        sys.stdout.write(rendered)
    for error in result["summary"]["errors"]:
        print(f"criterion-compare: FAIL: {error}", file=sys.stderr)
    return 0 if result["summary"]["status"] == "passed" else 1


if __name__ == "__main__":
    raise SystemExit(main())
