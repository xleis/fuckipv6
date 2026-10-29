#!/usr/bin/env python3
"""Create and compare machine-readable Criterion benchmark snapshots.

This helper detects relative regressions within the benchmarks that were run.
It does not establish absolute performance, cross-implementation parity, or a
WireGuard comparison. Samples that are too small to support a decision fail
closed instead of producing a release-ready result.
"""

from __future__ import annotations

import argparse
import json
import math
import sys
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

SNAPSHOT_SCHEMA = "fuckipv6-criterion-snapshot"
COMPARISON_SCHEMA = "fuckipv6-criterion-comparison"
SCHEMA_VERSION = 1


def utc_now() -> str:
    return datetime.now(timezone.utc).isoformat(timespec="seconds").replace("+00:00", "Z")


def write_json(path: Path, document: dict[str, Any]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_suffix(path.suffix + ".tmp")
    temporary.write_text(
        json.dumps(document, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    temporary.replace(path)


def _finite_number(value: Any, *, positive: bool = False) -> float | None:
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        return None
    number = float(value)
    if not math.isfinite(number) or (positive and number <= 0):
        return None
    return number


def _estimate_value(estimate: dict[str, Any], name: str) -> dict[str, Any]:
    value = estimate.get(name)
    if not isinstance(value, dict):
        raise ValueError(f"missing {name} estimate")
    point = _finite_number(value.get("point_estimate"), positive=True)
    if point is None:
        raise ValueError(f"invalid {name} point estimate")
    confidence = value.get("confidence_interval")
    if not isinstance(confidence, dict):
        raise ValueError(f"missing {name} confidence interval")
    lower = _finite_number(confidence.get("lower_bound"))
    upper = _finite_number(confidence.get("upper_bound"))
    if lower is None or upper is None or lower > point or point > upper:
        raise ValueError(f"invalid {name} confidence interval")
    standard_error = _finite_number(value.get("standard_error"))
    if standard_error is None or standard_error < 0:
        raise ValueError(f"invalid {name} standard error")
    return {"point": point, "lower": lower, "upper": upper, "standard_error": standard_error}


def _sample_count(sample_path: Path) -> int | None:
    try:
        document = json.loads(sample_path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError):
        return None
    times = document.get("times") if isinstance(document, dict) else None
    return len(times) if isinstance(times, list) and times else None


def parse_criterion_directory(criterion_dir: Path) -> tuple[dict[str, Any], list[str]]:
    """Read every Criterion estimate below *criterion_dir*."""
    benchmarks: dict[str, Any] = {}
    errors: list[str] = []
    estimate_paths = sorted(criterion_dir.glob("**/new/estimates.json"))
    if not estimate_paths:
        errors.append(f"{criterion_dir}: no Criterion estimates.json files found")
        return benchmarks, errors

    for estimate_path in estimate_paths:
        benchmark = "/".join(estimate_path.parent.parent.relative_to(criterion_dir).parts)
        try:
            estimate = json.loads(estimate_path.read_text(encoding="utf-8"))
            if not isinstance(estimate, dict):
                raise ValueError("estimate root must be a JSON object")
            mean = _estimate_value(estimate, "mean")
            median = _estimate_value(estimate, "median")
            std_dev = estimate.get("std_dev")
            std_dev_ns = None
            if isinstance(std_dev, dict):
                std_dev_ns = _finite_number(std_dev.get("point_estimate"))
            sample_count = _sample_count(estimate_path.with_name("sample.json"))
            benchmarks[benchmark] = {
                "mean_ns": mean["point"],
                "mean_ci_ns": [mean["lower"], mean["upper"]],
                "mean_standard_error_ns": mean["standard_error"],
                "median_ns": median["point"],
                "median_ci_ns": [median["lower"], median["upper"]],
                "median_standard_error_ns": median["standard_error"],
                "std_dev_ns": std_dev_ns,
                "sample_count": sample_count,
            }
        except (OSError, ValueError, TypeError, json.JSONDecodeError) as error:
            errors.append(f"{benchmark}: invalid Criterion estimate: {error}")

    return benchmarks, errors


def load_snapshot(path: Path) -> tuple[dict[str, Any], list[str]]:
    document = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(document, dict):
        raise ValueError(f"{path}: snapshot root must be a JSON object")
    if document.get("schema") != SNAPSHOT_SCHEMA:
        raise ValueError(f"{path}: unsupported snapshot schema")
    if document.get("schema_version") != SCHEMA_VERSION:
        raise ValueError(f"{path}: unsupported snapshot schema version")
    benchmarks = document.get("benchmarks")
    if not isinstance(benchmarks, dict):
        raise ValueError(f"{path}: snapshot benchmarks must be an object")

    errors: list[str] = []
    valid: dict[str, Any] = {}
    for name, record in benchmarks.items():
        if not isinstance(name, str) or not name or not isinstance(record, dict):
            errors.append(f"{path}: invalid benchmark entry")
            continue
        mean = _finite_number(record.get("mean_ns"), positive=True)
        sample_count = record.get("sample_count")
        if not isinstance(sample_count, int) or isinstance(sample_count, bool) or sample_count < 1:
            sample_count = None
        if mean is None:
            errors.append(f"{name}: invalid snapshot mean_ns")
            continue
        valid[name] = {**record, "mean_ns": mean, "sample_count": sample_count}
    return valid, errors


def load_source(path: Path) -> tuple[dict[str, Any], list[str], str]:
    if path.is_dir():
        benchmarks, errors = parse_criterion_directory(path)
        return benchmarks, errors, "criterion_directory"
    if path.is_file():
        benchmarks, errors = load_snapshot(path)
        return benchmarks, errors, "snapshot"
    raise ValueError(f"{path}: baseline/current path does not exist")


def make_snapshot(criterion_dir: Path) -> dict[str, Any]:
    benchmarks, errors = parse_criterion_directory(criterion_dir)
    if errors:
        raise ValueError("; ".join(errors))
    return {
        "schema": SNAPSHOT_SCHEMA,
        "schema_version": SCHEMA_VERSION,
        "generated_at": utc_now(),
        "source": {"kind": "criterion_directory", "path": str(criterion_dir.resolve())},
        "benchmarks": benchmarks,
    }


def _record_is_usable(record: Any, minimum_samples: int) -> bool:
    if not isinstance(record, dict):
        return False
    mean = _finite_number(record.get("mean_ns"), positive=True)
    count = record.get("sample_count")
    return (
        mean is not None
        and isinstance(count, int)
        and not isinstance(count, bool)
        and count >= minimum_samples
    )


def _comparison_record(
    name: str,
    baseline: Any,
    current: Any,
    policy: dict[str, Any],
) -> dict[str, Any]:
    minimum_samples = policy["min_samples"]
    if baseline is None:
        status = "missing_baseline"
        if policy["missing_baseline"] == "warn":
            status = "new_benchmark"
        return {"benchmark": name, "status": status, "baseline": None, "current": current}
    if current is None:
        return {
            "benchmark": name,
            "status": "missing_current",
            "baseline": baseline,
            "current": None,
        }
    if not _record_is_usable(baseline, minimum_samples) or not _record_is_usable(
        current, minimum_samples
    ):
        return {
            "benchmark": name,
            "status": "insufficient_samples",
            "baseline": baseline,
            "current": current,
        }

    baseline_mean = float(baseline["mean_ns"])
    current_mean = float(current["mean_ns"])
    delta_ns = current_mean - baseline_mean
    regression_percent = delta_ns / baseline_mean * 100.0
    threshold_ns = baseline_mean * policy["max_regression_percent"] / 100.0
    noise_floor_ns = baseline_mean * policy["noise_percent"] / 100.0
    baseline_se = _finite_number(baseline.get("mean_standard_error_ns"))
    current_se = _finite_number(current.get("mean_standard_error_ns"))
    statistical_margin_ns = None
    if baseline_se is not None and current_se is not None:
        statistical_margin_ns = policy["noise_sigmas"] * math.hypot(baseline_se, current_se)
    # Percentage noise is an allowance in addition to the configured
    # regression budget. The statistical margin remains an independent floor
    # when standard errors or sample counts make the estimate uncertain.
    allowed_delta_ns = max(
        threshold_ns + noise_floor_ns,
        statistical_margin_ns if statistical_margin_ns is not None else 0.0,
    )
    status = "regression" if delta_ns > allowed_delta_ns else "passed"
    result = {
        "benchmark": name,
        "status": status,
        "baseline": baseline,
        "current": current,
        "delta_ns": delta_ns,
        "regression_percent": regression_percent,
        "threshold_ns": threshold_ns,
        "noise_floor_ns": noise_floor_ns,
        "statistical_margin_ns": statistical_margin_ns,
        "allowed_delta_ns": allowed_delta_ns,
    }
    if status == "regression":
        result["error"] = (
            f"regression {regression_percent:.3f}% exceeds allowed delta "
            f"{allowed_delta_ns:.3f} ns"
        )
    return result


def compare_snapshots(
    baseline: dict[str, Any], current: dict[str, Any], policy: dict[str, Any]
) -> dict[str, Any]:
    results = [
        _comparison_record(name, baseline.get(name), current.get(name), policy)
        for name in sorted(set(baseline) | set(current))
    ]
    counts: dict[str, int] = {}
    for result in results:
        status = result["status"]
        counts[status] = counts.get(status, 0) + 1

    hard_failures = {
        "regression",
        "missing_current",
        "insufficient_samples",
        "missing_baseline",
    }
    failures = [result for result in results if result["status"] in hard_failures]
    compared = sum(counts.get(status, 0) for status in ("passed", "regression"))
    if compared == 0 and not failures:
        failures.append(
            {
                "benchmark": None,
                "status": "no_comparisons",
                "error": "no benchmarks were compared",
            }
        )
    warnings = [result for result in results if result["status"] == "new_benchmark"]
    return {
        "results": results,
        "summary": {
            "status": "passed" if not failures else "failed",
            "benchmark_count": len(results),
            "compared_count": compared,
            "failure_count": len(failures),
            "warning_count": len(warnings),
            "status_counts": counts,
            "failures": failures,
            "warnings": warnings,
        },
    }


def comparison_document(
    baseline_path: Path,
    baseline: dict[str, Any],
    baseline_kind: str,
    baseline_errors: list[str],
    current_path: Path,
    current: dict[str, Any],
    current_kind: str,
    current_errors: list[str],
    policy: dict[str, Any],
) -> dict[str, Any]:
    comparison = compare_snapshots(baseline, current, policy)
    source_errors = [*baseline_errors, *current_errors]
    if source_errors:
        comparison["summary"]["status"] = "failed"
        comparison["summary"]["failures"].extend(
            {"benchmark": None, "status": "invalid_source", "error": error}
            for error in source_errors
        )
        comparison["summary"]["failure_count"] += len(source_errors)
    return {
        "schema": COMPARISON_SCHEMA,
        "schema_version": SCHEMA_VERSION,
        "generated_at": utc_now(),
        "baseline": {
            "path": str(baseline_path.resolve()),
            "kind": baseline_kind,
            "benchmark_count": len(baseline),
        },
        "current": {
            "path": str(current_path.resolve()),
            "kind": current_kind,
            "benchmark_count": len(current),
        },
        "policy": policy,
        **comparison,
    }


def validate_policy(args: argparse.Namespace) -> dict[str, Any]:
    values = {
        "max_regression_percent": args.max_regression_percent,
        "noise_percent": args.noise_percent,
        "noise_sigmas": args.noise_sigmas,
    }
    for name, value in values.items():
        if not math.isfinite(value) or value < 0:
            raise ValueError(f"--{name.replace('_', '-')} must be finite and non-negative")
    if args.min_samples < 1:
        raise ValueError("--min-samples must be at least 1")
    return {
        **values,
        "min_samples": args.min_samples,
        "missing_baseline": args.missing_baseline,
    }


def snapshot_command(args: argparse.Namespace) -> int:
    document = make_snapshot(args.criterion)
    write_json(args.output, document)
    print(
        f"bench-compare: wrote {len(document['benchmarks'])} benchmarks to {args.output}",
        flush=True,
    )
    return 0


def compare_command(args: argparse.Namespace) -> int:
    policy = validate_policy(args)
    baseline, baseline_errors, baseline_kind = load_source(args.baseline)
    current, current_errors, current_kind = load_source(args.current)
    document = comparison_document(
        args.baseline,
        baseline,
        baseline_kind,
        baseline_errors,
        args.current,
        current,
        current_kind,
        current_errors,
        policy,
    )
    write_json(args.output, document)
    summary = document["summary"]
    print(
        f"bench-compare: {summary['status']} "
        f"({summary['compared_count']} compared, {summary['failure_count']} failures, "
        f"{summary['warning_count']} warnings); report: {args.output}",
        flush=True,
    )
    for failure in summary["failures"]:
        print(
            f"bench-compare: FAIL {failure.get('benchmark') or '<source>'}: "
            f"{failure.get('error', failure['status'])}",
            file=sys.stderr,
            flush=True,
        )
    return 0 if summary["status"] == "passed" else 1


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    subparsers = parser.add_subparsers(dest="command", required=True)

    snapshot_parser = subparsers.add_parser(
        "snapshot", help="save Criterion estimates as a reusable JSON baseline"
    )
    snapshot_parser.add_argument("--criterion", type=Path, required=True)
    snapshot_parser.add_argument("--output", type=Path, required=True)
    snapshot_parser.set_defaults(func=snapshot_command)

    compare_parser = subparsers.add_parser(
        "compare", help="compare current Criterion data against a baseline"
    )
    compare_parser.add_argument("--baseline", type=Path, required=True)
    compare_parser.add_argument("--current", type=Path, required=True)
    compare_parser.add_argument("--output", type=Path, required=True)
    compare_parser.add_argument("--max-regression-percent", type=float, default=5.0)
    compare_parser.add_argument("--noise-percent", type=float, default=1.0)
    compare_parser.add_argument("--min-samples", type=int, default=10)
    compare_parser.add_argument("--noise-sigmas", type=float, default=3.0)
    compare_parser.add_argument(
        "--missing-baseline", choices=("fail", "warn"), default="fail"
    )
    compare_parser.set_defaults(func=compare_command)
    return parser


def main() -> int:
    try:
        args = build_parser().parse_args()
        return args.func(args)
    except (OSError, ValueError, TypeError, json.JSONDecodeError) as error:
        print(f"bench-compare: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
