#!/usr/bin/env python3
"""Evaluate NX-17 dataplane evidence against the predeclared policy."""

from __future__ import annotations

import argparse
import json
import math
import re
import sys
from datetime import datetime, timezone
from collections import Counter
from pathlib import Path
from typing import Any

from harness import (
    COMPARATORS,
    DEFAULT_POLICY,
    REPORT_SCHEMA,
    TESTS,
    median,
    read_json,
    sha256_file,
    write_json,
)

PING_TIME_RE = re.compile(r"time[=<]\s*([0-9]+(?:\.[0-9]+)?)\s*ms")
PING_SUMMARY_RE = re.compile(
    r"([0-9]+)\s+packets transmitted,\s+([0-9]+)\s+(?:packets\s+)?received"
)


def _read_jsonl(path: Path) -> list[dict[str, Any]]:
    if not path.exists():
        return []
    records: list[dict[str, Any]] = []
    for line_number, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
        if not line.strip():
            continue
        try:
            value = json.loads(line)
        except json.JSONDecodeError as error:
            raise ValueError(f"{path}:{line_number}: invalid JSON: {error}") from error
        if not isinstance(value, dict):
            raise ValueError(f"{path}:{line_number}: expected a JSON object")
        records.append(value)
    return records


def nearest_rank_percentile(values: list[float], percentile: float) -> float:
    if not values:
        raise ValueError("percentile requires at least one value")
    if not 0 < percentile <= 100:
        raise ValueError("percentile must be in (0, 100]")
    ordered = sorted(values)
    rank = max(1, math.ceil(percentile / 100.0 * len(ordered)))
    return float(ordered[rank - 1])


def parse_ping_file(path: Path, expected_samples: int) -> dict[str, Any]:
    if not path.exists():
        raise ValueError(f"latency probe output is missing: {path}")
    text = path.read_text(encoding="utf-8", errors="replace")
    rtts = [float(match.group(1)) for match in PING_TIME_RE.finditer(text)]
    summary = PING_SUMMARY_RE.search(text)
    if summary:
        transmitted = int(summary.group(1))
        received = int(summary.group(2))
    else:
        transmitted = expected_samples
        received = len(rtts)
    if expected_samples <= 0:
        raise ValueError("expected latency samples must be positive")
    effective_transmitted = max(expected_samples, transmitted)
    loss_percent = 100.0 * max(0, effective_transmitted - received) / effective_transmitted
    return {
        "p99_ms": nearest_rank_percentile(rtts, 99.0) if rtts else None,
        "samples": len(rtts),
        "received": received,
        "transmitted": transmitted,
        "expected_samples": expected_samples,
        "loss_percent": loss_percent,
    }


def _metric(record: dict[str, Any], name: str) -> float | None:
    value = record.get("metrics", {}).get(name)
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        return None
    number = float(value)
    return number if math.isfinite(number) else None


def _result_map(
    records: list[dict[str, Any]], implementations: list[str], runs: int
) -> tuple[dict[tuple[str, str, int], dict[str, Any]], list[str]]:
    expected = {
        (implementation, test, run)
        for implementation in implementations
        for test in TESTS
        for run in range(1, runs + 1)
    }
    counts = Counter(
        (record.get("impl"), record.get("test"), record.get("run")) for record in records
    )
    errors = []
    missing = sorted(expected - set(counts))
    unexpected = sorted(set(counts) - expected)
    duplicates = sorted(key for key, count in counts.items() if count > 1)
    if missing:
        errors.append(f"missing result rows: {missing}")
    if unexpected:
        errors.append(f"unexpected result rows: {unexpected}")
    if duplicates:
        errors.append(f"duplicate result rows: {duplicates}")
    return {record_key: record for record in records if (record_key := (record.get("impl"), record.get("test"), record.get("run"))) in expected}, errors


def _latency_map(
    output: Path,
    records: list[dict[str, Any]],
    implementations: list[str],
    runs: int,
) -> tuple[dict[tuple[str, str, int], dict[str, Any]], list[str]]:
    clients = sorted(
        (record for record in records if record.get("kind") == "client"),
        key=lambda record: int(record.get("started_ns", 0)),
    )
    errors: list[str] = []
    result: dict[tuple[str, str, int], dict[str, Any]] = {}
    index = 0
    for implementation in implementations:
        for test in TESTS:
            for run in range(1, runs + 1):
                if index >= len(clients):
                    errors.append(
                        "missing latency invocation for "
                        f"{implementation}/{test}/run {run}"
                    )
                    continue
                invocation = clients[index]
                index += 1
                hint = invocation.get("test_hint")
                if hint != test:
                    errors.append(
                        f"latency invocation order mismatch: expected {test}, got {hint}"
                    )
                relative_file = invocation.get("ping_file")
                expected_samples = int(invocation.get("expected_samples", 0))
                try:
                    if not isinstance(relative_file, str):
                        raise ValueError("ping_file is missing")
                    stats = parse_ping_file(output / relative_file, expected_samples)
                except (OSError, ValueError) as error:
                    errors.append(f"{implementation}/{test}/run {run}: {error}")
                    continue
                stats["probe_interval_ms"] = invocation.get("probe_interval_ms")
                stats["ping_returncode"] = invocation.get("ping_returncode")
                result[(implementation, test, run)] = stats
    if index != len(clients):
        errors.append(f"unexpected latency invocations: {len(clients) - index}")
    return result, errors


def _median_metric(
    result_map: dict[tuple[str, str, int], dict[str, Any]],
    implementation: str,
    test: str,
    runs: int,
    metric: str,
) -> float | None:
    values = [
        _metric(result_map[(implementation, test, run)], metric)
        for run in range(1, runs + 1)
    ]
    if any(value is None for value in values):
        return None
    return median(value for value in values if value is not None)


def _cpu_cost(
    result_map: dict[tuple[str, str, int], dict[str, Any]],
    implementation: str,
    test: str,
    runs: int,
) -> tuple[float | None, str]:
    is_udp = test.startswith("udp_")
    denominator_metric = "sent_packets" if is_udp else "transferred_bytes"
    label = "cpu_seconds_per_udp_packet" if is_udp else "cpu_seconds_per_byte"
    costs = []
    for run in range(1, runs + 1):
        record = result_map[(implementation, test, run)]
        cpu_seconds = _metric(record, "cpu_seconds")
        denominator = _metric(record, denominator_metric)
        if cpu_seconds is None or denominator is None or denominator <= 0:
            return None, label
        costs.append(cpu_seconds / denominator)
    return median(costs), label


def _latency_median(
    latency_map: dict[tuple[str, str, int], dict[str, Any]],
    implementation: str,
    test: str,
    runs: int,
) -> float | None:
    values = []
    for run in range(1, runs + 1):
        value = latency_map.get((implementation, test, run), {}).get("p99_ms")
        if not isinstance(value, (int, float)) or not math.isfinite(float(value)):
            return None
        values.append(float(value))
    return median(values)


def _latency_quality(
    latency_map: dict[tuple[str, str, int], dict[str, Any]],
    implementation: str,
    test: str,
    runs: int,
    policy: dict[str, Any],
) -> list[str]:
    errors = []
    minimum = int(policy["latency"]["min_samples_per_run"])
    maximum_loss = float(policy["latency"]["max_loss_percent"])
    for run in range(1, runs + 1):
        stats = latency_map.get((implementation, test, run), {})
        samples = stats.get("samples")
        loss = stats.get("loss_percent")
        if not isinstance(samples, int) or samples < minimum:
            errors.append(
                f"latency samples for {implementation}/{test}/run {run} are "
                f"{samples!r}, below {minimum}"
            )
        if not isinstance(loss, (int, float)) or float(loss) > maximum_loss:
            errors.append(
                f"latency probe loss for {implementation}/{test}/run {run} is "
                f"{loss!r}%, above {maximum_loss}%"
            )
    return errors


def _comparison(
    *,
    result_map: dict[tuple[str, str, int], dict[str, Any]],
    latency_map: dict[tuple[str, str, int], dict[str, Any]],
    comparator: str,
    test: str,
    runs: int,
    policy: dict[str, Any],
) -> dict[str, Any]:
    checks: dict[str, Any] = {}
    candidate_throughput = _median_metric(result_map, "fuckipv6", test, runs, "throughput_gbps")
    baseline_throughput = _median_metric(result_map, comparator, test, runs, "throughput_gbps")
    throughput_ratio = (
        candidate_throughput / baseline_throughput
        if candidate_throughput is not None
        and baseline_throughput is not None
        and baseline_throughput > 0
        else None
    )
    throughput_threshold = float(policy["throughput_min_ratio"])
    checks["throughput"] = {
        "candidate_gbps": candidate_throughput,
        "baseline_gbps": baseline_throughput,
        "ratio": throughput_ratio,
        "minimum_ratio": throughput_threshold,
        "passed": throughput_ratio is not None and throughput_ratio >= throughput_threshold,
    }

    candidate_cpu, cpu_label = _cpu_cost(result_map, "fuckipv6", test, runs)
    baseline_cpu, baseline_cpu_label = _cpu_cost(result_map, comparator, test, runs)
    cpu_ratio = (
        candidate_cpu / baseline_cpu
        if candidate_cpu is not None and baseline_cpu is not None and baseline_cpu > 0
        else None
    )
    cpu_threshold = float(policy["cpu_cost_max_ratio"])
    checks["cpu"] = {
        "metric": cpu_label,
        "baseline_metric": baseline_cpu_label,
        "candidate": candidate_cpu,
        "baseline": baseline_cpu,
        "ratio": cpu_ratio,
        "maximum_ratio": cpu_threshold,
        "passed": cpu_ratio is not None and cpu_ratio <= cpu_threshold,
    }

    gated_tests = set(policy["latency"]["gated_tests"])
    if test in gated_tests:
        candidate_p99 = _latency_median(latency_map, "fuckipv6", test, runs)
        baseline_p99 = _latency_median(latency_map, comparator, test, runs)
        p99_ratio = (
            candidate_p99 / baseline_p99
            if candidate_p99 is not None and baseline_p99 is not None and baseline_p99 > 0
            else None
        )
        quality_errors = _latency_quality(
            latency_map, "fuckipv6", test, runs, policy
        ) + _latency_quality(latency_map, comparator, test, runs, policy)
        p99_threshold = float(policy["p99_max_ratio"])
        checks["p99"] = {
            "candidate_ms": candidate_p99,
            "baseline_ms": baseline_p99,
            "ratio": p99_ratio,
            "maximum_ratio": p99_threshold,
            "quality_errors": quality_errors,
            "passed": p99_ratio is not None
            and p99_ratio <= p99_threshold
            and not quality_errors,
        }
    else:
        checks["p99"] = {"status": "not_gated", "reason": "saturated load test"}

    if test.startswith("udp_") and test.endswith("_controlled"):
        candidate_loss = _median_metric(result_map, "fuckipv6", test, runs, "lost_percent")
        baseline_loss = _median_metric(result_map, comparator, test, runs, "lost_percent")
        loss_threshold = float(policy["controlled_udp_max_loss_percent"])
        checks["packet_loss"] = {
            "candidate_percent": candidate_loss,
            "baseline_percent": baseline_loss,
            "maximum_percent": loss_threshold,
            "passed": candidate_loss is not None
            and baseline_loss is not None
            and candidate_loss <= loss_threshold
            and baseline_loss <= loss_threshold,
        }

    passed = all(
        check.get("passed", True)
        for check in checks.values()
        if check.get("status") != "not_gated"
    )
    return {
        "comparator": comparator,
        "test": test,
        "checks": checks,
        "passed": passed,
    }


def evaluate_output(output: Path) -> tuple[dict[str, Any], int]:
    plan_path = output / "plan.json"
    hash_path = output / "plan.sha256"
    result_path = output / "results.jsonl"
    environment_path = output / "environment.json"
    latency_log_path = output / "latency-invocations.jsonl"
    report_path = output / "gate-report.json"

    errors: list[str] = []
    warnings: list[str] = []
    plan: dict[str, Any] | None = None
    environment: dict[str, Any] | None = None

    try:
        plan = read_json(plan_path)
    except (OSError, json.JSONDecodeError) as error:
        errors.append(f"cannot read plan: {error}")
    try:
        environment = read_json(environment_path)
    except (OSError, json.JSONDecodeError) as error:
        errors.append(f"cannot read environment: {error}")

    if plan is not None:
        expected_hash = sha256_file(plan_path)
        try:
            recorded_hash = hash_path.read_text(encoding="utf-8").strip().split()[0]
        except (OSError, IndexError):
            recorded_hash = ""
        if recorded_hash != expected_hash:
            errors.append("plan hash mismatch; policy was changed after preflight")

        implementations = plan.get("implementations")
        runs = int(plan.get("policy", {}).get("required_runs", 0))
        if not isinstance(implementations, list) or not implementations:
            errors.append("plan has no selected implementations")
            implementations = []
        if runs < 1:
            errors.append("plan required_runs must be positive")
            runs = 0
        if "fuckipv6" not in implementations:
            errors.append("plan does not include fuckipv6")
        for comparator in COMPARATORS:
            if comparator not in implementations:
                warnings.append(f"required comparator was skipped: {comparator}")

        if environment is not None:
            if environment.get("kernel") != plan.get("expected_kernel"):
                errors.append("environment kernel does not match the pinned plan")
            quiet = environment.get("quiet_host", {})
            if quiet.get("passed") is not True:
                errors.append(f"host was not quiet at preflight: {quiet.get('failures')}")
            affinity = environment.get("cpu_affinity", {})
            planned_affinity = plan.get("cpu_affinity", {})
            for name in ("client", "server", "control"):
                if not affinity.get(name):
                    errors.append(f"CPU affinity {name!r} is not pinned")
                else:
                    planned_value = planned_affinity.get(name)
                    if isinstance(planned_value, list):
                        planned_value = ",".join(str(cpu) for cpu in planned_value)
                    if affinity.get(name) != planned_value:
                        errors.append(
                            f"CPU affinity {name!r} changed after preflight: "
                            f"{affinity.get(name)!r} != {planned_value!r}"
                        )

            planned_go = plan.get("wireguard_go")
            if "wireguard-go" in implementations and planned_go is not None:
                measured_go = environment.get("wireguard_go")
                if not isinstance(measured_go, dict):
                    errors.append("wireguard-go environment record is missing")
                else:
                    for field in ("path", "version", "sha256"):
                        if measured_go.get(field) != planned_go.get(field):
                            errors.append(
                                f"wireguard-go {field} changed after preflight: "
                                f"{measured_go.get(field)!r} != {planned_go.get(field)!r}"
                            )

        try:
            result_records = _read_jsonl(result_path)
            if not result_records:
                raise ValueError("results.jsonl is empty")
            result_map, map_errors = _result_map(result_records, implementations, runs)
            errors.extend(map_errors)
            latency_records = _read_jsonl(latency_log_path)
            latency_map, latency_errors = _latency_map(
                output, latency_records, implementations, runs
            )
            errors.extend(latency_errors)
        except (OSError, ValueError) as error:
            errors.append(f"cannot read measured results: {error}")
            result_map = {}
            latency_map = {}

        runner_error_path = output / "runner-error.txt"
        if runner_error_path.exists():
            runner_error = runner_error_path.read_text(encoding="utf-8", errors="replace").strip()
            if runner_error:
                errors.append(f"collection command failed: {runner_error}")

    comparisons = []
    policy = plan.get("policy", DEFAULT_POLICY) if plan is not None else DEFAULT_POLICY
    if plan is not None and not errors:
        implementations = plan["implementations"]
        runs = int(policy["required_runs"])
        for comparator in COMPARATORS:
            if comparator not in implementations:
                continue
            for test in TESTS:
                comparisons.append(
                    _comparison(
                        result_map=result_map,
                        latency_map=latency_map,
                        comparator=comparator,
                        test=test,
                        runs=runs,
                        policy=policy,
                    )
                )

    failed_checks = [
        f"{comparison['comparator']}/{comparison['test']}: "
        + ", ".join(
            name
            for name, check in comparison["checks"].items()
            if check.get("status") != "not_gated" and not check.get("passed", False)
        )
        for comparison in comparisons
        if not comparison["passed"]
    ]

    if errors or failed_checks:
        status = "failed"
        exit_code = 1
    elif warnings:
        status = "incomplete"
        exit_code = 2
    else:
        status = "passed"
        exit_code = 0

    report = {
        "schema": REPORT_SCHEMA,
        "evaluated_utc": datetime.now(timezone.utc).isoformat(),
        "status": status,
        "exit_code": exit_code,
        "policy": policy,
        "comparisons": comparisons,
        "errors": errors,
        "warnings": warnings,
        "failed_checks": failed_checks,
        "claim": "No performance superiority claim is implied by this report.",
    }
    write_json(report_path, report)
    return report, exit_code


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True, help="NX-17 output directory")
    return parser


def main() -> int:
    args = build_parser().parse_args()
    try:
        report, exit_code = evaluate_output(args.output)
    except (OSError, ValueError) as error:
        print(f"benchmark-gate: {error}", file=sys.stderr)
        return 1
    print(f"NX-17 gate: {report['status'].upper()} (exit {exit_code})")
    for error in report["errors"]:
        print(f"  ERROR: {error}", file=sys.stderr)
    for warning in report["warnings"]:
        print(f"  INCOMPLETE: {warning}", file=sys.stderr)
    for failure in report["failed_checks"]:
        print(f"  THRESHOLD FAILURE: {failure}", file=sys.stderr)
    print(f"Report: {args.output / 'gate-report.json'}")
    return exit_code


if __name__ == "__main__":
    raise SystemExit(main())
