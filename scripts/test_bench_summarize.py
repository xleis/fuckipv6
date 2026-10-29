#!/usr/bin/env python3
"""Regression tests for scripts/bench-summarize.py."""

from __future__ import annotations

import contextlib
import csv
import importlib.util
import io
import json
import tempfile
import unittest
from argparse import Namespace
from pathlib import Path

SCRIPT = Path(__file__).with_name("bench-summarize.py")
SPEC = importlib.util.spec_from_file_location("bench_summarize", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
SUMMARIZER = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(SUMMARIZER)


class SummarizerTests(unittest.TestCase):
    def test_udp_uses_received_packet_count_and_keeps_sent_metrics(self) -> None:
        document = {
            "end": {
                "sum_received": {
                    "bits_per_second": 6_306_403.7422,
                    "bytes": 1_880_192,
                    "seconds": 2.385121,
                    "jitter_ms": 0.0634,
                    "lost_packets": 543_892,
                    "packets": 573_270,
                    "lost_percent": 94.875,
                },
                "sum_sent": {
                    "bits_per_second": 146_718_239.6665,
                    "bytes": 36_689_280,
                    "seconds": 2.00053,
                    "packets": 573_270,
                },
            }
        }
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            iperf_json = root / "iperf.json"
            results = root / "results.jsonl"
            iperf_json.write_text(json.dumps(document), encoding="utf-8")
            args = Namespace(
                iperf_json=iperf_json,
                perf_csv=None,
                results=results,
                impl="fuckipv6",
                test="udp_64_saturated",
                run=1,
                field=["cpu_seconds=13.48"],
            )
            with contextlib.redirect_stdout(io.StringIO()):
                SUMMARIZER.record(args)
            metrics = json.loads(results.read_text(encoding="utf-8"))["metrics"]

        self.assertEqual(metrics["received_packets"], 29_378)
        self.assertEqual(metrics["sent_packets"], 573_270)
        self.assertAlmostEqual(metrics["packets_per_second"], 12_317.2, places=1)
        self.assertAlmostEqual(
            metrics["sent_packets_per_second"], 286_559.0, places=0
        )
        self.assertAlmostEqual(metrics["offered_throughput_gbps"], 0.146718, places=6)

    def test_csv_preserves_small_cpu_per_byte_values(self) -> None:
        metrics = {
            "cpu_seconds_per_byte": 1.25e-8,
            "cycles_per_byte": 30.0,
        }
        record = {
            "impl": "test",
            "test": "controlled",
            "metrics": metrics,
        }
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            results = root / "results.jsonl"
            summary = root / "summary.csv"
            results.write_text(
                json.dumps(record, separators=(",", ":")) + "\n",
                encoding="utf-8",
            )
            SUMMARIZER.aggregate(Namespace(results=results, csv=summary))
            with summary.open(newline="", encoding="utf-8") as source:
                row = next(csv.DictReader(source))

        self.assertEqual(row["cpu_seconds_per_byte"], "1.25e-08")
        self.assertEqual(row["cycles_per_byte"], "30")


if __name__ == "__main__":
    unittest.main()
