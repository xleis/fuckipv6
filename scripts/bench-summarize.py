#!/usr/bin/env python3
"""Record and aggregate raw network benchmark results without third-party modules."""

from __future__ import annotations

import argparse
import csv
import json
import math
import statistics
import sys
from pathlib import Path
from typing import Any


def _as_float(value: Any) -> float | None:
    try:
        result = float(value)
    except (TypeError, ValueError):
        return None
    return result if math.isfinite(result) else None


def _as_int(value: Any) -> int | None:
    number = _as_float(value)
    return int(number) if number is not None else None


def _iperf_summary(document: dict[str, Any], test: str) -> dict[str, Any]:
    end = document.get("end", {})
    if test.startswith("udp_"):
        # end.sum is the sender aggregate on UDP. Only sum_received contains the
        # receiver's delivery counters, so never substitute sender bytes there.
        summary = end.get("sum_received")
    else:
        summary = end.get("sum_received") or end.get("sum") or end.get("sum_sent")
    if not isinstance(summary, dict):
        error = document.get("error") or "iperf3 output has no result summary"
        raise ValueError(str(error))
    return summary


def _iperf_sent_summary(document: dict[str, Any]) -> dict[str, Any]:
    sent = document.get("end", {}).get("sum_sent")
    return sent if isinstance(sent, dict) else {}


def _time_factor(unit: str) -> float:
    return {
        "ns": 1e-9,
        "us": 1e-6,
        "usec": 1e-6,
        "ms": 1e-3,
        "msec": 1e-3,
        "s": 1.0,
        "sec": 1.0,
    }.get(unit, 1.0)


def parse_perf(path: Path | None) -> dict[str, float]:
    """Parse the CSV emitted by perf stat -x, for the events used by the runner."""
    if path is None or not path.exists():
        return {}

    counters: dict[str, float] = {}
    for line in path.read_text(encoding="utf-8", errors="replace").splitlines():
        fields = line.split(",")
        if len(fields) < 3:
            continue
        value = _as_float(fields[0])
        if value is None:
            continue
        unit = fields[1]
        event = fields[2]
        if event == "task-clock":
            counters["task_clock_seconds"] = value * _time_factor(unit)
        elif event == "cycles":
            counters["cycles"] = value
        elif event == "instructions":
            counters["instructions"] = value
    return counters


def record(args: argparse.Namespace) -> None:
    document = json.loads(args.iperf_json.read_text(encoding="utf-8"))
    summary = _iperf_summary(document, args.test)
    sent_summary = _iperf_sent_summary(document)
    throughput_bps = _as_float(summary.get("bits_per_second"))
    offered_throughput_bps = _as_float(sent_summary.get("bits_per_second"))
    transferred_bytes = _as_int(summary.get("bytes"))
    sent_bytes = _as_int(sent_summary.get("bytes"))
    elapsed_seconds = _as_float(summary.get("seconds"))
    sent_elapsed_seconds = _as_float(sent_summary.get("seconds"))
    packet_count = _as_int(summary.get("packets"))
    lost_packets = _as_int(summary.get("lost_packets"))
    sent_packet_count = _as_int(sent_summary.get("packets"))
    perf = parse_perf(args.perf_csv)

    # iperf3 reports the offered UDP packet count, not the delivered count,
    # even in sum_received. Derive delivered packets from packets - lost.
    received_packet_count = None
    if packet_count is not None:
        received_packet_count = max(0, packet_count - (lost_packets or 0))

    packets_per_second = None
    if (
        received_packet_count is not None
        and elapsed_seconds
        and elapsed_seconds > 0
    ):
        packets_per_second = received_packet_count / elapsed_seconds

    sent_packets_per_second = None
    if sent_packet_count is not None and sent_elapsed_seconds and sent_elapsed_seconds > 0:
        sent_packets_per_second = sent_packet_count / sent_elapsed_seconds

    metadata = dict(item.split("=", 1) for item in args.field)
    cpu_seconds = _as_float(metadata.get("cpu_seconds"))

    cycles_per_byte = None
    if transferred_bytes and perf.get("cycles") is not None:
        cycles_per_byte = perf["cycles"] / transferred_bytes

    instructions_per_byte = None
    if transferred_bytes and perf.get("instructions") is not None:
        instructions_per_byte = perf["instructions"] / transferred_bytes
    cpu_seconds_per_byte = None
    if transferred_bytes and cpu_seconds is not None:
        cpu_seconds_per_byte = cpu_seconds / transferred_bytes

    result = {
        **metadata,
        "impl": args.impl,
        "test": args.test,
        "run": args.run,
        "valid": True,
        "invalid_reason": None,
        "metrics": {
            "throughput_gbps": throughput_bps / 1e9
            if throughput_bps is not None
            else None,
            "offered_throughput_gbps": offered_throughput_bps / 1e9
            if offered_throughput_bps is not None
            else None,
            "packets_per_second": packets_per_second,
            "sent_packets_per_second": sent_packets_per_second,
            "transferred_bytes": transferred_bytes,
            "sent_bytes": sent_bytes,
            "received_packets": received_packet_count,
            "sent_packets": sent_packet_count,
            "elapsed_seconds": elapsed_seconds,
            "jitter_ms": _as_float(summary.get("jitter_ms")),
            "lost_packets": lost_packets,
            "lost_percent": _as_float(summary.get("lost_percent")),
            "cpu_seconds": cpu_seconds,
            "cpu_seconds_per_byte": cpu_seconds_per_byte,
            "perf_task_clock_seconds": perf.get("task_clock_seconds"),
            "cycles_per_byte": cycles_per_byte,
            "instructions_per_byte": instructions_per_byte,
        },
    }

    invalid_reason = None
    if args.test.startswith("udp_") and sent_packet_count and sent_packet_count > 0:
        if received_packet_count == 0 or not transferred_bytes:
            invalid_reason = "zero_received_payload"
    elif transferred_bytes == 0:
        invalid_reason = "zero_received_payload"

    if invalid_reason is not None:
        result["valid"] = False
        result["invalid_reason"] = invalid_reason
        result["metrics"]["throughput_gbps"] = None
        result["metrics"]["packets_per_second"] = None
        result["metrics"]["cpu_seconds_per_byte"] = None
        result["metrics"]["cycles_per_byte"] = None
        result["metrics"]["instructions_per_byte"] = None

    with args.results.open("a", encoding="utf-8") as output:
        output.write(json.dumps(result, sort_keys=True, separators=(",", ":")) + "\n")

    if invalid_reason is not None:
        raise ValueError(
            f"invalid {args.impl}/{args.test}/run {args.run}: {invalid_reason} "
            f"(offered={offered_throughput_bps or 0:.3f} bit/s, "
            f"sent_packets={sent_packet_count or 0})"
        )

    print(
        f"{args.impl:20} {args.test:20} run={args.run} "
        f"throughput={result['metrics']['throughput_gbps'] or 0:.3f} Gbit/s "
        f"pps={packets_per_second or 0:.0f}"
    )


def _median(records: list[dict[str, Any]], metric: str) -> float | None:
    values = [
        record["metrics"][metric]
        for record in records
        if record.get("metrics", {}).get(metric) is not None
    ]
    return statistics.median(values) if values else None


def _format(value: float | None) -> str:
    # Keep very small CPU/byte values visible instead of rounding them to zero.
    return "" if value is None else f"{value:.12g}"


def aggregate(args: argparse.Namespace) -> None:
    records = [
        json.loads(line)
        for line in args.results.read_text(encoding="utf-8").splitlines()
        if line.strip()
    ]
    grouped: dict[tuple[str, str], list[dict[str, Any]]] = {}
    for record in records:
        key = (record["impl"], record["test"])
        grouped.setdefault(key, []).append(record)

    metrics = [
        "throughput_gbps",
        "offered_throughput_gbps",
        "packets_per_second",
        "sent_packets_per_second",
        "jitter_ms",
        "lost_percent",
        "cpu_seconds",
        "cpu_seconds_per_byte",
        "cycles_per_byte",
        "instructions_per_byte",
    ]
    with args.csv.open("w", encoding="utf-8", newline="") as output:
        writer = csv.writer(output)
        writer.writerow(["impl", "test", "runs", "valid_runs", "invalid_runs", *metrics])
        for (implementation, test), rows in sorted(grouped.items()):
            valid_rows = [row for row in rows if row.get("valid") is not False]
            writer.writerow(
                [
                    implementation,
                    test,
                    len(rows),
                    len(valid_rows),
                    len(rows) - len(valid_rows),
                    *[_format(_median(valid_rows, metric)) for metric in metrics],
                ]
            )


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    subparsers = parser.add_subparsers(dest="command", required=True)

    record_parser = subparsers.add_parser("record", help="append one raw iperf3 result")
    record_parser.add_argument("--iperf-json", type=Path, required=True)
    record_parser.add_argument("--perf-csv", type=Path)
    record_parser.add_argument("--results", type=Path, required=True)
    record_parser.add_argument("--impl", required=True)
    record_parser.add_argument("--test", required=True)
    record_parser.add_argument("--run", type=int, required=True)
    record_parser.add_argument(
        "--field",
        action="append",
        default=[],
        metavar="KEY=VALUE",
        help="metadata field; may be repeated",
    )
    record_parser.set_defaults(func=record)

    aggregate_parser = subparsers.add_parser(
        "aggregate", help="write per-implementation median metrics"
    )
    aggregate_parser.add_argument("--results", type=Path, required=True)
    aggregate_parser.add_argument("--csv", type=Path, required=True)
    aggregate_parser.set_defaults(func=aggregate)
    return parser


def main() -> int:
    try:
        args = build_parser().parse_args()
        args.func(args)
    except (OSError, ValueError, json.JSONDecodeError) as error:
        print(f"bench-summarize: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
