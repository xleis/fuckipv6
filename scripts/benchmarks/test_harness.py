#!/usr/bin/env python3
"""Executable parser and gate tests for the NX-17 benchmark harness."""

from __future__ import annotations

import copy
import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

SCRIPT_DIR = Path(__file__).resolve().parent
sys.path.insert(0, str(SCRIPT_DIR))

import gate  # noqa: E402
import harness  # noqa: E402
import iperf3_shim  # noqa: E402
import run  # noqa: E402


class ParserTests(unittest.TestCase):
    def test_cpu_lists_are_validated_and_must_be_disjoint(self) -> None:
        self.assertEqual(harness.parse_cpu_list("0,2-3", {0, 1, 2, 3}), {0, 2, 3})
        with self.assertRaisesRegex(ValueError, "descending"):
            harness.parse_cpu_list("3-1")
        with self.assertRaisesRegex(ValueError, "not available"):
            harness.parse_cpu_list("4", {0, 1})
        with self.assertRaisesRegex(ValueError, "share"):
            harness.cpu_sets_disjoint({"client": {1, 2}, "server": {2, 3}})

    def test_iperf_arguments_map_to_predeclared_traffic_models(self) -> None:
        self.assertEqual(
            iperf3_shim.classify_iperf_args(["-6", "-J", "-c", "fd00::2", "-t", "10"]),
            "tcp_1flow",
        )
        self.assertEqual(
            iperf3_shim.classify_iperf_args(["-c", "fd00::2", "-P", "8"]),
            "tcp_8flow",
        )
        self.assertEqual(
            iperf3_shim.classify_iperf_args(["-u", "-b", "0", "-l", "1420"]),
            "udp_1420_saturated",
        )
        self.assertEqual(
            iperf3_shim.classify_iperf_args(["-u", "-b", "5M", "-l", "64"]),
            "udp_64_controlled",
        )

    def test_wireguard_go_is_required_unless_partial_evidence_is_explicit(self) -> None:
        base = [
            "--impls",
            "fuckipv6 wireguard-kernel",
            "--client-cpus",
            "0",
            "--server-cpus",
            "1",
            "--control-cpus",
            "2",
            "--expected-kernel",
            "test-kernel",
        ]
        with self.assertRaisesRegex(ValueError, "wireguard-go is a required comparator"):
            run.validate_args(run.parse_args(base))
        args = run.parse_args([*base, "--allow-missing-wireguard-go"])
        run.validate_args(args)
        self.assertTrue(args.allow_missing_wireguard_go)

    def test_missing_wireguard_go_skip_is_explicit(self) -> None:
        args = run.parse_args(
            [
                "--impls",
                "fuckipv6 wireguard-kernel",
                "--allow-missing-wireguard-go",
                "--wireguard-go-bin",
                "/definitely/not/present/wireguard-go",
                "--client-cpus",
                "0",
                "--server-cpus",
                "1",
                "--control-cpus",
                "2",
                "--expected-kernel",
                "test-kernel",
            ]
        )
        go, skip = run._resolve_wireguard_go(args)
        self.assertIsNone(go)
        self.assertEqual(skip["implementation"], "wireguard-go")
        self.assertIn("not found", skip["reason"])

    def test_shim_passes_version_through_without_fake_latency(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            fake_iperf = root / "fake-iperf3"
            fake_iperf.write_text("#!/bin/sh\nprintf 'fake iperf3 version\\n'\n", encoding="utf-8")
            fake_iperf.chmod(0o755)
            metadata = root / "latency.jsonl"
            env = {
                **os.environ,
                "FIP6_BENCH_REAL_IPERF3": str(fake_iperf),
                "FIP6_BENCH_LATENCY_LOG": str(metadata),
                "FIP6_BENCH_LATENCY_DIR": str(root / "latency"),
            }
            result = subprocess.run(
                [sys.executable, str(SCRIPT_DIR / "iperf3_shim.py"), "--version"],
                check=False,
                text=True,
                capture_output=True,
                env=env,
            )
        self.assertEqual(result.returncode, 0)
        self.assertEqual(result.stdout, "fake iperf3 version\n")
        self.assertFalse(metadata.exists())


class GateTests(unittest.TestCase):
    def _make_output(
        self,
        root: Path,
        *,
        implementations: list[str] | None = None,
        candidate_p99_ms: float = 9.0,
        missing_candidate_udp_packets: bool = False,
    ) -> Path:
        implementations = implementations or list(harness.IMPLEMENTATIONS)
        output = root / "results"
        output.mkdir()
        policy = copy.deepcopy(harness.DEFAULT_POLICY)
        policy["required_runs"] = 1
        policy["latency"]["min_samples_per_run"] = 10
        policy["latency"]["max_loss_percent"] = 100.0
        plan = {
            "schema": harness.PLAN_SCHEMA,
            "policy": policy,
            "implementations": implementations,
            "requested_implementations": implementations,
            "skipped": (
                [{"implementation": "wireguard-go", "reason": "not installed"}]
                if "wireguard-go" not in implementations
                else []
            ),
            "expected_kernel": "test-kernel",
            "cpu_affinity": {"client": [0], "server": [1], "control": [2]},
        }
        harness.write_json(output / "plan.json", plan)
        (output / "plan.sha256").write_text(
            harness.sha256_file(output / "plan.json") + "\n", encoding="utf-8"
        )
        harness.write_json(
            output / "environment.json",
            {
                "kernel": "test-kernel",
                "quiet_host": {"passed": True, "failures": []},
                "cpu_affinity": {"client": "0", "server": "1", "control": "2"},
            },
        )

        throughput = {"fuckipv6": 110.0, "wireguard-kernel": 100.0, "wireguard-go": 95.0}
        cpu_seconds = {"fuckipv6": 1.0, "wireguard-kernel": 2.0, "wireguard-go": 2.0}
        p99_ms = {
            "fuckipv6": candidate_p99_ms,
            "wireguard-kernel": 10.0,
            "wireguard-go": 10.0,
        }
        result_lines = []
        latency_lines = []
        for implementation in implementations:
            for test in harness.TESTS:
                metrics = {
                    "throughput_gbps": throughput[implementation],
                    "cpu_seconds": cpu_seconds[implementation],
                    "sent_packets": 1000,
                    "transferred_bytes": 1_000_000,
                    "lost_percent": 0.0,
                }
                if missing_candidate_udp_packets and implementation == "fuckipv6" and test.startswith("udp_"):
                    metrics["sent_packets"] = None
                result_lines.append(
                    {
                        "impl": implementation,
                        "test": test,
                        "run": 1,
                        "metrics": metrics,
                    }
                )
                ping_file = output / "latency" / f"{implementation}-{test}.txt"
                ping_file.parent.mkdir(parents=True, exist_ok=True)
                ping_file.write_text(
                    "".join(
                        f"64 bytes from fd00::2: icmp_seq={index} ttl=64 time={p99_ms[implementation]:.2f} ms\n"
                        for index in range(1, 101)
                    )
                    + "100 packets transmitted, 100 received, 0% packet loss\n",
                    encoding="utf-8",
                )
                latency_lines.append(
                    {
                        "schema": harness.LATENCY_SCHEMA,
                        "kind": "client",
                        "started_ns": len(latency_lines) + 1,
                        "test_hint": test,
                        "ping_file": str(ping_file.relative_to(output)),
                        "expected_samples": 100,
                        "probe_interval_ms": 10.0,
                        "ping_returncode": 0,
                    }
                )
        (output / "results.jsonl").write_text(
            "".join(json.dumps(record) + "\n" for record in result_lines),
            encoding="utf-8",
        )
        (output / "latency-invocations.jsonl").write_text(
            "".join(json.dumps(record) + "\n" for record in latency_lines),
            encoding="utf-8",
        )
        return output

    def test_nearest_rank_p99_and_ping_parser(self) -> None:
        self.assertEqual(gate.nearest_rank_percentile([float(i) for i in range(1, 101)], 99), 99.0)
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "ping.txt"
            path.write_text(
                "time=1.00 ms\ntime=9.00 ms\n10 packets transmitted, 2 received\n",
                encoding="utf-8",
            )
            stats = gate.parse_ping_file(path, expected_samples=10)
        self.assertEqual(stats["p99_ms"], 9.0)
        self.assertEqual(stats["loss_percent"], 80.0)

    def test_complete_matrix_above_all_thresholds_passes(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            report, exit_code = gate.evaluate_output(self._make_output(Path(directory)))
        self.assertEqual(exit_code, 0)
        self.assertEqual(report["status"], "passed")
        self.assertTrue(all(item["passed"] for item in report["comparisons"]))

    def test_worse_p99_fails_explicitly(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            report, exit_code = gate.evaluate_output(
                self._make_output(Path(directory), candidate_p99_ms=11.0)
            )
        self.assertEqual(exit_code, 1)
        self.assertEqual(report["status"], "failed")
        self.assertTrue(any("p99" in failure for failure in report["failed_checks"]))

    def test_missing_wireguard_go_is_incomplete_never_pass(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            report, exit_code = gate.evaluate_output(
                self._make_output(
                    Path(directory),
                    implementations=["fuckipv6", "wireguard-kernel"],
                )
            )
        self.assertEqual(exit_code, 2)
        self.assertEqual(report["status"], "incomplete")
        self.assertTrue(report["warnings"])

    def test_missing_cpu_denominator_cannot_pass(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            report, exit_code = gate.evaluate_output(
                self._make_output(Path(directory), missing_candidate_udp_packets=True)
            )
        self.assertEqual(exit_code, 1)
        self.assertEqual(report["status"], "failed")
        self.assertTrue(any("cpu" in failure for failure in report["failed_checks"]))

    def test_no_measured_results_cannot_pass(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            output = self._make_output(Path(directory))
            (output / "results.jsonl").unlink()
            report, exit_code = gate.evaluate_output(output)
        self.assertEqual(exit_code, 1)
        self.assertEqual(report["status"], "failed")
        self.assertTrue(any("results.jsonl is empty" in error for error in report["errors"]))


if __name__ == "__main__":
    unittest.main()
