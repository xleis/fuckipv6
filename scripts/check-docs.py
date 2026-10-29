#!/usr/bin/env python3
"""Check tracked Markdown links, anchors, and critical status invariants.

External URLs are syntax-checked but never fetched. The status checks encode
the project's current release boundary: Secure Session v2 classic is in the
production data plane, but it lacks independent audit and post-quantum
integration, and the project has not proven parity with WireGuard.
"""

from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Iterable
from urllib.parse import unquote, urlsplit

REPORT_SCHEMA = "fuckipv6-doc-consistency-report"
SCHEMA_VERSION = 1
EXTERNAL_SCHEMES = {"http", "https", "mailto", "ftp", "ftps"}
PREREQUISITES = {
    "EQ-12": ("EQ-11",),
    "EQ-13": ("EQ-04", "EQ-05", "EQ-06", "EQ-09"),
    "EQ-14": ("EQ-11",),
    "EQ-16": ("EQ-15",),
}
SECURITY_INVARIANTS = (
    (
        "readme-v2-production-boundary",
        "README.md",
        r"Secure Session v2 classic[^\n]*UDP/TUN[^\n]*尚未完成独立密码学审计[^\n]*后量子",
        "README must distinguish production v2 from its audit and PQ boundaries",
    ),
    (
        "readme-no-wireguard-performance-claim",
        "README.md",
        r"不是超越 WireGuard 的证明",
        "README must not turn current microbenchmarks into a WireGuard performance claim",
    ),
    (
        "todo-production-v2-audit-boundary",
        "SECURITY_PERFORMANCE_TODO.md",
        r"Secure Session v2[^\n]*生产\s*`P2pEngine`[^\n]*独立(?:协议与)?密码学审计",
        "security TODO must record production v2 integration and its audit boundary",
    ),
    (
        "changelog-pq-boundary",
        "CHANGELOG.md",
        r"不得据此宣称已具备后量子安全或超越 WireGuard",
        "changelog must keep the PQ primitive and WireGuard boundary explicit",
    ),
)


def utc_now() -> str:
    return datetime.now(timezone.utc).isoformat(timespec="seconds").replace("+00:00", "Z")


def write_json(path: Path, document: dict[str, Any]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_suffix(path.suffix + ".tmp")
    temporary.write_text(
        json.dumps(document, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    temporary.replace(path)


def tracked_markdown_files(root: Path) -> list[Path]:
    try:
        completed = subprocess.run(
            ["git", "ls-files", "-z", "--", "*.md"],
            cwd=root,
            check=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
        )
        paths = [
            root / item.decode("utf-8")
            for item in completed.stdout.split(b"\0")
            if item
        ]
        if paths:
            return sorted(path for path in paths if path.is_file())
    except (OSError, subprocess.CalledProcessError, UnicodeDecodeError):
        pass
    ignored = {".git", "target", "dist"}
    return sorted(
        path
        for path in root.rglob("*.md")
        if path.is_file() and not any(part in ignored for part in path.relative_to(root).parts)
    )


def _without_fences(text: str) -> str:
    output: list[str] = []
    fence: str | None = None
    for line in text.splitlines(keepends=True):
        stripped = line.lstrip()
        marker = "```" if stripped.startswith("```") else "~~~" if stripped.startswith("~~~") else None
        if marker is not None:
            if fence is None:
                fence = marker
            elif fence == marker:
                fence = None
            output.append("\n" if line.endswith("\n") else "")
            continue
        output.append(("" if fence is not None else line))
    return "".join(output)


def _without_inline_code(text: str) -> str:
    return re.sub(r"`[^`\n]*`", lambda match: " " * len(match.group(0)), text)


def _heading_plain_text(value: str) -> str:
    value = re.sub(r"!?\[([^\]]*)\]\([^)]*\)", r"\1", value)
    value = re.sub(r"<[^>]+>", "", value)
    return re.sub(r"[`*_~]", "", value).strip()


def _slug(value: str) -> str:
    value = _heading_plain_text(value).lower()
    value = re.sub(r"[^\w\s-]", "", value, flags=re.UNICODE)
    return re.sub(r"\s+", "-", value.strip())


def heading_anchors(text: str) -> set[str]:
    anchors: set[str] = set()
    duplicates: dict[str, int] = {}
    for line in _without_fences(text).splitlines():
        match = re.match(r"^\s{0,3}#{1,6}\s+(.+?)\s*#*\s*$", line)
        if match is None:
            continue
        base = _slug(match.group(1))
        count = duplicates.get(base, 0)
        anchor = base if count == 0 else f"{base}-{count}"
        duplicates[base] = count + 1
        anchors.add(anchor)
    return anchors


def _inline_links(text: str) -> Iterable[tuple[int, str]]:
    cleaned = _without_inline_code(_without_fences(text))
    pattern = re.compile(
        r"!?\[[^\]]*\]\(\s*(?:<([^>]+)>|([^\s)]+))(?:\s+[^)]*)?\)"
    )
    for match in pattern.finditer(cleaned):
        line = cleaned.count("\n", 0, match.start()) + 1
        yield line, match.group(1) or match.group(2)


def _reference_links(text: str) -> Iterable[tuple[int, str]]:
    cleaned = _without_inline_code(_without_fences(text))
    pattern = re.compile(
        r"^[ \t]{0,3}\[[^\]]+\]:[ \t]*(?:<([^>]+)>|(\S+))", re.MULTILINE
    )
    for match in pattern.finditer(cleaned):
        line = cleaned.count("\n", 0, match.start()) + 1
        yield line, match.group(1) or match.group(2)


def _issue(
    code: str, source: Path, line: int | None, message: str, target: str | None = None
) -> dict[str, Any]:
    result: dict[str, Any] = {
        "code": code,
        "source": str(source),
        "line": line,
        "message": message,
    }
    if target is not None:
        result["target"] = target
    return result


def _resolve_link(
    source: Path,
    target: str,
    root: Path,
    anchors: dict[Path, set[str]],
) -> dict[str, Any] | None:
    parsed = urlsplit(target)
    if parsed.scheme:
        if parsed.scheme.lower() == "file":
            return _issue(
                "nonportable-file-url",
                source.relative_to(root),
                None,
                "file:// links are host-specific; use a repository-relative path",
                target,
            )
        if parsed.scheme.lower() not in EXTERNAL_SCHEMES:
            return _issue(
                "unknown-url-scheme",
                source.relative_to(root),
                None,
                f"unsupported URL scheme: {parsed.scheme}",
                target,
            )
        return None
    if not parsed.path:
        destination = source
    elif parsed.path.startswith("/"):
        return _issue(
            "absolute-local-path",
            source.relative_to(root),
            None,
            "absolute local paths are not portable; use a repository-relative path",
            target,
        )
    else:
        destination = (source.parent / unquote(parsed.path)).resolve()
    try:
        destination.relative_to(root)
    except ValueError:
        return _issue(
            "path-escapes-root",
            source.relative_to(root),
            None,
            "relative link escapes the repository root",
            target,
        )
    if not destination.exists():
        return _issue(
            "broken-link",
            source.relative_to(root),
            None,
            "linked path does not exist",
            target,
        )
    if parsed.fragment and destination.suffix.lower() == ".md":
        fragment = unquote(parsed.fragment).lower()
        destination_anchors = anchors.get(destination)
        if destination_anchors is None:
            destination_anchors = heading_anchors(destination.read_text(encoding="utf-8"))
            anchors[destination] = destination_anchors
        if fragment not in destination_anchors:
            return _issue(
                "broken-anchor",
                source.relative_to(root),
                None,
                "linked Markdown heading anchor does not exist",
                target,
            )
    return None


def check_links(root: Path, files: list[Path]) -> tuple[int, list[dict[str, Any]]]:
    anchors: dict[Path, set[str]] = {}
    issues: list[dict[str, Any]] = []
    link_count = 0
    for path in files:
        relative = path.relative_to(root)
        text = path.read_text(encoding="utf-8")
        anchors[path.resolve()] = heading_anchors(text)
        for line, target in [*_inline_links(text), *_reference_links(text)]:
            link_count += 1
            issue = _resolve_link(path.resolve(), target, root.resolve(), anchors)
            if issue is not None:
                issue["source"] = str(relative)
                issue["line"] = line
                issues.append(issue)
    return link_count, issues


def check_security_invariants(root: Path) -> list[dict[str, Any]]:
    issues: list[dict[str, Any]] = []
    for code, relative, pattern, message in SECURITY_INVARIANTS:
        path = root / relative
        try:
            text = path.read_text(encoding="utf-8")
        except OSError:
            issues.append(_issue(code, Path(relative), None, "required document is missing"))
            continue
        if re.search(pattern, text, flags=re.IGNORECASE) is None:
            issues.append(_issue(code, Path(relative), None, message))
    return issues


def check_execution_status(root: Path) -> list[dict[str, Any]]:
    path = root / "EXECUTION_QUEUE.md"
    try:
        text = path.read_text(encoding="utf-8")
    except OSError:
        return [_issue("missing-execution-queue", Path("EXECUTION_QUEUE.md"), None, "required document is missing")]
    found: dict[str, list[tuple[int, str]]] = {}
    pattern = re.compile(r"^\s*-\s*\[([ x~])\]\s*\*\*(EQ-\d{2})\b", re.MULTILINE)
    for match in pattern.finditer(text):
        line = text.count("\n", 0, match.start()) + 1
        found.setdefault(match.group(2), []).append((line, match.group(1)))
    issues: list[dict[str, Any]] = []
    for item, entries in sorted(found.items()):
        if len(entries) > 1:
            issues.append(
                _issue(
                    "duplicate-status",
                    Path("EXECUTION_QUEUE.md"),
                    entries[1][0],
                    f"{item} has more than one status entry",
                )
            )
    statuses = {item: entries[0][1] for item, entries in found.items()}
    for item, dependencies in PREREQUISITES.items():
        if statuses.get(item) != "x":
            continue
        for dependency in dependencies:
            if statuses.get(dependency) != "x":
                issues.append(
                    _issue(
                        "premature-completion",
                        Path("EXECUTION_QUEUE.md"),
                        found[item][0][0],
                        f"{item} is complete while prerequisite {dependency} is not",
                    )
                )
    return issues


def check_documents(root: Path, files: list[Path] | None = None) -> dict[str, Any]:
    root = root.resolve()
    markdown_files = files if files is not None else tracked_markdown_files(root)
    markdown_files = [path.resolve() for path in markdown_files]
    link_count, link_issues = check_links(root, markdown_files)
    issues = [*link_issues, *check_security_invariants(root), *check_execution_status(root)]
    return {
        "schema": REPORT_SCHEMA,
        "schema_version": SCHEMA_VERSION,
        "generated_at": utc_now(),
        "root": str(root),
        "files_checked": len(markdown_files),
        "links_checked": link_count,
        "issues": issues,
        "summary": {
            "status": "passed" if not issues else "failed",
            "issue_count": len(issues),
        },
    }


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, default=Path(__file__).resolve().parent.parent)
    parser.add_argument("--report", type=Path, help="write a machine-readable JSON report")
    return parser


def main(argv: list[str] | None = None) -> int:
    try:
        args = build_parser().parse_args(argv)
        document = check_documents(args.root)
        if args.report:
            write_json(args.report, document)
        summary = document["summary"]
        print(
            f"check-docs: {summary['status']} ({document['files_checked']} files, "
            f"{document['links_checked']} links, {summary['issue_count']} issues)"
        )
        for issue in document["issues"]:
            location = issue["source"]
            if issue.get("line") is not None:
                location += f":{issue['line']}"
            target = f" -> {issue['target']}" if issue.get("target") else ""
            print(f"check-docs: {location}: {issue['message']}{target}", file=sys.stderr)
        return 0 if summary["status"] == "passed" else 1
    except (OSError, UnicodeError, ValueError) as error:
        print(f"check-docs: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
