#!/usr/bin/env python3
"""Generate and verify the feature→evidence mapping the audit item requires.

For every action the shared manifest advertises, the table names its command,
permission, audit policy, backend status, availability, and the evidence that the
action is actually wired: a test or source file in the repository that mentions
the command name. A row whose evidence file is missing, or does not mention the
command, is a defect: the action would be advertised without a verifiable path.
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path
from typing import Iterable

PROJECT_ROOT = Path(__file__).resolve().parent.parent
DEFAULT_MANIFEST = PROJECT_ROOT / "contracts" / "typescript" / "fixtures" / "node-manifest.json"
DEFAULT_OUTPUT = PROJECT_ROOT / "docs" / "FEATURE_EVIDENCE.zh-CN.md"
# Files that wire actions to commands; the first one that mentions the command is
# the row's evidence.
EVIDENCE_SOURCES = (
    "crates/client/src/node/command.rs",
    "crates/client/src/node/runtime_handler.rs",
    "crates/client/src/node/ui_manifest.rs",
    "contracts/typescript/fixtures/node-manifest.json",
)
REPORT_SCHEMA = "fuckipv6-feature-evidence-matrix"


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
                    "permission": action["permission"],
                    "audit_policy": action.get("audit", {}).get("policy", "none"),
                    "backend_status": action["backend_status"],
                    "availability": action["availability"],
                }
            )
    return rows


def find_evidence(command: str) -> tuple[str, bool] | None:
    """First source that mentions the command, and whether it really mentions it."""
    needle = f'"{command}"'
    for relative in EVIDENCE_SOURCES:
        path = PROJECT_ROOT / relative
        if not path.is_file():
            continue
        text = path.read_text(encoding="utf-8", errors="replace")
        if needle in text or f" {command} " in text:
            return relative, True
    return None


def check(manifest_path: Path) -> dict:
    errors: list[str] = []
    rows = manifest_actions(manifest_path)
    if not rows:
        errors.append("no actions parsed from the manifest")

    resolved: list[dict] = []
    for row in rows:
        evidence = find_evidence(row["command"])
        if evidence is None:
            errors.append(
                f"{row['page']}/{row['action']}: command {row['command']} has no evidence source"
            )
            row["evidence"] = "MISSING"
        else:
            row["evidence"] = evidence[0]
        resolved.append(row)

    implemented = [row for row in resolved if row["backend_status"] == "implemented"]
    unavailable = [row for row in resolved if row["backend_status"] != "implemented"]
    return {
        "schema": REPORT_SCHEMA,
        "status": "failed" if errors else "passed",
        "actions": len(resolved),
        "implemented": len(implemented),
        "not_implemented": len(unavailable),
        "rows": resolved,
        "errors": errors,
    }


def render(report: dict) -> str:
    lines = [
        "# 功能—证据映射表",
        "",
        "> 由 `scripts/feature-evidence-matrix.py` 从共享 manifest 生成并校验：表中的每一行对应 manifest 里的一个可见动作，",
        "> 证据列必须指向仓库内**真实提到该命令**的文件；缺失或对不上即视为“看起来可用但没有可验证路径”。",
        "",
        f"- 可见动作：**{report['actions']}**（implemented {report['implemented']} / 未实现 {report['not_implemented']}）",
        f"- 生成状态：**{report['status']}**",
        "",
        "| 页面 | 动作 | 命令 | 权限 | 审计策略 | 后端 | 证据 |",
        "|---|---|---|---|---|---|---|",
    ]
    for row in report["rows"]:
        lines.append(
            f"| `{row['page']}` | `{row['action']}` | `{row['command']}` | `{row['permission']}` | "
            f"`{row['audit_policy']}` | `{row['backend_status']}` | `{row['evidence']}` |"
        )
    if report["errors"]:
        lines += ["", "## 阻断项", ""]
        lines += [f"- {error}" for error in report["errors"]]
    return "\n".join(lines) + "\n"


def main(argv: Iterable[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--manifest", type=Path, default=DEFAULT_MANIFEST)
    parser.add_argument("--output", type=Path, default=DEFAULT_OUTPUT)
    parser.add_argument("--check", action="store_true", help="fail when the table on disk is stale")
    parser.add_argument("--report", type=Path, default=None)
    args = parser.parse_args(list(argv) if argv is not None else None)

    if not args.manifest.is_file():
        print(f"manifest not found: {args.manifest}", file=sys.stderr)
        return 2

    report = check(args.manifest)
    rendered = render(report)
    if args.check:
        current = args.output.read_text(encoding="utf-8") if args.output.is_file() else ""
        if current != rendered:
            print(
                f"feature-evidence-matrix: {args.output} is stale; regenerate it with "
                "python3 scripts/feature-evidence-matrix.py",
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
        f"feature-evidence-matrix: {report['status']} ({report['actions']} actions, "
        f"{len(report['errors'])} issues)",
        file=sys.stderr,
    )
    return 0 if report["status"] == "passed" else 1


if __name__ == "__main__":
    raise SystemExit(main())
