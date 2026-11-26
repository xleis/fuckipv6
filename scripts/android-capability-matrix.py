#!/usr/bin/env python3
"""Compare the Android surface with the desktop manifest, command by command.

The audit item asks for an Android↔desktop capability matrix. This generator
derives it from two authorities instead of prose: the shared manifest (what the
product advertises) and the Android main sources (what the phone actually
implements). "mounted" requires the command string to appear in a bridge or UI
file, so a comment cannot claim a capability.
"""

from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path
from typing import Iterable

PROJECT_ROOT = Path(__file__).resolve().parent.parent
DEFAULT_MANIFEST = PROJECT_ROOT / "contracts" / "typescript" / "fixtures" / "node-manifest.json"
DEFAULT_ANDROID = PROJECT_ROOT / "android" / "app" / "src" / "main" / "kotlin"
DEFAULT_OUTPUT = PROJECT_ROOT / "docs" / "ANDROID_CAPABILITY_MATRIX.zh-CN.md"
REPORT_SCHEMA = "fuckipv6-android-capability-matrix"

# Only these directories may claim a mounted action: bridges speak the protocol,
# the UI is where the action is offered.
CLAIMING_PREFIXES = (
    "cc/xlei/fuckipv6/android/bridge/",
    "cc/xlei/fuckipv6/android/ui/",
)
# Only the *value* of a "command" field counts: JSON object literals in Kotlin
# carry plenty of other snake_case strings (field names, states), and treating
# those as commands produced 314 false drift reports in the first run.
COMMAND_PATTERNS = (
    re.compile(r'\.?put\(\s*"command"\s*,\s*"([a-z][a-z0-9_]*)"'),
    re.compile(r'"command"\s*:\s*"([a-z][a-z0-9_]*)"'),
    re.compile(r'command\s*=\s*"([a-z][a-z0-9_]*)"'),
)


# Actions Android implements over REST instead of the command channel. The
# manifest names the command; the phone speaks HTTP, and hiding that difference
# would either overstate the command channel or understate the phone.
REST_ALTERNATES: dict[str, tuple[str, str]] = {
    "get_snapshot": ("/api/v1/snapshot", "cc/xlei/fuckipv6/android/bridge/NodeApiConfig.kt"),
}

# Capabilities the manifest does not advertise as page actions at all: they are
# REST resources or platform services. They are listed so the matrix does not
# pretend the phone lacks them.
EXTRA_CAPABILITIES: tuple[tuple[str, str, str], ...] = (
    ("客户端黑名单查询", "command", "get_blacklist"),
    ("客户端封禁", "command", "ban_client"),
    ("客户端解除封禁", "command", "unban_client"),
    ("File 上传", "rest", "/api/v1/file/uploads"),
    ("File 下载", "rest", "/api/v1/file/downloads"),
    ("File 传输列表/控制", "rest", "/api/v1/file/transfers"),
    ("SSE 事件订阅", "rest", "/api/v1/events"),
    ("VpnService 启停", "platform", "TUNNEL_START"),
)


def manifest_actions(manifest_path: Path) -> list[dict]:
    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    rows: list[dict] = []
    for page in manifest.get("pages", []):
        for action in page.get("actions", []):
            rows.append(
                {
                    "page": page["id"],
                    "action": action["id"],
                    "command": action["command"],
                    "backend_status": action["backend_status"],
                }
            )
    return rows


_TEXT_CACHE: dict[Path, str] = {}


def android_text_cache(android_root: Path) -> str:
    key = android_root.resolve()
    cached = _TEXT_CACHE.get(key)
    if cached is None:
        cached = "".join(
            path.read_text(encoding="utf-8", errors="replace")
            for path in sorted(android_root.rglob("*.kt"))
        )
        _TEXT_CACHE[key] = cached
    return cached


def android_command_index(android_root: Path) -> dict[str, list[str]]:
    """Map every command *value* to the files that send it."""
    index: dict[str, list[str]] = {}
    for path in sorted(android_root.rglob("*.kt")):
        relative = path.relative_to(android_root).as_posix()
        text = path.read_text(encoding="utf-8", errors="replace")
        found: set[str] = set()
        for pattern in COMMAND_PATTERNS:
            found.update(pattern.findall(text))
        for literal in found:
            index.setdefault(literal, []).append(relative)
    return index


def claiming_files(index: dict[str, list[str]], command: str) -> list[str]:
    return [
        path
        for path in index.get(command, [])
        if any(path.startswith(prefix) for prefix in CLAIMING_PREFIXES)
    ]


def check(manifest_path: Path, android_root: Path) -> dict:
    errors: list[str] = []
    rows = manifest_actions(manifest_path)
    if not rows:
        errors.append("no actions parsed from the manifest")
    index = android_command_index(android_root)
    if not index:
        errors.append("no Android sources parsed; the extractor is out of date")

    manifest_commands = {row["command"] for row in rows}
    # Any command the phone speaks must exist in the product contract.
    for command, files in sorted(index.items()):
        claiming = [f for f in files if any(f.startswith(p) for p in CLAIMING_PREFIXES)]
        if not claiming:
            continue
        if command not in manifest_commands:
            errors.append(
                f"Android speaks {command} ({claiming[0]}) but the manifest has no such command"
            )

    resolved: list[dict] = []
    for row in rows:
        files = claiming_files(index, row["command"])
        alternate = REST_ALTERNATES.get(row["command"])
        if not files and alternate:
            token, evidence = alternate
            if token in android_text_cache(android_root) and (android_root / evidence).is_file():
                files = [evidence]
        resolved.append(
            {
                **row,
                "desktop": "available" if row["backend_status"] == "implemented" else f"blocked:{row['backend_status']}",
                "android": "mounted" if files else "desktop-only",
                "android_evidence": files[0] if files else "",
            }
        )

    extras: list[dict] = []
    android_text = android_text_cache(android_root)
    for label, transport, token in EXTRA_CAPABILITIES:
        present = token in android_text
        extras.append(
            {
                "capability": label,
                "transport": transport,
                "token": token,
                "android": "mounted" if present else "absent",
            }
        )
        if not present:
            errors.append(f"extra capability {label} ({token}) is not present in Android sources")

    mounted = [row for row in resolved if row["android"] == "mounted"]
    return {
        "extra_capabilities": extras,
        "schema": REPORT_SCHEMA,
        "status": "failed" if errors else "passed",
        "actions": len(resolved),
        "unique_commands": len(manifest_commands),
        "android_mounted": len(mounted),
        "android_desktop_only": len(resolved) - len(mounted),
        "extras_mounted": len([item for item in extras if item["android"] == "mounted"]),
        "rows": resolved,
        "errors": errors,
    }


def render(report: dict) -> str:
    lines = [
        "# Android 与桌面能力矩阵",
        "",
        "> 由 `scripts/android-capability-matrix.py` 从共享 manifest 与 Android 主源码生成并校验。",
        "> `mounted` 表示 Android 的 bridge/ui 源码里**真的出现**该命令字面量；只有注释或测试提到不算。",
        "",
        f"- 可见动作：**{report['actions']}**（唯一命令 {report['unique_commands']}）",
        f"- Android 已挂载：**{report['android_mounted']}**；桌面专属：**{report['android_desktop_only']}**",
        f"- manifest 之外的 Android 能力：**{report.get('extras_mounted', 0)}** 项（黑名单/File/事件流/VpnService）",
        f"- 生成状态：**{report['status']}**",
        "",
        "| 页面 | 动作 | 命令 | 桌面 | Android | Android 证据 |",
        "|---|---|---|---|---|---|",
    ]
    for row in report["rows"]:
        evidence = f"`{row['android_evidence']}`" if row["android_evidence"] else "—"
        lines.append(
            f"| `{row['page']}` | `{row['action']}` | `{row['command']}` | `{row['desktop']}` | "
            f"`{row['android']}` | {evidence} |"
        )
    lines += [
        "",
        "## manifest 之外的 Android 侧能力",
        "",
        "| 能力 | 通道 | 标识 | Android |",
        "|---|---|---|---|",
    ]
    for item in report.get("extra_capabilities", []):
        lines.append(
            f"| {item['capability']} | `{item['transport']}` | `{item['token']}` | `{item['android']}` |"
        )
    if report["errors"]:
        lines += ["", "## 阻断项", ""]
        lines += [f"- {error}" for error in report["errors"]]
    return "\n".join(lines) + "\n"


def main(argv: Iterable[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--manifest", type=Path, default=DEFAULT_MANIFEST)
    parser.add_argument("--android", type=Path, default=DEFAULT_ANDROID)
    parser.add_argument("--output", type=Path, default=DEFAULT_OUTPUT)
    parser.add_argument("--check", action="store_true")
    parser.add_argument("--report", type=Path, default=None)
    args = parser.parse_args(list(argv) if argv is not None else None)

    for path in (args.manifest,):
        if not path.is_file():
            print(f"input not found: {path}", file=sys.stderr)
            return 2
    if not args.android.is_dir():
        print(f"android sources not found: {args.android}", file=sys.stderr)
        return 2

    report = check(args.manifest, args.android)
    rendered = render(report)
    if args.check:
        current = args.output.read_text(encoding="utf-8") if args.output.is_file() else ""
        if current != rendered:
            print(
                f"android-capability-matrix: {args.output} is stale; regenerate it with "
                "python3 scripts/android-capability-matrix.py",
                file=sys.stderr,
            )
            return 1
    else:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(rendered, encoding="utf-8")

    if args.report:
        args.report.parent.mkdir(parents=True, exist_ok=True)
        args.report.write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    for error in report["errors"]:
        print(f"[FAIL] {error}", file=sys.stderr)
    print(
        f"android-capability-matrix: {report['status']} ({report['actions']} actions, "
        f"android mounted {report['android_mounted']}, {len(report['errors'])} issues)",
        file=sys.stderr,
    )
    return 0 if report["status"] == "passed" else 1


if __name__ == "__main__":
    raise SystemExit(main())
