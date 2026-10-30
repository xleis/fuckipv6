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

    def test_shim_records_ping_paths_relative_to_evidence_root(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            fake_bin = root / "bin"
            fake_bin.mkdir()
            fake_iperf = fake_bin / "real-iperf3"
            fake_iperf.write_text("#!/bin/sh\nexit 0\n", encoding="utf-8")
            fake_iperf.chmod(0o755)
            fake_ping = fake_bin / "ping"
            fake_ping.write_text(
                "#!/bin/sh\n"
                "printf '64 bytes from fd00::2: time=1.00 ms\\n'\n"
                "printf '2 packets transmitted, 2 received, 0%% packet loss\\n'\n",
                encoding="utf-8",
            )
            fake_ping.chmod(0o755)
            output = root / "results"
            output.mkdir()
            metadata = output / "latency-invocations.jsonl"
            env = {
                **os.environ,
                "PATH": f"{fake_bin}:{os.environ.get('PATH', '')}",
                "FIP6_BENCH_REAL_IPERF3": str(fake_iperf),
                "FIP6_BENCH_LATENCY_LOG": str(metadata),
                "FIP6_BENCH_LATENCY_DIR": str(output),
                "FIP6_BENCH_PROBE_INTERVAL_MS": "10",
                "FIP6_BENCH_PROBE_TIMEOUT_S": "1",
            }
            result = subprocess.run(
                [
                    sys.executable,
                    str(SCRIPT_DIR / "iperf3_shim.py"),
                    "-6",
                    "-J",
                    "-c",
                    "fd00::2",
                    "-t",
                    "0.02",
                ],
                check=False,
                text=True,
                capture_output=True,
                env=env,
            )
            record = json.loads(metadata.read_text(encoding="utf-8").splitlines()[0])
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(record["ping_file"], Path(record["ping_file"]).as_posix())
        self.assertTrue(record["ping_file"].startswith("latency/probe-"))
        self.assertFalse(Path(record["ping_file"]).is_absolute())
        self.assertTrue(record["ping_stderr_file"].startswith("latency/probe-"))


class GateTests(unittest.TestCase):
    def _make_output(
        self,
        root: Path,
        *,
        implementations: list[str] | None = None,
        candidate_p99_ms: float = 9.0,
        missing_candidate_udp_packets: bool = False,
        controlled_losses: dict[tuple[str, int], float] | None = None,
        latency_samples: int = 200,
    ) -> Path:
        implementations = implementations or list(harness.IMPLEMENTATIONS)
        output = root / "results"
        output.mkdir()
        policy = copy.deepcopy(harness.DEFAULT_POLICY)
        metadata = harness.policy_metadata(policy)

        binary_dir = root / "bin"
        binary_dir.mkdir()
        binaries: dict[str, object] = {}
        for name in ("fuckipv6", "coordinator", "wireguard-go"):
            path = binary_dir / name
            path.write_text(f"#!/bin/sh\n# {name}\n", encoding="utf-8")
            path.chmod(0o755)
            binaries[name] = {"path": str(path), "sha256": harness.sha256_file(path)}
        tools = {}
        for name in ("ip", "iperf3", "perf", "ping", "python3", "taskset", "wg", "git"):
            path = binary_dir / f"tool-{name}"
            path.write_text(f"#!/bin/sh\n# {name}\n", encoding="utf-8")
            tools[name] = {"path": str(path), "sha256": harness.sha256_file(path)}

        wireguard_go = (
            {
                "path": binaries["wireguard-go"]["path"],
                "version": "wireguard-go test",
                "sha256": binaries["wireguard-go"]["sha256"],
            }
            if "wireguard-go" in implementations
            else None
        )
        plan = {
            "schema": harness.PLAN_SCHEMA,
            "evidence": True,
            "policy": policy,
            "policy_id": metadata["id"],
            "policy_sha256": metadata["sha256"],
            "policy_canonical": metadata["canonical"],
            "canonical_policy_id": harness.CANONICAL_POLICY_ID,
            "source": {
                "schema": harness.SOURCE_SCHEMA,
                "git_commit": "a" * 40,
                "git_tree": "b" * 40,
                "git_status": "",
                "dirty": False,
            },
            "implementations": implementations,
            "requested_implementations": implementations,
            "skipped": (
                [{"implementation": "wireguard-go", "reason": "not installed"}]
                if "wireguard-go" not in implementations
                else []
            ),
            "expected_kernel": "test-kernel",
            "wireguard_go": wireguard_go,
            "binaries": {
                "fuckipv6": binaries["fuckipv6"],
                "coordinator": binaries["coordinator"],
                "wireguard-go": binaries.get("wireguard-go"),
                "tools": tools,
            },
            "cpu_affinity": {"client": [0], "server": [1], "control": [2]},
            "environment_limits": copy.deepcopy(policy["environment"]),
            "traffic": {
                "runs": harness.DEFAULT_POLICY["required_runs"],
                "duration_seconds": 30,
                "omit_seconds": 2,
                "mtu": 1420,
                "udp_1420_controlled_rate": "50M",
                "udp_64_controlled_rate": "5M",
            },
            "latency_probe": {
                "implementation": "concurrent ICMPv6 echo through the tunnel",
                "interval_ms": 10.0,
                "timeout_seconds": 1.0,
                "aggregation": "nearest-rank p99 per run, median across runs",
            },
        }
        harness.write_json(output / "plan.json", plan)
        (output / "plan.sha256").write_text(
            harness.sha256_file(output / "plan.json") + "\n", encoding="utf-8"
        )
        harness.write_json(
            output / "environment.json",
            {
                "schema": harness.ENVIRONMENT_SCHEMA,
                "kernel": "test-kernel",
                "quiet_host": {"passed": True, "failures": []},
                "cpu_affinity": {"client": "0", "server": "1", "control": "2"},
                "wireguard_go": wireguard_go,
            },
        )

        losses = controlled_losses or {}
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
                for run_number in range(1, harness.DEFAULT_POLICY["required_runs"] + 1):
                    is_udp = test.startswith("udp_")
                    loss = losses.get((implementation, run_number), 0.0)
                    bits_per_second = throughput[implementation] * 1_000_000_000.0
                    sent_summary = {
                        "seconds": 30.0,
                        "bytes": 1_000_000,
                        "bits_per_second": bits_per_second,
                    }
                    received_summary = {
                        "seconds": 30.0,
                        "bytes": 1_000_000,
                        "bits_per_second": bits_per_second,
                    }
                    if is_udp:
                        sent_summary.update(
                            {"packets": 1000, "lost_percent": 0.0, "lost_packets": 0}
                        )
                        received_summary.update(
                            {"packets": 1000, "lost_percent": loss, "lost_packets": 0}
                        )
                    if (
                        missing_candidate_udp_packets
                        and implementation == "fuckipv6"
                        and is_udp
                    ):
                        sent_summary.pop("packets", None)
                    raw_stem = f"{implementation}__{test}__run{run_number}"
                    raw_json = output / "raw" / f"{raw_stem}.json"
                    harness.write_json(
                        raw_json,
                        {
                            "start": {},
                            "intervals": [],
                            "end": {
                                "sum_sent": sent_summary,
                                "sum_received": received_summary,
                            },
                        },
                    )
                    perf_csv = output / "raw" / f"{raw_stem}.perf.csv"
                    perf_csv.parent.mkdir(parents=True, exist_ok=True)
                    perf_csv.write_text(
                        f"{cpu_seconds[implementation] * 1000.0:.3f},msec,task-clock,1,100.00,,\n",
                        encoding="utf-8",
                    )
                    result_lines.append(
                        {
                            "impl": implementation,
                            "test": test,
                            "run": run_number,
                            "metrics": {
                                "throughput_gbps": throughput[implementation],
                                "offered_throughput_gbps": throughput[implementation],
                                "transferred_bytes": 1_000_000,
                                "sent_bytes": 1_000_000,
                                "sent_packets": 1000 if is_udp else None,
                                "lost_percent": loss if is_udp else None,
                                "elapsed_seconds": 30.0,
                                "cpu_seconds": cpu_seconds[implementation],
                                "perf_task_clock_seconds": cpu_seconds[implementation],
                            },
                        }
                    )

                    ping_file = (
                        output
                        / "latency"
                        / f"{implementation}-{test}-run{run_number}.txt"
                    )
                    ping_file.parent.mkdir(parents=True, exist_ok=True)
                    ping_file.write_text(
                        "".join(
                            f"64 bytes from fd00::2: icmp_seq={index} ttl=64 time={p99_ms[implementation]:.2f} ms\n"
                            for index in range(1, latency_samples + 1)
                        )
                        + (
                            f"{latency_samples} packets transmitted, {latency_samples} received, "
                            f"0% packet loss\n"
                        ),
                        encoding="utf-8",
                    )
                    latency_lines.append(
                        {
                            "schema": harness.LATENCY_SCHEMA,
                            "kind": "client",
                            "started_ns": len(latency_lines) + 1,
                            "test_hint": test,
                            "ping_file": str(ping_file.relative_to(output)),
                            "expected_samples": 200,
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
        run.write_evidence_manifest(output)
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
        self.assertTrue(any("sent packets" in error for error in report["errors"]))

    def test_noncanonical_weakened_policy_never_passes(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            output = self._make_output(Path(directory))
            plan = harness.read_json(output / "plan.json")
            plan["policy"]["throughput_min_ratio"] = 0.01
            metadata = harness.policy_metadata(plan["policy"])
            plan["policy_id"] = metadata["id"]
            plan["policy_sha256"] = metadata["sha256"]
            plan["policy_canonical"] = metadata["canonical"]
            harness.write_json(output / "plan.json", plan)
            (output / "plan.sha256").write_text(
                harness.sha256_file(output / "plan.json") + "\n", encoding="utf-8"
            )
            run.write_evidence_manifest(output)
            report, exit_code = gate.evaluate_output(output)
        self.assertEqual(exit_code, 1)
        self.assertEqual(report["status"], "failed")
        self.assertTrue(any("not the canonical" in error for error in report["errors"]))
        self.assertFalse(report["policy_metadata"]["canonical"])

    def test_canonical_policy_with_weakened_execution_plan_cannot_pass(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            output = self._make_output(Path(directory))
            plan = harness.read_json(output / "plan.json")
            plan["traffic"]["duration_seconds"] = 2
            plan["environment_limits"]["load1_max"] = 100.0
            plan["latency_probe"]["interval_ms"] = 1000.0
            harness.write_json(output / "plan.json", plan)
            (output / "plan.sha256").write_text(
                harness.sha256_file(output / "plan.json") + "\n", encoding="utf-8"
            )
            run.write_evidence_manifest(output)
            report, exit_code = gate.evaluate_output(output)
        self.assertEqual(exit_code, 1)
        self.assertEqual(report["status"], "failed")
        self.assertTrue(
            any(
                "plan traffic.duration_seconds does not match the canonical policy" in error
                for error in report["errors"]
            )
        )
        self.assertTrue(
            any(
                "plan environment_limits.load1_max does not match the canonical policy" in error
                for error in report["errors"]
            )
        )
        self.assertTrue(
            any(
                "plan latency_probe.interval_ms does not match the canonical policy" in error
                for error in report["errors"]
            )
        )

    def test_controlled_udp_loss_is_checked_per_run_not_by_median(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            report, exit_code = gate.evaluate_output(
                self._make_output(
                    Path(directory),
                    controlled_losses={
                        ("fuckipv6", 4): 50.0,
                        ("fuckipv6", 5): 50.0,
                    },
                )
            )
        self.assertEqual(exit_code, 1)
        self.assertEqual(report["status"], "failed")
        self.assertTrue(
            any(
                "udp_1420_controlled" in failure and "packet_loss" in failure
                for failure in report["failed_checks"]
            )
        )
        controlled = next(
            comparison
            for comparison in report["comparisons"]
            if comparison["comparator"] == "wireguard-kernel"
            and comparison["test"] == "udp_1420_controlled"
        )
        self.assertEqual(
            controlled["checks"]["packet_loss"]["candidate_per_run_percent"][-2:],
            [50.0, 50.0],
        )

    def test_zero_cpu_from_raw_perf_cannot_pass(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            output = self._make_output(Path(directory))
            perf = output / "raw" / "fuckipv6__tcp_1flow__run1.perf.csv"
            perf.write_text("0,msec,task-clock,1,100.00,,\n", encoding="utf-8")
            run.write_evidence_manifest(output)
            report, exit_code = gate.evaluate_output(output)
        self.assertEqual(exit_code, 1)
        self.assertEqual(report["status"], "failed")
        self.assertTrue(any("must be positive" in error for error in report["errors"]))

    def test_nonpositive_reported_cpu_cannot_pass(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            output = self._make_output(Path(directory))
            result_path = output / "results.jsonl"
            records = [
                json.loads(line)
                for line in result_path.read_text(encoding="utf-8").splitlines()
            ]
            records[0]["metrics"]["cpu_seconds"] = -1.0
            result_path.write_text(
                "".join(json.dumps(record) + "\n" for record in records),
                encoding="utf-8",
            )
            run.write_evidence_manifest(output)
            report, exit_code = gate.evaluate_output(output)
        self.assertEqual(exit_code, 1)
        self.assertEqual(report["status"], "failed")
        self.assertTrue(
            any("reported cpu_seconds must be positive" in error for error in report["errors"])
        )

    def test_nonpositive_raw_throughput_cannot_pass(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            output = self._make_output(Path(directory))
            raw_path = output / "raw" / "fuckipv6__tcp_1flow__run1.json"
            raw = harness.read_json(raw_path)
            raw["end"]["sum_received"]["bits_per_second"] = 0
            harness.write_json(raw_path, raw)
            run.write_evidence_manifest(output)
            report, exit_code = gate.evaluate_output(output)
        self.assertEqual(exit_code, 1)
        self.assertEqual(report["status"], "failed")
        self.assertTrue(any("raw throughput" in error for error in report["errors"]))

    def test_raw_udp_loss_outside_percent_range_cannot_pass(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            output = self._make_output(Path(directory))
            raw_path = output / "raw" / "fuckipv6__udp_1420_controlled__run1.json"
            raw = harness.read_json(raw_path)
            raw["end"]["sum_received"]["lost_percent"] = -1.0
            harness.write_json(raw_path, raw)
            run.write_evidence_manifest(output)
            report, exit_code = gate.evaluate_output(output)
        self.assertEqual(exit_code, 1)
        self.assertEqual(report["status"], "failed")
        self.assertTrue(
            any("raw lost percent must be at least 0.0" in error for error in report["errors"])
        )

    def test_zero_raw_udp_packet_count_cannot_pass(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            output = self._make_output(Path(directory))
            raw_path = output / "raw" / "fuckipv6__udp_1420_controlled__run1.json"
            raw = harness.read_json(raw_path)
            raw["end"]["sum_sent"]["packets"] = 0
            harness.write_json(raw_path, raw)
            run.write_evidence_manifest(output)
            report, exit_code = gate.evaluate_output(output)
        self.assertEqual(exit_code, 1)
        self.assertEqual(report["status"], "failed")
        self.assertTrue(
            any("raw sent packets must be positive" in error for error in report["errors"])
        )

    def test_latency_sample_count_is_range_checked(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            report, exit_code = gate.evaluate_output(
                self._make_output(Path(directory), latency_samples=100)
            )
        self.assertEqual(exit_code, 1)
        self.assertEqual(report["status"], "failed")
        quality_errors = [
            error
            for comparison in report["comparisons"]
            for error in comparison["checks"].get("p99", {}).get("quality_errors", [])
        ]
        self.assertTrue(any("below 200" in error for error in quality_errors))

    def test_zero_latency_samples_cannot_pass(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            report, exit_code = gate.evaluate_output(
                self._make_output(Path(directory), latency_samples=0)
            )
        self.assertEqual(exit_code, 1)
        self.assertEqual(report["status"], "failed")
        self.assertTrue(
            any("transmitted no packets" in error for error in report["errors"])
        )

    def test_raw_evidence_recomputes_reported_metrics(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            output = self._make_output(Path(directory))
            result_path = output / "results.jsonl"
            records = [
                json.loads(line)
                for line in result_path.read_text(encoding="utf-8").splitlines()
            ]
            records[0]["metrics"]["throughput_gbps"] = 999.0
            result_path.write_text(
                "".join(json.dumps(record) + "\n" for record in records),
                encoding="utf-8",
            )
            run.write_evidence_manifest(output)
            report, exit_code = gate.evaluate_output(output)
        self.assertEqual(exit_code, 1)
        self.assertEqual(report["status"], "failed")
        self.assertTrue(
            any("does not match raw evidence" in error for error in report["errors"])
        )

    def test_tampered_raw_file_breaks_evidence_manifest(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            output = self._make_output(Path(directory))
            raw_path = output / "raw" / "fuckipv6__tcp_1flow__run1.json"
            raw_path.write_text("{}\n", encoding="utf-8")
            report, exit_code = gate.evaluate_output(output)
        self.assertEqual(exit_code, 1)
        self.assertEqual(report["status"], "failed")
        self.assertTrue(any("evidence hash mismatch" in error for error in report["errors"]))

    def test_source_and_binary_provenance_are_required(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            output = self._make_output(Path(directory))
            plan = harness.read_json(output / "plan.json")
            del plan["source"]
            plan["binaries"]["fuckipv6"]["sha256"] = "0" * 64
            harness.write_json(output / "plan.json", plan)
            (output / "plan.sha256").write_text(
                harness.sha256_file(output / "plan.json") + "\n", encoding="utf-8"
            )
            run.write_evidence_manifest(output)
            report, exit_code = gate.evaluate_output(output)
        self.assertEqual(exit_code, 1)
        self.assertEqual(report["status"], "failed")
        self.assertTrue(any("source provenance" in error for error in report["errors"]))
        self.assertTrue(
            any("binary SHA-256 changed" in error for error in report["errors"])
        )

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
