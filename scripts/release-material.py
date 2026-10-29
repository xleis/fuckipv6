#!/usr/bin/env python3
"""Generate release SBOM, SHA256, and unsigned provenance sidecars.

The source inventory comes from Cargo metadata and Cargo.lock. This command
does not sign artifacts or produce a SLSA attestation.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import platform
import re
import subprocess
import sys
import tomllib
import uuid
from datetime import datetime, timezone
from pathlib import Path
from typing import Any
from urllib.parse import quote, urlsplit, urlunsplit

PROJECT_ROOT = Path(__file__).resolve().parent.parent
TOOL_VERSION = "1.0.0"
CDX_SPEC_VERSION = "1.5"
SPDX_VERSION = "SPDX-2.3"
SPDX_NAMESPACE = uuid.UUID("8fb7e81d-31bf-5f0b-8d6c-62f2e5467cbf")


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def iso8601(epoch: int) -> str:
    return datetime.fromtimestamp(epoch, timezone.utc).isoformat(timespec="seconds").replace(
        "+00:00", "Z"
    )


def purl(name: str, version: str) -> str:
    return f"pkg:cargo/{quote(name, safe='-._')}@{quote(version, safe='-._+')}"


def spdx_id(name: str, version: str, package_id: str) -> str:
    readable = re.sub(r"[^A-Za-z0-9.-]+", "-", f"{name}-{version}").strip("-")
    suffix = hashlib.sha256(package_id.encode("utf-8")).hexdigest()[:12]
    return f"SPDXRef-Package-{readable}-{suffix}"


def cargo_command(arguments: list[str]) -> str:
    completed = subprocess.run(
        [os.environ.get("CARGO", "cargo"), *arguments],
        cwd=PROJECT_ROOT,
        check=False,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )
    if completed.returncode != 0:
        raise RuntimeError(completed.stderr.strip() or "cargo command failed")
    return completed.stdout


def git_output(*arguments: str) -> str:
    completed = subprocess.run(
        ["git", *arguments],
        cwd=PROJECT_ROOT,
        check=False,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )
    if completed.returncode != 0:
        raise RuntimeError(completed.stderr.strip() or "git command failed")
    return completed.stdout.strip()


def normalized_remote(remote: str | None) -> str | None:
    if not remote:
        return None
    try:
        parsed = urlsplit(remote)
    except ValueError:
        return None
    if not parsed.scheme or not parsed.netloc:
        return None
    hostname = parsed.hostname or ""
    port = f":{parsed.port}" if parsed.port else ""
    return urlunsplit((parsed.scheme, f"{hostname}{port}", parsed.path, "", ""))


def source_repository() -> str | None:
    server = os.environ.get("GITHUB_SERVER_URL")
    repository = os.environ.get("GITHUB_REPOSITORY")
    if server and repository:
        return f"{server.rstrip('/')}/{repository}"
    try:
        remote = git_output("remote", "get-url", "origin")
    except RuntimeError:
        return None
    return normalized_remote(remote)


def load_lock(path: Path) -> dict[tuple[str, str], dict[str, Any]]:
    with path.open("rb") as source:
        document = tomllib.load(source)
    return {
        (str(package["name"]), str(package["version"])): package
        for package in document.get("package", [])
    }


def metadata_for(target: str) -> dict[str, Any]:
    output = cargo_command(
        [
            "metadata",
            "--locked",
            "--format-version",
            "1",
            "--filter-platform",
            target,
        ]
    )
    return json.loads(output)


def component_from(metadata_package: dict[str, Any], lock: dict[tuple[str, str], dict[str, Any]]) -> dict[str, Any]:
    name = metadata_package["name"]
    version = metadata_package["version"]
    reference = purl(name, version)
    component: dict[str, Any] = {
        "type": "library",
        "bom-ref": reference,
        "name": name,
        "version": version,
        "purl": reference,
    }
    license_expression = metadata_package.get("license")
    if license_expression:
        component["licenses"] = [{"expression": license_expression}]
    source = metadata_package.get("source")
    lock_package = lock.get((name, version), {})
    checksum = lock_package.get("checksum")
    if checksum:
        component["hashes"] = [{"alg": "SHA-256", "content": checksum}]
    properties = []
    if source:
        properties.append({"name": "cargo:source", "value": source})
    if checksum:
        properties.append({"name": "cargo:checksum", "value": checksum})
    if metadata_package.get("manifest_path"):
        properties.append(
            {"name": "cargo:manifest-path", "value": metadata_package["manifest_path"]}
        )
    if properties:
        component["properties"] = properties
    return component


def release_graph(metadata: dict[str, Any]) -> tuple[dict[str, Any], dict[str, Any], list[str], dict[str, list[str]]]:
    packages = {package["id"]: package for package in metadata["packages"]}
    all_nodes = {node["id"]: node for node in metadata.get("resolve", {}).get("nodes", [])}
    workspace_ids = set(metadata.get("workspace_members", []))

    binary_members = [
        package_id
        for package_id in workspace_ids
        if package_id in packages
        and any("bin" in target.get("kind", []) for target in packages[package_id].get("targets", []))
    ]
    root_ids = sorted(binary_members or workspace_ids, key=lambda item: packages[item]["name"])

    def runtime_edge(parent_id: str, dependency_id: str) -> bool:
        parent = packages[parent_id]
        dependency = packages[dependency_id]
        return any(
            declaration.get("name") == dependency["name"]
            and declaration.get("kind") != "dev"
            for declaration in parent.get("dependencies", [])
        )

    reachable: set[str] = set()
    pending = list(root_ids)
    edges: dict[str, list[str]] = {}
    while pending:
        package_id = pending.pop(0)
        if package_id in reachable:
            continue
        reachable.add(package_id)
        dependencies = sorted(
            dependency_id
            for dependency_id in all_nodes.get(package_id, {}).get("dependencies", [])
            if dependency_id in packages and runtime_edge(package_id, dependency_id)
        )
        edges[package_id] = dependencies
        pending.extend(dependency_id for dependency_id in dependencies if dependency_id not in reachable)

    nodes = {package_id: all_nodes[package_id] for package_id in reachable}
    return packages, nodes, root_ids, edges


def build_cyclonedx(
    metadata: dict[str, Any],
    lock: dict[tuple[str, str], dict[str, Any]],
    *,
    target: str,
    artifact_name: str,
    release_version: str,
    commit: str,
    created: str,
    source_repo: str | None,
) -> dict[str, Any]:
    packages, nodes, root_ids, edges = release_graph(metadata)

    components = [
        component_from(packages[package_id], lock)
        for package_id in sorted(nodes)
        if package_id in packages
    ]
    components.sort(key=lambda component: component["bom-ref"])

    root_ref = f"pkg:cargo/fuckipv6-release@{quote(release_version, safe='-._+')}"
    dependencies = []
    root_dependencies = [
        purl(packages[package_id]["name"], packages[package_id]["version"])
        for package_id in root_ids
    ]
    dependencies.append({"ref": root_ref, "dependsOn": sorted(root_dependencies)})
    for package_id in sorted(nodes):
        if package_id not in packages:
            continue
        package = packages[package_id]
        depends_on = sorted(
            purl(packages[dependency]["name"], packages[dependency]["version"])
            for dependency in edges.get(package_id, [])
        )
        dependencies.append(
            {
                "ref": purl(package["name"], package["version"]),
                "dependsOn": depends_on,
            }
        )

    namespace = source_repo or "https://fuckipv6.invalid"
    serial = uuid.uuid5(
        SPDX_NAMESPACE, f"{namespace}:{commit}:{target}:{artifact_name}:{release_version}"
    )
    root_component: dict[str, Any] = {
        "type": "application",
        "bom-ref": root_ref,
        "name": "fuckipv6-release",
        "version": release_version,
        "description": f"fuckipv6 release artifact {artifact_name} for {target}",
        "properties": [
            {"name": "fuckipv6:artifact", "value": artifact_name},
            {"name": "fuckipv6:target", "value": target},
            {"name": "fuckipv6:source-commit", "value": commit},
        ],
    }
    if source_repo:
        root_component["externalReferences"] = [
            {"type": "vcs", "url": f"{source_repo.rstrip('/')}/tree/{commit}"}
        ]
    return {
        "bomFormat": "CycloneDX",
        "specVersion": CDX_SPEC_VERSION,
        "serialNumber": f"urn:uuid:{serial}",
        "version": 1,
        "metadata": {
            "timestamp": created,
            "tools": {
                "components": [
                    {
                        "type": "application",
                        "name": "fuckipv6-release-material",
                        "version": TOOL_VERSION,
                    }
                ]
            },
            "component": root_component,
        },
        "components": components,
        "dependencies": dependencies,
    }


def build_spdx(
    metadata: dict[str, Any],
    lock: dict[tuple[str, str], dict[str, Any]],
    *,
    target: str,
    artifact_name: str,
    release_version: str,
    commit: str,
    created: str,
    source_repo: str | None,
) -> dict[str, Any]:
    packages_by_id, nodes, root_ids, edges = release_graph(metadata)

    namespace_seed = source_repo or "https://fuckipv6.invalid"
    namespace_uuid = uuid.uuid5(
        SPDX_NAMESPACE, f"{namespace_seed}:{commit}:{target}:{artifact_name}:{release_version}"
    )
    root_spdx_id = "SPDXRef-Package-fuckipv6-release"
    spdx_packages = [
        {
            "SPDXID": root_spdx_id,
            "name": "fuckipv6-release",
            "versionInfo": release_version,
            "downloadLocation": "NOASSERTION",
            "filesAnalyzed": False,
            "licenseConcluded": "NOASSERTION",
            "licenseDeclared": "NOASSERTION",
            "copyrightText": "NOASSERTION",
            "comment": f"Artifact {artifact_name} for target {target}; source commit {commit}",
        }
    ]
    package_refs: dict[str, str] = {}
    for package_id in sorted(nodes):
        package = packages_by_id.get(package_id)
        if package is None:
            continue
        name = package["name"]
        version = package["version"]
        reference = spdx_id(name, version, package_id)
        package_refs[package_id] = reference
        lock_package = lock.get((name, version), {})
        source = package.get("source")
        if source and source.startswith("registry+"):
            download_location = (
                f"https://crates.io/api/v1/crates/{quote(name, safe='-._')}/"
                f"{quote(version, safe='-._+')}/download"
            )
        else:
            download_location = "NOASSERTION"
        external_refs = [
            {
                "referenceCategory": "PACKAGE-MANAGER",
                "referenceType": "purl",
                "referenceLocator": purl(name, version),
            }
        ]
        spdx_package: dict[str, Any] = {
            "SPDXID": reference,
            "name": name,
            "versionInfo": version,
            "downloadLocation": download_location,
            "filesAnalyzed": False,
            "licenseConcluded": "NOASSERTION",
            "licenseDeclared": package.get("license") or "NOASSERTION",
            "copyrightText": "NOASSERTION",
            "externalRefs": external_refs,
        }
        checksum = lock_package.get("checksum")
        if checksum:
            spdx_package["checksums"] = [{"algorithm": "SHA256", "checksumValue": checksum}]
        if source:
            spdx_package["sourceInfo"] = source
        spdx_packages.append(spdx_package)

    relationships = [{"spdxElementId": "SPDXRef-DOCUMENT", "relationshipType": "DESCRIBES", "relatedSpdxElement": root_spdx_id}]
    for package_id in root_ids:
        if package_id in package_refs:
            relationships.append(
                {
                    "spdxElementId": root_spdx_id,
                    "relationshipType": "CONTAINS",
                    "relatedSpdxElement": package_refs[package_id],
                }
            )
    for package_id in sorted(nodes):
        if package_id not in package_refs:
            continue
        for dependency in edges.get(package_id, []):
            if dependency in package_refs:
                relationships.append(
                    {
                        "spdxElementId": package_refs[package_id],
                        "relationshipType": "DEPENDS_ON",
                        "relatedSpdxElement": package_refs[dependency],
                    }
                )
    relationships.sort(key=lambda item: (item["spdxElementId"], item["relatedSpdxElement"]))

    return {
        "spdxVersion": SPDX_VERSION,
        "dataLicense": "CC0-1.0",
        "SPDXID": "SPDXRef-DOCUMENT",
        "name": f"fuckipv6-{release_version}-{target}-{artifact_name}",
        "documentNamespace": f"https://spdx.org/spdxdocs/fuckipv6-{namespace_uuid}",
        "creationInfo": {
            "created": created,
            "creators": [f"Tool: fuckipv6-release-material-{TOOL_VERSION}"],
            "comment": "Dependency inventory; not a cryptographic audit or signed attestation.",
        },
        "packages": spdx_packages,
        "relationships": relationships,
    }


def build_provenance(
    *,
    artifact: Path,
    artifact_sha256: str,
    target: str,
    release_version: str,
    source: dict[str, Any],
    lock_sha256: str,
    created: str,
) -> dict[str, Any]:
    workflow = {
        key.removeprefix("GITHUB_"): os.environ[key]
        for key in (
            "GITHUB_WORKFLOW_REF",
            "GITHUB_RUN_ID",
            "GITHUB_RUN_ATTEMPT",
            "GITHUB_JOB",
            "GITHUB_REF",
            "GITHUB_SHA",
        )
        if os.environ.get(key)
    }
    builder = "https://github.com/actions/runner" if os.environ.get("GITHUB_ACTIONS") else "local"
    return {
        "schema": "fuckipv6.provenance.v1",
        "created": created,
        "release_version": release_version,
        "artifact": {
            "name": artifact.name,
            "sha256": artifact_sha256,
            "target": target,
        },
        "source": source,
        "materials": {
            "cargo_lock_sha256": lock_sha256,
            "rustc": cargo_command(["--version"]).strip(),
        },
        "builder": {
            "id": builder,
            "host_os": platform.system(),
            "host_arch": platform.machine(),
            "workflow": workflow,
        },
        "verification": {
            "signed": False,
            "slsa_attestation": False,
            "statement": "Unsigned provenance metadata generated from the checked-out source tree.",
        },
    }


def write_json(path: Path, document: dict[str, Any]) -> None:
    path.write_text(json.dumps(document, indent=2, sort_keys=True) + "\n", encoding="utf-8")


def write_manifest(path: Path, entries: list[tuple[str, str]]) -> None:
    lines = [f"{digest}  {name}" for name, digest in sorted(entries)]
    path.write_text("\n".join(lines) + "\n", encoding="utf-8")


def material_prefix(artifact: Path) -> str:
    name = artifact.name
    for suffix in (".tar.gz", ".tar.xz", ".zip"):
        if name.endswith(suffix):
            return name[: -len(suffix)]
    return artifact.stem


def generate(args: argparse.Namespace) -> dict[str, Path]:
    artifact = args.artifact.resolve()
    if not artifact.is_file():
        raise ValueError(f"artifact does not exist: {artifact}")
    output_dir = args.output_dir.resolve() if args.output_dir else artifact.parent
    output_dir.mkdir(parents=True, exist_ok=True)

    status = git_output("status", "--porcelain", "--untracked-files=normal")
    if status and not args.allow_dirty:
        raise ValueError(
            "source tree is dirty; commit changes or pass --allow-dirty for non-release diagnostics"
        )
    commit = git_output("rev-parse", "HEAD")
    tree = git_output("rev-parse", "HEAD^{tree}")
    commit_epoch = int(git_output("show", "-s", "--format=%ct", "HEAD"))
    source_epoch = int(os.environ.get("SOURCE_DATE_EPOCH", commit_epoch))
    created = iso8601(source_epoch)
    lock_path = PROJECT_ROOT / "Cargo.lock"
    lock_sha256 = sha256_file(lock_path)
    artifact_sha256 = sha256_file(artifact)
    metadata = metadata_for(args.target)
    lock = load_lock(lock_path)
    repo = args.source_repository or source_repository()
    source = {
        "repository": repo,
        "commit": commit,
        "tree": tree,
        "dirty": bool(status),
        "release_version": args.release_version,
        "target": args.target,
        "platform": args.platform,
    }

    prefix = output_dir / material_prefix(artifact)
    cdx_path = Path(f"{prefix}.sbom.cdx.json")
    spdx_path = Path(f"{prefix}.sbom.spdx.json")
    provenance_path = Path(f"{prefix}.provenance.json")
    sha_path = Path(f"{artifact}.sha256")
    manifest_path = Path(f"{prefix}.SHA256SUMS")

    cdx = build_cyclonedx(
        metadata,
        lock,
        target=args.target,
        artifact_name=artifact.name,
        release_version=args.release_version,
        commit=commit,
        created=created,
        source_repo=repo,
    )
    spdx = build_spdx(
        metadata,
        lock,
        target=args.target,
        artifact_name=artifact.name,
        release_version=args.release_version,
        commit=commit,
        created=created,
        source_repo=repo,
    )
    provenance = build_provenance(
        artifact=artifact,
        artifact_sha256=artifact_sha256,
        target=args.target,
        release_version=args.release_version,
        source=source,
        lock_sha256=lock_sha256,
        created=created,
    )
    write_json(cdx_path, cdx)
    write_json(spdx_path, spdx)
    write_json(provenance_path, provenance)
    sha_path.write_text(f"{artifact_sha256}  {artifact.name}\n", encoding="utf-8")

    entries = [(path.name, sha256_file(path)) for path in (artifact, cdx_path, spdx_path, provenance_path)]
    write_manifest(manifest_path, entries)
    return {
        "artifact": artifact,
        "cyclonedx": cdx_path,
        "spdx": spdx_path,
        "provenance": provenance_path,
        "sha256": sha_path,
        "manifest": manifest_path,
    }


def parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--artifact", required=True, type=Path, help="release archive to describe")
    parser.add_argument("--output-dir", type=Path, help="sidecar directory (default: artifact directory)")
    parser.add_argument("--target", required=True, help="Rust target triple")
    parser.add_argument("--platform", required=True, help="release platform label")
    parser.add_argument("--release-version", required=True, help="release tag or version")
    parser.add_argument("--source-repository", help="public source repository URL")
    parser.add_argument(
        "--allow-dirty",
        action="store_true",
        help="diagnostic mode: mark provenance dirty instead of refusing to generate",
    )
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    try:
        outputs = generate(parse_args(sys.argv[1:] if argv is None else argv))
    except (OSError, RuntimeError, ValueError, json.JSONDecodeError) as error:
        print(f"error: {error}", file=sys.stderr)
        return 1
    for label, path in outputs.items():
        print(f"{label}: {path}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
