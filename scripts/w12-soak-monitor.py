#!/usr/bin/env python3
"""W12-09 24-hour direct-P2P stability monitor.

The normal mode records local and remote status/resource snapshots, periodic
IPv4/IPv6 probes, and real TCP canaries over the virtual network. The
``snapshot`` and ``snapshot-network`` modes are invoked remotely through SSH.
Credentials are read only from the environment through ``sshpass -e`` and are
never written into the evidence directory.
"""

import argparse
import concurrent.futures
import hashlib
import json
import math
import os
import pathlib
import re
import shlex
import socket
import subprocess
import sys
import time
from datetime import datetime, timezone
from typing import Any, Dict, List, Optional, Tuple


LOCAL = {
    "name": "local",
    "virtual_ipv4": "10.8.0.3",
    "virtual_ipv6": "fd00::1003",
    "binary": "/home/xlei/project/fuckipv6/target/x86_64-unknown-linux-musl/release/fuckipv6",
    "config": "/tmp/fuckipv6-w12-local/config",
    "runtime": "/tmp/fuckipv6-w12-local/runtime",
}
HK = {
    "name": "hong_kong",
    "host": "101.36.116.21",
    "virtual_ipv4": "10.8.0.1",
    "virtual_ipv6": "fd00::1001",
    "binary": "/root/fuckipv6-w12/fuckipv6",
    "config": "/root/fuckipv6-w12/config",
    "runtime": "/root/fuckipv6-w12/runtime",
}
CN47 = {
    "name": "cn47",
    "host": "47.87.67.139",
    "virtual_ipv4": "10.8.0.2",
    "virtual_ipv6": "fd00::1002",
    "binary": "/root/fuckipv6-w12-47/fuckipv6",
    "config": "/root/fuckipv6-w12-47/config",
    "runtime": "/root/fuckipv6-w12-47/runtime",
}

SSH_OPTIONS = [
    "-o",
    "StrictHostKeyChecking=no",
    "-o",
    "ConnectTimeout=10",
    "-o",
    "ServerAliveInterval=15",
    "-o",
    "ServerAliveCountMax=2",
]


def utc_now() -> str:
    return datetime.now(timezone.utc).isoformat().replace("+00:00", "Z")


def unix_ms() -> int:
    return int(time.time() * 1000)


def sha256_file(path: pathlib.Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def atomic_write(path: pathlib.Path, text: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_suffix(path.suffix + ".tmp")
    temporary.write_text(text)
    temporary.replace(path)


def append_jsonl(path: pathlib.Path, value: Any) -> None:
    with path.open("a", encoding="utf-8") as handle:
        handle.write(json.dumps(value, ensure_ascii=False, sort_keys=True) + "\n")
        handle.flush()
        os.fsync(handle.fileno())


def json_from_stdout(completed: subprocess.CompletedProcess) -> Any:
    if completed.returncode != 0:
        raise RuntimeError(
            f"command failed ({completed.returncode}): {completed.stderr.strip()}"
        )
    return json.loads(completed.stdout)


def process_metrics(binary: str) -> Dict[str, Any]:
    patterns = {
        "launcher": rf"^{re.escape(binary)} up ",
        "control": rf"^{re.escape(binary)} __control$",
        "node": rf"^{re.escape(binary)} __node$",
    }
    clock_ticks = os.sysconf("SC_CLK_TCK")
    result = {}  # type: Dict[str, Any]
    for role, pattern in patterns.items():
        completed = subprocess.run(
            ["pgrep", "-f", pattern], stdout=subprocess.PIPE, stderr=subprocess.PIPE, universal_newlines=True, timeout=10
        )
        pids = [int(line) for line in completed.stdout.splitlines() if line.strip()]
        if not pids:
            result[role] = {"found": False}
            continue
        pid = pids[0]
        proc = pathlib.Path(f"/proc/{pid}")
        status = {}
        for line in (proc / "status").read_text().splitlines():
            key, _, value = line.partition(":")
            if key in {"VmRSS", "VmSize", "Threads"}:
                status[key] = value.strip()
        stat_fields = (proc / "stat").read_text().split()
        result[role] = {
            "found": True,
            "pid": pid,
            "ppid": int(stat_fields[3]),
            "state": stat_fields[2],
            "start_ticks": int(stat_fields[21]),
            "cpu_seconds": (int(stat_fields[13]) + int(stat_fields[14])) / clock_ticks,
            "rss_kib": int(status.get("VmRSS", "0 KiB").split()[0]),
            "vsz_kib": int(status.get("VmSize", "0 KiB").split()[0]),
            "threads": int(status.get("Threads", "0")),
            "fd_count": len(list((proc / "fd").iterdir())),
        }
    return result


def directory_metrics(path: str) -> Dict[str, int]:
    root = pathlib.Path(path)
    files = 0
    bytes_total = 0
    if root.exists():
        for entry in root.rglob("*"):
            try:
                if entry.is_symlink() or entry.is_file():
                    files += 1
                    if not entry.is_symlink():
                        bytes_total += entry.stat().st_size
            except FileNotFoundError:
                continue
    return {"files": files, "bytes": bytes_total}


def socket_metrics(node_pid: int) -> Dict[str, int]:
    counts = {"tcp_established": 0, "tcp_listen": 0, "udp": 0}
    needle = f"pid={node_pid}"
    for family, key in (("-t", "tcp"), ("-u", "udp")):
        completed = subprocess.run(
            ["ss", "-H", "-a", "-n", "-p", family],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            universal_newlines=True,
            timeout=10,
        )
        for line in completed.stdout.splitlines():
            if needle not in line:
                continue
            if key == "udp":
                counts["udp"] += 1
            elif line.startswith("LISTEN"):
                counts["tcp_listen"] += 1
            elif line.startswith("ESTAB"):
                counts["tcp_established"] += 1
    return counts


def route_metrics(ips: List[str]) -> Dict[str, bool]:
    routes4 = subprocess.run(
        ["ip", "-4", "route", "show", "table", "all"],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        universal_newlines=True,
        timeout=10,
    ).stdout
    routes6 = subprocess.run(
        ["ip", "-6", "route", "show", "table", "all"],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        universal_newlines=True,
        timeout=10,
    ).stdout
    output = routes4 + routes6
    return {ip: ip in output for ip in ips}


def snapshot_mode() -> None:
    binary = os.environ["W12_BINARY"]
    runtime = os.environ["W12_RUNTIME_DIR"]
    route_ips = [ip for ip in os.environ.get("W12_ROUTE_IPS", "").split(",") if ip]
    processes = process_metrics(binary)
    node_pid = processes.get("node", {}).get("pid")
    result = {
        "captured_at": utc_now(),
        "hostname": socket.gethostname(),
        "processes": processes,
        "runtime": directory_metrics(runtime),
        "sockets": socket_metrics(node_pid) if node_pid else {},
        "routes": route_metrics(route_ips),
    }
    print(json.dumps(result, sort_keys=True))


def ping_target(name: str, address: str) -> Dict[str, Any]:
    family = "-6" if ":" in address else "-4"
    started = time.monotonic()
    completed = subprocess.run(
        ["ping", family, "-n", "-c", "3", "-W", "2", "-i", "0.2", address],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        universal_newlines=True,
        timeout=15,
    )
    output = completed.stdout + completed.stderr
    loss_match = re.search(r"(\d+(?:\.\d+)?)% packet loss", output)
    rtt_match = re.search(r"rtt min/avg/max/mdev = ([\d.]+)/([\d.]+)/([\d.]+)", output)
    return {
        "name": name,
        "address": address,
        "ok": completed.returncode == 0,
        "loss_percent": float(loss_match.group(1)) if loss_match else 100.0,
        "rtt_avg_ms": float(rtt_match.group(2)) if rtt_match else None,
        "elapsed_secs": time.monotonic() - started,
        "error": None if completed.returncode == 0 else output.strip()[-1000:],
    }


def parse_targets(value: str) -> List[Tuple[str, str]]:
    targets = []
    for item in value.split(","):
        if item:
            name, address = item.split("=", 1)
            targets.append((name, address))
    return targets


def snapshot_network_mode() -> None:
    targets = parse_targets(os.environ["W12_PING_TARGETS"])
    with concurrent.futures.ThreadPoolExecutor(max_workers=len(targets) or 1) as pool:
        futures = [pool.submit(ping_target, name, address) for name, address in targets]
        probes = [future.result() for future in futures]
    print(json.dumps({"captured_at": utc_now(), "probes": probes}, sort_keys=True))


def ssh_command(host: str, command: str) -> List[str]:
    return ["sshpass", "-e", "ssh", *SSH_OPTIONS, f"root@{host}", command]


def run_ssh(
    password: str,
    host: str,
    command: str,
    *,
    input_text: Optional[str] = None,
    timeout: int = 40,
) -> subprocess.CompletedProcess:
    environment = os.environ.copy()
    environment["SSHPASS"] = password
    return subprocess.run(
        ssh_command(host, command),
        input=input_text,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        universal_newlines=True,
        timeout=timeout,
        env=environment,
    )


def run_as_root(
    command: List[str], *, input_text: Optional[str] = None
) -> subprocess.CompletedProcess:
    return subprocess.run(
        ["sudo", "-n", *command],
        input=input_text,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        universal_newlines=True,
        timeout=40,
    )


def snapshot_environment(node: Dict[str, str]) -> Dict[str, str]:
    values = {
        "W12_BINARY": node["binary"],
        "W12_CONFIG_DIR": node["config"],
        "W12_RUNTIME_DIR": node["runtime"],
        "W12_ROUTE_IPS": f"{HK['virtual_ipv4']},{HK['virtual_ipv6']},{CN47['virtual_ipv4']},{CN47['virtual_ipv6']}",
    }
    return values


def collect_metrics(
    node: Dict[str, Any], script_path: pathlib.Path, ssh_password: str
) -> Dict[str, Any]:
    environment = snapshot_environment(node)
    if node["name"] == "local":
        command = ["env", *[f"{key}={value}" for key, value in environment.items()], sys.executable, str(script_path), "snapshot"]
        return json_from_stdout(run_as_root(command))
    command = "env " + " ".join(
        f"{key}={shlex.quote(value)}" for key, value in environment.items()
    ) + " python3 - snapshot"
    completed = run_ssh(
        ssh_password,
        node["host"],
        command,
        input_text=script_path.read_text(),
        timeout=45,
    )
    return json_from_stdout(completed)


def collect_status(node: Dict[str, Any], ssh_password: str) -> Dict[str, Any]:
    environment = [
        f"FUCKIPV6_CONFIG_DIR={node['config']}",
        f"FUCKIPV6_RUNTIME_DIR={node['runtime']}",
    ]
    if node["name"] == "local":
        completed = run_as_root(
            ["env", *environment, node["binary"], "status", "--json"]
        )
    else:
        command = "env " + " ".join(
            f"{key}={shlex.quote(value)}" for key, value in
            (item.split("=", 1) for item in environment)
        ) + " " + shlex.quote(node["binary"]) + " status --json"
        completed = run_ssh(ssh_password, node["host"], command, timeout=45)
    return json_from_stdout(completed)


def collect_remote_network(
    node: Dict[str, Any],
    targets: List[Tuple[str, str]],
    script_path: pathlib.Path,
    ssh_password: str,
) -> Dict[str, Any]:
    value = ",".join(f"{name}={address}" for name, address in targets)
    command = f"env W12_PING_TARGETS={shlex.quote(value)} python3 - snapshot-network"
    completed = run_ssh(
        ssh_password,
        node["host"],
        command,
        input_text=script_path.read_text(),
        timeout=50,
    )
    return json_from_stdout(completed)


def status_summary(status: Dict[str, Any]) -> Dict[str, Any]:
    networks = status.get("networks", [])
    network = networks[0] if networks else {}
    peers = []
    for peer in status.get("peers", []):
        peers.append(
            {
                "peer_id": peer.get("peer_id"),
                "virtual_ipv4": peer.get("virtual_ipv4"),
                "virtual_ipv6": peer.get("virtual_ipv6"),
                "online": peer.get("online"),
                "p2p_active": peer.get("p2p_active"),
                "p2p_healthy": peer.get("p2p_healthy"),
                "loss_rate": peer.get("loss_rate"),
                "traffic": peer.get("traffic"),
            }
        )
    return {
        "lifecycle": status.get("lifecycle"),
        "control_health": status.get("control_health"),
        "network": {
            "name": network.get("name"),
            "status": network.get("status"),
            "traffic": network.get("traffic"),
        },
        "peers": peers,
    }


def tcp_canary(ip: str, port: int, payload_size: int, cycle: int) -> Dict[str, Any]:
    seed = f"fuckipv6-w12-soak-cycle-{cycle}\n".encode()
    payload = (seed * math.ceil(payload_size / len(seed)))[:payload_size]
    digest = hashlib.sha256(payload).hexdigest()
    header = json.dumps({"bytes": len(payload), "sha256": digest, "cycle": cycle}).encode() + b"\n"
    started = time.monotonic()
    try:
        with socket.create_connection((ip, port), timeout=15) as conn:
            conn.settimeout(20)
            conn.sendall(header + payload)
            conn.shutdown(socket.SHUT_WR)
            response = b""
            while not response.endswith(b"\n"):
                chunk = conn.recv(4096)
                if not chunk:
                    break
                response += chunk
        result = json.loads(response)
        ok = result.get("ok") is True and result.get("sha256") == digest
        return {
            "ok": ok,
            "address": ip,
            "bytes": payload_size,
            "sha256": digest,
            "response": result,
            "elapsed_secs": time.monotonic() - started,
        }
    except Exception as exc:
        return {
            "ok": False,
            "address": ip,
            "bytes": payload_size,
            "sha256": digest,
            "elapsed_secs": time.monotonic() - started,
            "error": str(exc),
        }


class EventRecorder:
    def __init__(self, path: pathlib.Path) -> None:
        self.path = path
        self.last = {}  # type: Dict[str, bool]

    def transition(self, key: str, healthy: bool, detail: Any) -> None:
        previous = self.last.get(key)
        if previous is None:
            self.last[key] = healthy
            if not healthy:
                append_jsonl(
                    self.path,
                    {"at": utc_now(), "kind": "initial_unhealthy", "target": key, "detail": detail},
                )
            return
        if previous != healthy:
            self.last[key] = healthy
            append_jsonl(
                self.path,
                {
                    "at": utc_now(),
                    "kind": "recovered" if healthy else "degraded",
                    "target": key,
                    "detail": detail,
                },
            )


def resource_alerts(baseline: Dict[str, Any], current: Dict[str, Any]) -> List[str]:
    alerts = []
    for role, base in baseline.get("processes", {}).items():
        now = current.get("processes", {}).get(role)
        if not base.get("found") or not now or not now.get("found"):
            continue
        if now["pid"] != base["pid"]:
            alerts.append(f"{role} pid changed {base['pid']} -> {now['pid']}")
        if now["rss_kib"] > base["rss_kib"] + 128 * 1024:
            alerts.append(f"{role} RSS grew >128 MiB")
        if now["fd_count"] > base["fd_count"] + 32:
            alerts.append(f"{role} FD grew >32")
    if current.get("runtime", {}).get("bytes", 0) > baseline.get("runtime", {}).get("bytes", 0) + 256 * 1024 * 1024:
        alerts.append("runtime directory grew >256 MiB")
    return alerts


def summarize(
    records_path: pathlib.Path, evidence: pathlib.Path, planned_seconds: float
) -> Dict[str, Any]:
    records = [json.loads(line) for line in records_path.read_text().splitlines() if line.strip()]
    if not records:
        return {"status": "no_data", "records": 0}
    events_path = evidence / "events.jsonl"
    events = (
        [json.loads(line) for line in events_path.read_text().splitlines() if line.strip()]
        if events_path.exists()
        else []
    )
    event_counts = {}  # type: Dict[str, int]
    for event in events:
        kind = str(event.get("kind", "unknown"))
        event_counts[kind] = event_counts.get(kind, 0) + 1
    first = records[0]
    last = records[-1]
    probe_values = {}  # type: Dict[str, List[Dict[str, Any]]]
    canary_values = {}  # type: Dict[str, List[Dict[str, Any]]]
    status_failures = 0
    for record in records:
        for probe in record.get("local_network", {}).get("probes", []):
            probe_values.setdefault(f"local->{probe['name']}", []).append(probe)
        for side, network in record.get("remote_network", {}).items():
            for probe in network.get("probes", []):
                probe_values.setdefault(f"{side}->{probe['name']}", []).append(probe)
        for side, canary in record.get("tcp_canaries", {}).items():
            canary_values.setdefault(side, []).append(canary)
        if not record.get("local_status_ok", False):
            status_failures += 1
        for side in ("hong_kong", "cn47"):
            if record.get("remote_status_sampled") and not record.get("remote_status_ok", {}).get(side, False):
                status_failures += 1
    probe_summary = {
        key: {
            "samples": len(values),
            "successes": sum(1 for value in values if value.get("ok")),
            "success_rate": sum(1 for value in values if value.get("ok")) / len(values),
            "avg_rtt_ms": sum(value["rtt_avg_ms"] for value in values if value.get("rtt_avg_ms") is not None) / max(1, sum(1 for value in values if value.get("rtt_avg_ms") is not None)),
        }
        for key, values in sorted(probe_values.items())
    }
    canary_summary = {
        key: {
            "samples": len(values),
            "successes": sum(1 for value in values if value.get("ok")),
            "success_rate": sum(1 for value in values if value.get("ok")) / len(values),
            "p95_elapsed_secs": sorted(value["elapsed_secs"] for value in values)[max(0, math.ceil(len(values) * 0.95) - 1)],
        }
        for key, values in sorted(canary_values.items())
    }
    resource_summary = {}  # type: Dict[str, Any]
    for side in ("local_metrics", "hong_kong_metrics", "cn47_metrics"):
        samples = [record[side] for record in records if record.get(side)]
        if not samples:
            continue
        for role in ("launcher", "control", "node"):
            values = [sample["processes"].get(role, {}) for sample in samples]
            values = [value for value in values if value.get("found")]
            if not values:
                continue
            resource_summary[f"{side}.{role}"] = {
                "samples": len(values),
                "rss_kib": {
                    "first": values[0]["rss_kib"],
                    "last": values[-1]["rss_kib"],
                    "min": min(value["rss_kib"] for value in values),
                    "max": max(value["rss_kib"] for value in values),
                },
                "fd_count": {
                    "first": values[0]["fd_count"],
                    "last": values[-1]["fd_count"],
                    "min": min(value["fd_count"] for value in values),
                    "max": max(value["fd_count"] for value in values),
                },
            }
        runtime = [sample["runtime"] for sample in samples]
        resource_summary[f"{side}.runtime"] = {
            "files_first": runtime[0]["files"],
            "files_last": runtime[-1]["files"],
            "bytes_first": runtime[0]["bytes"],
            "bytes_last": runtime[-1]["bytes"],
            "bytes_max": max(value["bytes"] for value in runtime),
        }
    elapsed = last["elapsed_seconds"]
    canary_success = sum(value["successes"] for value in canary_summary.values())
    canary_samples = sum(value["samples"] for value in canary_summary.values())
    probe_success = sum(value["successes"] for value in probe_summary.values())
    probe_samples = sum(value["samples"] for value in probe_summary.values())
    route_ok = all(
        all(record.get("local_metrics", {}).get("routes", {}).values())
        for record in records
    )
    result = {
        "status": "completed" if elapsed >= planned_seconds else "stopped_early",
        "started_at": first["captured_at"],
        "finished_at": last["captured_at"],
        "elapsed_seconds": elapsed,
        "planned_seconds": planned_seconds,
        "records": len(records),
        "status_failures": status_failures,
        "probes": probe_summary,
        "tcp_canaries": canary_summary,
        "overall_probe_success_rate": probe_success / probe_samples if probe_samples else None,
        "overall_canary_success_rate": canary_success / canary_samples if canary_samples else None,
        "local_routes_present_for_all_records": route_ok,
        "resources": resource_summary,
        "events_file": str(events_path),
        "unhealthy_event_count": len(events),
        "unhealthy_event_kinds": event_counts,
    }
    result["verdict"] = (
        "pass"
        if result["status"] == "completed"
        and result["overall_probe_success_rate"] == 1.0
        and result["overall_canary_success_rate"] == 1.0
        and status_failures == 0
        and route_ok
        and not events
        else "review_required"
    )
    return result


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--duration-hours", type=float, default=24.0)
    parser.add_argument("--interval-seconds", type=int, default=60)
    parser.add_argument("--remote-interval-seconds", type=int, default=300)
    parser.add_argument("--tcp-port", type=int, default=39301)
    parser.add_argument("--tcp-payload-bytes", type=int, default=16384)
    parser.add_argument("--evidence-dir")
    parser.add_argument("--hk-host", default=HK["host"])
    parser.add_argument("--cn47-host", default=CN47["host"])
    parser.add_argument("--ssh-password-env", default="W12_SSH_PASSWORD")
    parser.add_argument("--once", action="store_true", help="Collect one full cycle for setup validation")
    parser.add_argument("--summary-only", action="store_true")
    args = parser.parse_args()

    script_path = pathlib.Path(__file__).resolve()
    if args.summary_only:
        if not args.evidence_dir:
            parser.error("--summary-only requires --evidence-dir")
        evidence = pathlib.Path(args.evidence_dir)
        metadata = json.loads((evidence / "run.json").read_text())
        summary = summarize(
            evidence / "metrics.jsonl", evidence, float(metadata["planned_seconds"])
        )
        atomic_write(evidence / "summary.json", json.dumps(summary, ensure_ascii=False, indent=2, sort_keys=True) + "\n")
        print(json.dumps(summary, ensure_ascii=False, indent=2, sort_keys=True))
        return

    password = os.environ.get(args.ssh_password_env)
    if not password:
        parser.error(f"missing SSH password environment variable {args.ssh_password_env}")
    if args.interval_seconds < 10 or args.remote_interval_seconds < args.interval_seconds:
        parser.error("intervals must be >=10s and remote interval must be >= local interval")

    HK["host"] = args.hk_host
    CN47["host"] = args.cn47_host
    nodes = [LOCAL, HK, CN47]
    timestamp = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ")
    evidence = pathlib.Path(
        args.evidence_dir or f"/tmp/fuckipv6-w12-real-verify/w12-09-soak-{timestamp}"
    )
    for child in ("status/local", "status/hong_kong", "status/cn47", "metrics"):
        (evidence / child).mkdir(parents=True, exist_ok=True)
    metrics_path = evidence / "metrics.jsonl"
    events = EventRecorder(evidence / "events.jsonl")
    planned_seconds = args.duration_hours * 3600
    started_monotonic = time.monotonic()
    metadata = {
        "test_id": "W12-09",
        "started_at": utc_now(),
        "planned_seconds": planned_seconds,
        "interval_seconds": args.interval_seconds,
        "remote_interval_seconds": args.remote_interval_seconds,
        "tcp_port": args.tcp_port,
        "tcp_payload_bytes": args.tcp_payload_bytes,
        "monitor_script": str(script_path),
        "monitor_sha256": sha256_file(script_path),
        "nodes": nodes,
        "scope": "Direct mesh-only P2P baseline; no Exit and no OpenClash changes.",
    }
    atomic_write(evidence / "run.json", json.dumps(metadata, ensure_ascii=False, indent=2, sort_keys=True) + "\n")
    atomic_write(evidence / "monitor.pid", str(os.getpid()) + "\n")
    print(json.dumps({"event": "started", "evidence": str(evidence), "pid": os.getpid()}), flush=True)

    baseline_metrics = {}  # type: Dict[str, Any]
    cycle = 0
    try:
        while True:
            cycle += 1
            cycle_started = time.monotonic()
            record = {  # type: Dict[str, Any]
                "cycle": cycle,
                "captured_at": utc_now(),
                "elapsed_seconds": cycle_started - started_monotonic,
            }
            try:
                local_status = collect_status(LOCAL, password)
                record["local_status_ok"] = True
                record["local_status"] = status_summary(local_status)
                atomic_write(
                    evidence / "status/local" / f"{cycle:05d}.json",
                    json.dumps(local_status, ensure_ascii=False, indent=2, sort_keys=True) + "\n",
                )
            except Exception as exc:
                record["local_status_ok"] = False
                record["local_status_error"] = str(exc)
                events.transition("local_status", False, str(exc))
            if record["local_status_ok"]:
                events.transition("local_status", True, None)
                for peer in record["local_status"]["peers"]:
                    healthy = bool(peer["online"] and peer["p2p_active"] and peer["p2p_healthy"])
                    events.transition(f"peer:{peer['virtual_ipv4']}", healthy, peer)

            try:
                local_metrics = collect_metrics(LOCAL, script_path, password)
                record["local_metrics"] = local_metrics
                if not baseline_metrics:
                    baseline_metrics["local"] = local_metrics
                for alert in resource_alerts(baseline_metrics["local"], local_metrics):
                    events.transition(f"resource:local:{alert}", False, {"alert": alert})
            except Exception as exc:
                record["local_metrics_error"] = str(exc)
                events.transition("local_metrics", False, str(exc))

            with concurrent.futures.ThreadPoolExecutor(max_workers=4) as pool:
                local_futures = [
                    pool.submit(ping_target, "hong_kong_ipv4", HK["virtual_ipv4"]),
                    pool.submit(ping_target, "hong_kong_ipv6", HK["virtual_ipv6"]),
                    pool.submit(ping_target, "cn47_ipv4", CN47["virtual_ipv4"]),
                    pool.submit(ping_target, "cn47_ipv6", CN47["virtual_ipv6"]),
                ]
                local_probes = [future.result() for future in local_futures]
            record["local_network"] = {"captured_at": utc_now(), "probes": local_probes}
            for probe in local_probes:
                events.transition(f"ping:local->{probe['name']}", bool(probe["ok"]), probe)

            remote_due = cycle == 1 or (cycle - 1) % max(1, args.remote_interval_seconds // args.interval_seconds) == 0
            record["remote_status_sampled"] = remote_due
            if remote_due:
                record["remote_status_ok"] = {}
                record["remote_network"] = {}
                canaries = {}  # type: Dict[str, Any]
                with concurrent.futures.ThreadPoolExecutor(max_workers=2) as pool:
                    tcp_futures = {
                        "hong_kong": pool.submit(tcp_canary, HK["virtual_ipv4"], args.tcp_port, args.tcp_payload_bytes, cycle),
                        "cn47": pool.submit(tcp_canary, CN47["virtual_ipv4"], args.tcp_port, args.tcp_payload_bytes, cycle),
                    }
                    for key, future in tcp_futures.items():
                        canaries[key] = future.result()
                record["tcp_canaries"] = canaries
                for key, canary in canaries.items():
                    events.transition(f"tcp:{key}", bool(canary["ok"]), canary)

                for node in (HK, CN47):
                    side = node["name"]
                    try:
                        status = collect_status(node, password)
                        record["remote_status_ok"][side] = True
                        record.setdefault("remote_status", {})[side] = status_summary(status)
                        atomic_write(
                            evidence / "status" / side / f"{cycle:05d}.json",
                            json.dumps(status, ensure_ascii=False, indent=2, sort_keys=True) + "\n",
                        )
                        events.transition(f"status:{side}", True, None)
                    except Exception as exc:
                        record["remote_status_ok"][side] = False
                        record.setdefault("remote_status_errors", {})[side] = str(exc)
                        events.transition(f"status:{side}", False, str(exc))
                    try:
                        metrics = collect_metrics(node, script_path, password)
                        record.setdefault("remote_metrics", {})[side] = metrics
                        baseline_metrics.setdefault(side, metrics)
                        for alert in resource_alerts(baseline_metrics[side], metrics):
                            events.transition(f"resource:{side}:{alert}", False, {"alert": alert})
                    except Exception as exc:
                        record.setdefault("remote_metrics_errors", {})[side] = str(exc)

                    targets = []
                    for target in (LOCAL, HK, CN47):
                        if target is node:
                            continue
                        targets.extend(
                            [
                                (f"{target['name']}_ipv4", target["virtual_ipv4"]),
                                (f"{target['name']}_ipv6", target["virtual_ipv6"]),
                            ]
                        )
                    try:
                        network = collect_remote_network(node, targets, script_path, password)
                        record["remote_network"][side] = network
                        for probe in network["probes"]:
                            events.transition(f"ping:{side}->{probe['name']}", bool(probe["ok"]), probe)
                    except Exception as exc:
                        record["remote_network"][side] = {"error": str(exc), "probes": []}
                        events.transition(f"ping:{side}", False, str(exc))

            append_jsonl(metrics_path, record)
            atomic_write(
                evidence / "run-state.json",
                json.dumps(
                    {
                        "pid": os.getpid(),
                        "cycle": cycle,
                        "last_heartbeat": utc_now(),
                        "elapsed_seconds": time.monotonic() - started_monotonic,
                        "planned_seconds": planned_seconds,
                    },
                    sort_keys=True,
                )
                + "\n",
            )
            print(
                json.dumps(
                    {
                        "event": "cycle",
                        "cycle": cycle,
                        "elapsed_seconds": round(time.monotonic() - started_monotonic, 3),
                    }
                ),
                flush=True,
            )
            if args.once or time.monotonic() - started_monotonic >= planned_seconds:
                break
            sleep_for = args.interval_seconds - (time.monotonic() - cycle_started)
            if sleep_for > 0:
                time.sleep(sleep_for)
    except Exception as exc:
        append_jsonl(events.path, {"at": utc_now(), "kind": "monitor_failure", "error": str(exc)})
        raise
    finally:
        summary = summarize(metrics_path, evidence, planned_seconds)
        atomic_write(evidence / "summary.json", json.dumps(summary, ensure_ascii=False, indent=2, sort_keys=True) + "\n")
        print(json.dumps({"event": "stopped", "summary": summary}, ensure_ascii=False), flush=True)


if __name__ == "__main__":
    mode = sys.argv[1] if len(sys.argv) > 1 else ""
    if mode == "snapshot":
        sys.argv.pop(1)
        snapshot_mode()
    elif mode == "snapshot-network":
        sys.argv.pop(1)
        snapshot_network_mode()
    else:
        main()
