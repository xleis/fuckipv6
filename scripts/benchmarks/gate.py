#!/usr/bin/env python3
"""Evaluate NX-17 dataplane evidence against the predeclared policy."""

from __future__ import annotations

import argparse
import json
import math
import re
import sys
from collections import Counter
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

from harness import (
    CANONICAL_POLICY_ID,
    CANONICAL_POLICY_SHA256,
    COMPARATORS,
    DEFAULT_POLICY,
    ENVIRONMENT_SCHEMA,
    EVIDENCE_MANIFEST_SCHEMA,
    IMPLEMENTATIONS,
    LATENCY_SCHEMA,
    PLAN_SCHEMA,
    POLICY_SCHEMA,
    REPORT_SCHEMA,
    SOURCE_SCHEMA,
    TESTS,
    median,
    policy_sha256,
    read_json,
    sha256_file,
    write_json,
)

PING_TIME_RE = re.compile(r"time[=<]\s*([0-9]+(?:\.[0-9]+)?)\s*ms")
PING_SUMMARY_RE = re.compile(
    r"([0-9]+)\s+packets transmitted,\s+([0-9]+)\s+(?:packets\s+)?received"
)
EVIDENCE_LINE_RE = re.compile(r"([0-9a-f]{64})  (.+)")
SHA256_RE = re.compile(r"[0-9a-f]{64}")
GIT_OBJECT_RE = re.compile(r"(?:[0-9a-f]{40}|[0-9a-f]{64})")


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
    if not path.is_file():
        raise ValueError(f"latency probe output is missing: {path}")
    if isinstance(expected_samples, bool) or expected_samples <= 0:
        raise ValueError("expected latency samples must be positive")
    text = path.read_text(encoding="utf-8", errors="replace")
    rtts = [float(match.group(1)) for match in PING_TIME_RE.finditer(text)]
    summary = PING_SUMMARY_RE.search(text)
    if summary is None:
        raise ValueError("latency probe output has no packet summary")
    transmitted = int(summary.group(1))
    received = int(summary.group(2))
    if transmitted <= 0:
        raise ValueError("latency probe transmitted no packets")
    if received < 0 or received > transmitted:
        raise ValueError(
            f"latency probe received count is invalid: {received}/{transmitted}"
        )
    if received != len(rtts):
        raise ValueError(
            f"latency probe RTT count {len(rtts)} does not match received packets {received}"
        )
    if any(not math.isfinite(rtt) or rtt < 0 for rtt in rtts):
        raise ValueError("latency probe contains an invalid RTT")
    effective_transmitted = max(expected_samples, transmitted)
    loss_percent = 100.0 * max(0, effective_transmitted - received) / effective_transmitted
    if not 0.0 <= loss_percent <= 100.0:
        raise ValueError(f"latency probe loss is outside 0..100%: {loss_percent}")
    return {
        "p99_ms": nearest_rank_percentile(rtts, 99.0) if rtts else None,
        "samples": len(rtts),
        "rtt_samples": len(rtts),
        "received": received,
        "transmitted": transmitted,
        "expected_samples": expected_samples,
        "loss_percent": loss_percent,
    }


def _metric(record: dict[str, Any], name: str) -> float | None:
    metrics = record.get("metrics")
    if not isinstance(metrics, dict):
        return None
    value = metrics.get(name)
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        return None
    number = float(value)
    return number if math.isfinite(number) else None


def _required_number(
    value: Any,
    label: str,
    *,
    positive: bool = False,
    minimum: float | None = None,
    maximum: float | None = None,
) -> float:
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        raise ValueError(f"{label} is missing or not numeric")
    number = float(value)
    if not math.isfinite(number):
        raise ValueError(f"{label} must be finite")
    if positive and number <= 0:
        raise ValueError(f"{label} must be positive")
    if minimum is not None and number < minimum:
        raise ValueError(f"{label} must be at least {minimum}")
    if maximum is not None and number > maximum:
        raise ValueError(f"{label} must be at most {maximum}")
    return number


def _required_integer(value: Any, label: str, *, positive: bool = True) -> int:
    number = _required_number(value, label, positive=positive)
    integer = int(number)
    if number != integer:
        raise ValueError(f"{label} must be an integer")
    return integer


def _parse_task_clock_seconds(path: Path) -> float:
    if not path.is_file():
        raise ValueError(f"raw perf evidence is missing: {path}")
    factors = {
        "ns": 1e-9,
        "us": 1e-6,
        "usec": 1e-6,
        "ms": 1e-3,
        "msec": 1e-3,
        "s": 1.0,
        "sec": 1.0,
    }
    task_clock = None
    for line_number, line in enumerate(
        path.read_text(encoding="utf-8", errors="replace").splitlines(), 1
    ):
        fields = line.split(",")
        if len(fields) < 3 or fields[2] != "task-clock":
            continue
        try:
            value = float(fields[0])
        except ValueError as error:
            raise ValueError(
                f"invalid perf task-clock at {path}:{line_number}: {fields[0]!r}"
            ) from error
        value = _required_number(value, f"perf task-clock at {path}:{line_number}")
        factor = factors.get(fields[1])
        if factor is None:
            raise ValueError(f"unsupported perf task-clock unit {fields[1]!r}")
        if task_clock is not None:
            raise ValueError(f"duplicate perf task-clock counter in {path}")
        task_clock = value * factor
    if task_clock is None:
        raise ValueError(f"raw perf evidence has no task-clock counter: {path}")
    return _required_number(task_clock, "perf task-clock seconds", positive=True)


def _recompute_raw_metrics(iperf_path: Path, perf_path: Path, test: str) -> dict[str, Any]:
    try:
        document = read_json(iperf_path)
    except OSError as error:
        raise ValueError(f"raw iperf3 evidence is missing: {iperf_path}") from error
    if not isinstance(document, dict):
        raise ValueError(f"raw iperf3 evidence is not an object: {iperf_path}")
    if document.get("error") is not None:
        raise ValueError(f"raw iperf3 evidence contains an error: {document.get('error')}")
    end = document.get("end")
    if not isinstance(end, dict):
        raise ValueError(f"raw iperf3 evidence has no end summary: {iperf_path}")
    sent_summary = end.get("sum_sent")
    received_summary = end.get("sum_received") or end.get("sum")
    if not isinstance(sent_summary, dict) or not isinstance(received_summary, dict):
        raise ValueError(f"raw iperf3 evidence has incomplete summaries: {iperf_path}")

    throughput_bps = _required_number(
        received_summary.get("bits_per_second"), "raw throughput", positive=True
    )
    offered_bps = _required_number(
        sent_summary.get("bits_per_second"), "raw offered throughput", positive=True
    )
    transferred_bytes = _required_integer(
        received_summary.get("bytes"), "raw transferred bytes"
    )
    sent_bytes = _required_integer(sent_summary.get("bytes"), "raw sent bytes")
    elapsed_seconds = _required_number(
        received_summary.get("seconds"), "raw elapsed seconds", positive=True
    )
    cpu_seconds = _parse_task_clock_seconds(perf_path)

    sent_packets = None
    lost_percent = None
    if test.startswith("udp_"):
        sent_packets = _required_integer(sent_summary.get("packets"), "raw sent packets")
        lost_percent = _required_number(
            received_summary.get("lost_percent"),
            "raw lost percent",
            minimum=0.0,
            maximum=100.0,
        )

    return {
        "impl": None,
        "test": test,
        "run": None,
        "metrics": {
            "throughput_gbps": throughput_bps / 1_000_000_000.0,
            "offered_throughput_gbps": offered_bps / 1_000_000_000.0,
            "transferred_bytes": transferred_bytes,
            "sent_bytes": sent_bytes,
            "sent_packets": sent_packets,
            "lost_percent": lost_percent,
            "elapsed_seconds": elapsed_seconds,
            "cpu_seconds": cpu_seconds,
        },
        "raw_iperf": str(iperf_path),
        "raw_perf": str(perf_path),
    }


def _numbers_match(reported: float | None, recomputed: float, label: str) -> bool:
    if reported is None:
        return False
    tolerance = max(1e-9, abs(recomputed) * 1e-9)
    return abs(reported - recomputed) <= tolerance


def _recompute_results(
    output: Path,
    result_map: dict[tuple[str, str, int], dict[str, Any]],
) -> tuple[dict[tuple[str, str, int], dict[str, Any]], list[str]]:
    recomputed: dict[tuple[str, str, int], dict[str, Any]] = {}
    errors: list[str] = []
    for key, reported in result_map.items():
        implementation, test, run = key
        raw_stem = f"{implementation}__{test}__run{run}"
        try:
            actual = _recompute_raw_metrics(
                output / "raw" / f"{raw_stem}.json",
                output / "raw" / f"{raw_stem}.perf.csv",
                test,
            )
        except (OSError, ValueError, json.JSONDecodeError) as error:
            errors.append(f"{implementation}/{test}/run {run}: {error}")
            continue
        actual["impl"] = implementation
        actual["run"] = run

        try:
            _required_number(
                _metric(reported, "cpu_seconds"),
                f"{implementation}/{test}/run {run} reported cpu_seconds",
                positive=True,
            )
        except ValueError as error:
            errors.append(str(error))

        comparisons = (
            ("throughput_gbps", False),
            ("offered_throughput_gbps", False),
            ("transferred_bytes", True),
            ("sent_bytes", True),
            ("sent_packets", True),
            ("lost_percent", False),
            ("elapsed_seconds", False),
            ("perf_task_clock_seconds", False),
        )
        for metric_name, exact in comparisons:
            reported_value = _metric(reported, metric_name)
            actual_value = actual["metrics"].get(metric_name)
            if metric_name == "perf_task_clock_seconds":
                reported_value = _metric(reported, "perf_task_clock_seconds")
                actual_value = actual["metrics"]["cpu_seconds"]
            if actual_value is None:
                if reported_value is not None:
                    errors.append(
                        f"{implementation}/{test}/run {run}: {metric_name} is unexpectedly reported"
                    )
                continue
            matches = (
                reported_value is not None
                and math.isclose(reported_value, float(actual_value), rel_tol=0.0, abs_tol=1e-9)
                if exact
                else _numbers_match(reported_value, float(actual_value), metric_name)
            )
            if not matches:
                errors.append(
                    f"{implementation}/{test}/run {run}: reported {metric_name} "
                    f"{reported_value!r} does not match raw evidence {actual_value!r}"
                )
        recomputed[key] = actual
    return recomputed, errors


def _validate_evidence_manifest(output: Path) -> list[str]:
    manifest_path = output / "evidence.sha256"
    if not manifest_path.is_file():
        return ["evidence.sha256 is missing"]
    try:
        lines = manifest_path.read_text(encoding="utf-8").splitlines()
    except OSError as error:
        return [f"cannot read evidence.sha256: {error}"]
    if not lines or lines[0] != EVIDENCE_MANIFEST_SCHEMA:
        return [f"evidence.sha256 schema must be {EVIDENCE_MANIFEST_SCHEMA}"]

    errors: list[str] = []
    entries: dict[str, str] = {}
    for line_number, line in enumerate(lines[1:], 2):
        if not line:
            continue
        match = EVIDENCE_LINE_RE.fullmatch(line)
        if match is None:
            errors.append(f"evidence.sha256:{line_number}: malformed entry")
            continue
        digest, relative = match.groups()
        path = Path(relative)
        if path.is_absolute() or ".." in path.parts:
            errors.append(f"evidence.sha256:{line_number}: unsafe path {relative!r}")
            continue
        if relative in entries:
            errors.append(f"evidence.sha256:{line_number}: duplicate path {relative!r}")
            continue
        entries[relative] = digest

    required = {
        "plan.json",
        "plan.sha256",
        "environment.json",
        "results.jsonl",
        "latency-invocations.jsonl",
    }
    missing = sorted(required - entries.keys())
    if missing:
        errors.append(f"evidence manifest omits required files: {missing}")
    for relative, expected in sorted(entries.items()):
        path = output / relative
        if not path.is_file():
            errors.append(f"evidence file is missing: {relative}")
            continue
        try:
            actual = sha256_file(path)
        except OSError as error:
            errors.append(f"cannot hash evidence file {relative}: {error}")
            continue
        if actual != expected:
            errors.append(f"evidence hash mismatch: {relative}")

    excluded = {"evidence.sha256", "gate-report.json"}
    actual_files = {
        path.relative_to(output).as_posix()
        for path in output.rglob("*")
        if path.is_file() and path.relative_to(output).as_posix() not in excluded
    }
    unmanifested = sorted(actual_files - entries.keys())
    if unmanifested:
        errors.append(f"evidence manifest omits files: {unmanifested}")
    return errors


def _validate_binary(record: Any, label: str, errors: list[str]) -> None:
    if not isinstance(record, dict):
        errors.append(f"binary record is missing: {label}")
        return
    path_value = record.get("path")
    digest = record.get("sha256")
    if not isinstance(path_value, str) or not Path(path_value).is_absolute():
        errors.append(f"binary path is missing or not absolute: {label}")
        return
    if not isinstance(digest, str) or SHA256_RE.fullmatch(digest) is None:
        errors.append(f"binary SHA-256 is invalid: {label}")
        return
    path = Path(path_value)
    if not path.is_file():
        errors.append(f"binary is missing: {label}: {path}")
        return
    try:
        actual_digest = sha256_file(path)
    except OSError as error:
        errors.append(f"cannot hash binary {label}: {error}")
        return
    if actual_digest != digest:
        errors.append(f"binary SHA-256 changed after preflight: {label}")


def _validate_plan_identity(
    plan: dict[str, Any], implementations: list[str], errors: list[str]
) -> None:
    if plan.get("schema") != PLAN_SCHEMA:
        errors.append(f"plan schema must be {PLAN_SCHEMA}")
    if plan.get("evidence") is not True:
        errors.append("plan evidence flag must be true for a measured run")

    policy = plan.get("policy")
    if not isinstance(policy, dict):
        errors.append("plan policy is missing")
    else:
        if policy.get("schema") != POLICY_SCHEMA:
            errors.append(f"policy schema must be {POLICY_SCHEMA}")
        digest = policy_sha256(policy)
        if digest != CANONICAL_POLICY_SHA256:
            errors.append(
                "policy is not the canonical NX-17 policy; custom or weakened "
                "thresholds cannot produce a passing report"
            )
        if plan.get("policy_sha256") != digest:
            errors.append("plan policy SHA-256 does not match the policy object")
        if plan.get("policy_id") != (
            CANONICAL_POLICY_ID if digest == CANONICAL_POLICY_SHA256 else "custom"
        ):
            errors.append("plan policy id does not match the policy object")
        if plan.get("policy_canonical") is not (digest == CANONICAL_POLICY_SHA256):
            errors.append("plan canonical policy flag does not match the policy object")
        if plan.get("canonical_policy_id") != CANONICAL_POLICY_ID:
            errors.append("plan does not identify the canonical policy id")
        _validate_plan_execution(plan, policy, errors)

    source = plan.get("source")
    if not isinstance(source, dict):
        errors.append("source provenance is missing")
    else:
        if source.get("schema") != SOURCE_SCHEMA:
            errors.append(f"source schema must be {SOURCE_SCHEMA}")
        for field in ("git_commit", "git_tree"):
            value = source.get(field)
            if not isinstance(value, str) or GIT_OBJECT_RE.fullmatch(value) is None:
                errors.append(f"source {field} is not a full git object id")
        if source.get("git_status") != "" or source.get("dirty") is not False:
            errors.append("source tree was dirty when the evidence plan was created")

    binaries = plan.get("binaries")
    if not isinstance(binaries, dict):
        errors.append("binary provenance is missing")
        return
    for name in ("fuckipv6", "coordinator"):
        _validate_binary(binaries.get(name), name, errors)
    if "wireguard-go" in implementations:
        _validate_binary(binaries.get("wireguard-go"), "wireguard-go", errors)
    tools = binaries.get("tools")
    if not isinstance(tools, dict):
        errors.append("tool binary provenance is missing")
        return
    for name in ("ip", "iperf3", "perf", "ping", "python3", "taskset", "wg", "git"):
        _validate_binary(tools.get(name), f"tool:{name}", errors)


def _plan_value_matches(actual: Any, expected: Any) -> bool:
    if isinstance(expected, bool):
        return actual is expected
    if isinstance(expected, (int, float)):
        return (
            isinstance(actual, (int, float))
            and not isinstance(actual, bool)
            and math.isfinite(float(actual))
            and float(actual) == float(expected)
        )
    return actual == expected


def _validate_plan_value(
    actual: Any, expected: Any, label: str, errors: list[str]
) -> None:
    if not _plan_value_matches(actual, expected):
        errors.append(
            f"plan {label} does not match the canonical policy: "
            f"{actual!r} != {expected!r}"
        )


def _validate_plan_execution(
    plan: dict[str, Any], policy: dict[str, Any], errors: list[str]
) -> None:
    """Require the recorded execution parameters to match the hashed policy."""
    traffic = plan.get("traffic")
    policy_traffic = policy.get("traffic")
    if not isinstance(traffic, dict):
        errors.append("plan traffic configuration is missing")
    elif not isinstance(policy_traffic, dict):
        errors.append("policy traffic configuration is missing")
    else:
        _validate_plan_value(
            traffic.get("runs"),
            policy.get("required_runs"),
            "traffic.runs",
            errors,
        )
        for field in ("duration_seconds", "omit_seconds", "mtu"):
            _validate_plan_value(
                traffic.get(field),
                policy_traffic.get(field),
                f"traffic.{field}",
                errors,
            )
        traffic_models = policy.get("traffic_models")
        if not isinstance(traffic_models, list):
            traffic_models = []
        controlled_rates = {
            model.get("name"): model.get("controlled_rate")
            for model in traffic_models
            if isinstance(model, dict) and model.get("load") == "controlled"
        }
        _validate_plan_value(
            traffic.get("udp_1420_controlled_rate"),
            controlled_rates.get("udp_1420_controlled"),
            "traffic.udp_1420_controlled_rate",
            errors,
        )
        _validate_plan_value(
            traffic.get("udp_64_controlled_rate"),
            controlled_rates.get("udp_64_controlled"),
            "traffic.udp_64_controlled_rate",
            errors,
        )

    environment_limits = plan.get("environment_limits")
    policy_environment = policy.get("environment")
    if not isinstance(environment_limits, dict):
        errors.append("plan environment limits are missing")
    elif not isinstance(policy_environment, dict):
        errors.append("policy environment limits are missing")
    else:
        for field in (
            "load1_max",
            "cpu_busy_percent_max",
            "require_performance_governor",
        ):
            _validate_plan_value(
                environment_limits.get(field),
                policy_environment.get(field),
                f"environment_limits.{field}",
                errors,
            )

    latency_probe = plan.get("latency_probe")
    policy_latency = policy.get("latency")
    if not isinstance(latency_probe, dict):
        errors.append("plan latency probe configuration is missing")
    elif not isinstance(policy_latency, dict):
        errors.append("policy latency configuration is missing")
    else:
        _validate_plan_value(
            latency_probe.get("interval_ms"),
            policy_latency.get("probe_interval_ms"),
            "latency_probe.interval_ms",
            errors,
        )
        _validate_plan_value(
            latency_probe.get("timeout_seconds"),
            policy_latency.get("probe_timeout_seconds"),
            "latency_probe.timeout_seconds",
            errors,
        )


def _result_map(
    records: list[dict[str, Any]], implementations: list[str], runs: int
) -> tuple[dict[tuple[str, str, int], dict[str, Any]], list[str]]:
    expected = {
        (implementation, test, run)
        for implementation in implementations
        for test in TESTS
        for run in range(1, runs + 1)
    }
    counts: Counter[tuple[str, str, int]] = Counter()
    errors = []
    malformed_rows = []
    for line_number, record in enumerate(records, 1):
        implementation = record.get("impl")
        test = record.get("test")
        run = record.get("run")
        if (
            not isinstance(implementation, str)
            or not isinstance(test, str)
            or isinstance(run, bool)
            or not isinstance(run, int)
        ):
            malformed_rows.append(line_number)
            continue
        counts[(implementation, test, run)] += 1
    if malformed_rows:
        errors.append(f"malformed result rows at JSONL lines: {malformed_rows}")
    missing = sorted(expected - set(counts))
    unexpected = sorted(set(counts) - expected)
    duplicates = sorted(key for key, count in counts.items() if count > 1)
    if missing:
        errors.append(f"missing result rows: {missing}")
    if unexpected:
        errors.append(f"unexpected result rows: {unexpected}")
    if duplicates:
        errors.append(f"duplicate result rows: {duplicates}")
    valid_records = {}
    for record in records:
        implementation = record.get("impl")
        test = record.get("test")
        run = record.get("run")
        if (
            not isinstance(implementation, str)
            or not isinstance(test, str)
            or isinstance(run, bool)
            or not isinstance(run, int)
        ):
            continue
        record_key = (implementation, test, run)
        if record_key in expected:
            valid_records[record_key] = record
    return valid_records, errors


def _latency_map(
    output: Path,
    records: list[dict[str, Any]],
    implementations: list[str],
    runs: int,
) -> tuple[dict[tuple[str, str, int], dict[str, Any]], list[str]]:
    def started_key(record: dict[str, Any]) -> int:
        value = record.get("started_ns")
        if isinstance(value, bool) or not isinstance(value, (int, float)):
            return -1
        return int(value) if math.isfinite(float(value)) else -1

    clients = sorted(
        (record for record in records if record.get("kind") == "client"),
        key=started_key,
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
                started_ns = invocation.get("started_ns")
                if (
                    isinstance(started_ns, bool)
                    or not isinstance(started_ns, (int, float))
                    or not math.isfinite(float(started_ns))
                    or float(started_ns) <= 0
                ):
                    errors.append(
                        f"latency invocation timestamp is invalid for "
                        f"{implementation}/{test}/run {run}: {started_ns!r}"
                    )
                if invocation.get("schema") != LATENCY_SCHEMA:
                    errors.append(
                        f"latency invocation schema mismatch for {implementation}/{test}/run {run}"
                    )
                if hint != test:
                    errors.append(
                        f"latency invocation order mismatch: expected {test}, got {hint}"
                    )
                relative_file = invocation.get("ping_file")
                expected_samples = int(invocation.get("expected_samples", 0))
                try:
                    if not isinstance(relative_file, str):
                        raise ValueError("ping_file is missing")
                    relative_path = Path(relative_file)
                    if relative_path.is_absolute() or ".." in relative_path.parts:
                        raise ValueError(f"ping_file escapes the evidence root: {relative_file}")
                    evidence_root = output.resolve()
                    ping_path = (output / relative_path).resolve()
                    try:
                        ping_path.relative_to(evidence_root)
                    except ValueError as error:
                        raise ValueError(
                            f"ping_file escapes the evidence root: {relative_file}"
                        ) from error
                    stats = parse_ping_file(ping_path, expected_samples)
                except (OSError, TypeError, ValueError) as error:
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
        if isinstance(samples, bool) or not isinstance(samples, int) or samples < minimum:
            errors.append(
                f"latency samples for {implementation}/{test}/run {run} are "
                f"{samples!r}, below {minimum}"
            )
        if (
            isinstance(loss, bool)
            or not isinstance(loss, (int, float))
            or not math.isfinite(float(loss))
            or not 0.0 <= float(loss) <= 100.0
            or float(loss) > maximum_loss
        ):
            errors.append(
                f"latency probe loss for {implementation}/{test}/run {run} is "
                f"{loss!r}%, above {maximum_loss}%"
            )
        if stats.get("ping_returncode") != 0:
            errors.append(
                f"latency probe for {implementation}/{test}/run {run} exited "
                f"with status {stats.get('ping_returncode')!r}"
            )
        probe_interval = stats.get("probe_interval_ms")
        expected_interval = float(policy["latency"]["probe_interval_ms"])
        if (
            isinstance(probe_interval, bool)
            or not isinstance(probe_interval, (int, float))
            or not math.isclose(
                float(probe_interval), expected_interval, rel_tol=0.0, abs_tol=1e-9
            )
        ):
            errors.append(
                f"latency probe interval for {implementation}/{test}/run {run} is "
                f"{probe_interval!r}ms, expected {expected_interval}ms"
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
        candidate_losses = [
            _metric(result_map[("fuckipv6", test, run)], "lost_percent")
            for run in range(1, runs + 1)
        ]
        baseline_losses = [
            _metric(result_map[(comparator, test, run)], "lost_percent")
            for run in range(1, runs + 1)
        ]
        loss_threshold = float(policy["controlled_udp_max_loss_percent"])
        checks["packet_loss"] = {
            "candidate_per_run_percent": candidate_losses,
            "baseline_per_run_percent": baseline_losses,
            "maximum_percent": loss_threshold,
            "passed": all(
                loss is not None and 0.0 <= loss <= loss_threshold
                for loss in candidate_losses + baseline_losses
            ),
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
    result_map: dict[tuple[str, str, int], dict[str, Any]] = {}
    latency_map: dict[tuple[str, str, int], dict[str, Any]] = {}

    errors.extend(_validate_evidence_manifest(output))

    try:
        plan = read_json(plan_path)
    except (OSError, json.JSONDecodeError) as error:
        errors.append(f"cannot read plan: {error}")
    if plan is not None and not isinstance(plan, dict):
        errors.append("plan must be a JSON object")
        plan = None
    try:
        environment = read_json(environment_path)
    except (OSError, json.JSONDecodeError) as error:
        errors.append(f"cannot read environment: {error}")
    if environment is not None and not isinstance(environment, dict):
        errors.append("environment must be a JSON object")
        environment = None

    if plan is not None:
        try:
            expected_hash = sha256_file(plan_path)
        except OSError as error:
            expected_hash = ""
            errors.append(f"cannot hash plan: {error}")
        try:
            recorded_hash = hash_path.read_text(encoding="utf-8").strip()
        except OSError:
            recorded_hash = ""
        if SHA256_RE.fullmatch(recorded_hash) is None:
            errors.append("plan.sha256 must contain exactly one lowercase SHA-256 digest")
        elif recorded_hash != expected_hash:
            errors.append("plan hash mismatch; policy was changed after preflight")

        policy_value = plan.get("policy")
        policy = policy_value if isinstance(policy_value, dict) else DEFAULT_POLICY
        implementations_value = plan.get("implementations")
        implementations = implementations_value if isinstance(implementations_value, list) else []
        if not implementations:
            errors.append("plan has no selected implementations")
        invalid_implementations = [
            value for value in implementations if not isinstance(value, str)
        ]
        if invalid_implementations:
            errors.append("plan implementations must be strings")
            implementations = [
                value for value in implementations if isinstance(value, str)
            ]
        if len(implementations) != len(set(implementations)):
            errors.append("plan implementations contain duplicates")
        unknown_implementations = sorted(set(implementations) - set(IMPLEMENTATIONS))
        if unknown_implementations:
            errors.append(f"plan contains unsupported implementations: {unknown_implementations}")
        try:
            runs = int(policy.get("required_runs", 0))
        except (TypeError, ValueError):
            runs = 0
        if runs < 1:
            errors.append("plan required_runs must be positive")
            runs = 0
        if "fuckipv6" not in implementations:
            errors.append("plan does not include fuckipv6")
        for comparator in COMPARATORS:
            if comparator not in implementations:
                warnings.append(f"required comparator was skipped: {comparator}")
        _validate_plan_identity(plan, implementations, errors)

        if environment is not None:
            if environment.get("schema") != ENVIRONMENT_SCHEMA:
                errors.append(f"environment schema must be {ENVIRONMENT_SCHEMA}")
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
            result_map, raw_errors = _recompute_results(output, result_map)
            errors.extend(raw_errors)
            latency_records = _read_jsonl(latency_log_path)
            latency_map, latency_errors = _latency_map(
                output, latency_records, implementations, runs
            )
            errors.extend(latency_errors)
        except (OSError, ValueError) as error:
            errors.append(f"cannot read measured results: {error}")

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
        "policy_metadata": {
            "id": plan.get("policy_id") if plan is not None else None,
            "sha256": policy_sha256(policy),
            "canonical": policy_sha256(policy) == CANONICAL_POLICY_SHA256,
            "canonical_id": CANONICAL_POLICY_ID,
            "canonical_sha256": CANONICAL_POLICY_SHA256,
        },
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
