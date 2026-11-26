#!/usr/bin/env python3
"""Run every fault-injection suite and emit one consolidated evidence report.

The individual suites live next to the features that need them, each with its own
privilege requirement. This runner does not invent evidence: it executes what the
current environment can execute, records `skipped` with the suite's own reason
for the rest, and refuses to call a partial run complete unless asked.
"""

from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
import time
from pathlib import Path
from typing import Iterable

PROJECT_ROOT = Path(__file__).resolve().parent.parent
SKIP_STATUS = 77
REPORT_SCHEMA = "fuckipv6-fault-injection-suite"


class Suite:
    def __init__(
        self,
        name: str,
        domain: str,
        description: str,
        script: str | None = None,
        *,
        rootless_script: str | None = None,
        cargo_test: tuple[str, str, str] | None = None,
        requires_release_build: bool = False,
        root_required: bool = False,
        timeout_seconds: int = 600,
    ) -> None:
        self.name = name
        self.domain = domain
        self.description = description
        self.script = script
        self.rootless_script = rootless_script
        self.cargo_test = cargo_test
        self.requires_release_build = requires_release_build
        self.root_required = root_required
        self.timeout_seconds = timeout_seconds

    def command(self, is_root: bool) -> list[str] | None:
        if self.cargo_test:
            package, target, test_name = self.cargo_test
            return ["cargo", "test", "-p", package, "--test", target, test_name, "--", "--exact"]
        if self.rootless_script:
            return ["bash", "scripts/w12-rootless-netns.sh", "bash", self.rootless_script]
        if self.root_required and not is_root:
            return None
        if self.script is None:
            return None
        return ["bash", self.script]


SUITES: tuple[Suite, ...] = (
    Suite(
        "lifecycle-cleanup",
        "生命周期",
        "关停后 TUN、路由、DNS、子进程清理可重试",
        script="scripts/test-lifecycle-cleanup-netns.sh",
        root_required=True,
        timeout_seconds=600,
    ),
    Suite(
        "recovery",
        "生命周期",
        "残留资源自动恢复与再次启动",
        script="scripts/test-recovery-netns.sh",
        root_required=True,
        timeout_seconds=600,
    ),
    Suite(
        "traffic-routing",
        "路由",
        "规则命中与流量归属",
        script="scripts/test-traffic-routing-netns.sh",
        root_required=True,
        timeout_seconds=900,
    ),
    Suite(
        "exit-node",
        "Exit",
        "本机 Exit 数据面与回退",
        script="scripts/test-exit-node-netns.sh",
        root_required=True,
        timeout_seconds=900,
    ),
    Suite(
        "w12-interference",
        "网络",
        "延迟/丢包/阻断/MTU/DNS 投毒矩阵",
        rootless_script="scripts/test-w12-interference-netns.sh",
        timeout_seconds=1800,
    ),
    Suite(
        "w12-exit-local",
        "Exit",
        "本机 Exit 启动、回退与清理",
        rootless_script="scripts/test-w12-exit-local-netns.sh",
        timeout_seconds=1800,
    ),
    Suite(
        "w12-p2p-invite",
        "网络",
        "邀请入网与直接 P2P 建连",
        rootless_script="scripts/test-w12-p2p-invite-netns.sh",
        timeout_seconds=1800,
    ),
    Suite(
        "w13-relay-failure",
        "Relay",
        "Relay 故障切换与撤权",
        rootless_script="scripts/test-w13-relay-failure-netns.sh",
        timeout_seconds=1800,
    ),
    Suite(
        "w14-admin-recovery-netns",
        "Coordinator",
        "管理员恢复接管（nftables 分区变体，需真实 root 以便分区真正生效）",
        script="scripts/test-w14-admin-recovery-netns.sh",
        root_required=True,
        timeout_seconds=1800,
    ),
    Suite(
        "w14-admin-recovery-drill",
        "Coordinator",
        "管理员恢复多进程演练（离线 Trustee + 两次 SIGKILL 重启）",
        cargo_test=(
            "fuckipv6-coordinator",
            "w14_admin_recovery_multinode",
            "w14_admin_recovery_multinode_offline_trustee_drill",
        ),
        timeout_seconds=1800,
    ),
    Suite(
        "w14-coordinator-fault",
        "Coordinator",
        "Leader 崩溃与租约接管",
        rootless_script="scripts/test-w14-coordinator-fault-netns.sh",
        timeout_seconds=1800,
    ),
    Suite(
        "w15-file",
        "File",
        "上传下载、续传与撤权中断（需要先有 release 构建）",
        rootless_script="scripts/test-w15-file-netns.sh",
        requires_release_build=True,
        timeout_seconds=1800,
    ),
)


def classify(exit_code: int | None, timed_out: bool) -> str:
    if timed_out:
        return "timeout"
    if exit_code == 0:
        return "passed"
    if exit_code == SKIP_STATUS:
        return "skipped"
    return "failed"


def run_suite(suite: Suite, *, is_root: bool, timeout_override: int | None) -> dict:
    command = suite.command(is_root)
    base = {
        "name": suite.name,
        "domain": suite.domain,
        "description": suite.description,
    }
    # Check for the actual release binary, not cargo's deps directory: a cleaned or
    # relocated artifact tree can leave the binary without `deps/`.
    release_dir = PROJECT_ROOT / "target" / "release"
    release_binary = next(
        (
            candidate
            for candidate in release_dir.glob("fuckipv6*")
            if candidate.is_file() and candidate.suffix != ".d"
        ),
        None,
    )
    if suite.requires_release_build and release_binary is None:
        return {
            **base,
            "status": "skipped",
            "reason": "requires a release build: run `cargo build --release` first",
            "duration_seconds": 0.0,
            "exit_code": None,
            "tail": "",
        }
    if command is None:
        return {
            **base,
            "status": "skipped",
            "reason": "root is required and the runner is not root",
            "duration_seconds": 0.0,
            "exit_code": None,
            "tail": "",
        }
    timeout = timeout_override or suite.timeout_seconds
    started = time.monotonic()
    try:
        completed = subprocess.run(
            command,
            cwd=PROJECT_ROOT,
            capture_output=True,
            text=True,
            timeout=timeout,
        )
        exit_code = completed.returncode
        output = (completed.stdout or "") + (completed.stderr or "")
        timed_out = False
    except subprocess.TimeoutExpired as expired:
        exit_code = None
        output = ""
        for stream in (expired.stdout, expired.stderr):
            if isinstance(stream, bytes):
                output += stream.decode("utf-8", "replace")
            elif stream:
                output += stream
        timed_out = True
    duration = round(time.monotonic() - started, 1)
    status = classify(exit_code, timed_out)
    tail_lines = [line for line in output.strip().splitlines() if line.strip()][-6:]
    return {
        **base,
        "status": status,
        "duration_seconds": duration,
        "exit_code": exit_code,
        "reason": "" if status in {"passed", "skipped"} else "\n".join(tail_lines),
        "tail": "\n".join(tail_lines),
    }


def summarize(results: list[dict], *, require_full: bool) -> tuple[str, list[str]]:
    problems: list[str] = []
    for result in results:
        if result["status"] == "failed":
            problems.append(f"{result['name']} failed: {result.get('tail', '')}")
        if result["status"] == "timeout":
            problems.append(f"{result['name']} timed out")
        if require_full and result["status"] == "skipped":
            problems.append(f"{result['name']} was skipped: {result.get('reason', '')}")
    return ("failed" if problems else "passed", problems)


def render_markdown(report: dict) -> str:
    lines = [
        "# 故障注入证据套件",
        "",
        f"- 生成时间：`{report['generated_at']}`",
        f"- 结论：**{report['status']}**（passed={report['counts']['passed']}，skipped={report['counts']['skipped']}，"
        f"failed={report['counts']['failed']}，timeout={report['counts']['timeout']}）",
        f"- 是否 root：`{report['is_root']}`；是否要求全覆盖：`{report['require_full']}`",
        "",
        "| 套件 | 域 | 状态 | 耗时(s) | 说明 |",
        "|---|---|---|---|---|",
    ]
    for result in report["results"]:
        detail = result["description"] if result["status"] == "passed" else (
            result.get("reason") or result["description"]
        )
        detail = detail.replace("|", "\\|").replace("\n", " ")
        lines.append(
            f"| `{result['name']}` | {result['domain']} | {result['status']} | "
            f"{result['duration_seconds']} | {detail} |"
        )
    if report["problems"]:
        lines += ["", "## 阻断项", ""]
        lines += [f"- {problem}" for problem in report["problems"]]
    return "\n".join(lines) + "\n"


def is_running_as_root() -> bool:
    return hasattr(os, "geteuid") and os.geteuid() == 0


def main(argv: Iterable[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--only", action="append", default=[], help="suite name to run (repeatable)")
    parser.add_argument("--timeout-seconds", type=int, default=None, help="override every suite timeout")
    parser.add_argument("--require-full", action="store_true", help="treat skipped suites as failures")
    parser.add_argument(
        "--report",
        type=Path,
        default=PROJECT_ROOT / "target" / "fault-injection" / "report.json",
    )
    parser.add_argument("--list", action="store_true")
    args = parser.parse_args(list(argv) if argv is not None else None)

    selected = [suite for suite in SUITES if not args.only or suite.name in args.only]
    if args.list:
        for suite in selected:
            print(f"{suite.name:26s} {suite.domain:10s} {suite.description}")
        return 0
    if not selected:
        print("no suites selected", file=sys.stderr)
        return 2

    is_root = is_running_as_root()
    results = [
        run_suite(suite, is_root=is_root, timeout_override=args.timeout_seconds)
        for suite in selected
    ]
    status, problems = summarize(results, require_full=args.require_full)
    counts = {key: sum(1 for r in results if r["status"] == key) for key in
              ("passed", "skipped", "failed", "timeout")}
    report = {
        "schema": REPORT_SCHEMA,
        "generated_at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
        "status": status,
        "is_root": is_root,
        "require_full": args.require_full,
        "counts": counts,
        "results": results,
        "problems": problems,
    }
    args.report.parent.mkdir(parents=True, exist_ok=True)
    args.report.write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    args.report.with_suffix(".md").write_text(render_markdown(report), encoding="utf-8")

    for result in results:
        print(f"[suite] {result['name']}: {result['status']} ({result['duration_seconds']}s)")
    for problem in problems:
        print(f"[suite] {problem}", file=sys.stderr)
    print(f"fault-injection-suite: {status} ({counts})", file=sys.stderr)
    return 0 if status == "passed" else 1


if __name__ == "__main__":
    raise SystemExit(main())
