#!/usr/bin/env python3
"""Tests for release-material.py."""

from __future__ import annotations

import importlib.util
import json
import tempfile
import unittest
from pathlib import Path

SCRIPT = Path(__file__).with_name("release-material.py")
SPEC = importlib.util.spec_from_file_location("release_material", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
MATERIAL = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MATERIAL)


def fixture() -> tuple[dict, dict]:
    metadata = {
        "workspace_members": ["path+root#root@0.3.0", "path+common#common@0.3.0"],
        "packages": [
            {
                "id": "path+root#root@0.3.0",
                "name": "root",
                "version": "0.3.0",
                "license": "MIT",
                "source": None,
                "manifest_path": "/repo/Cargo.toml",
                "targets": [{"kind": ["bin"], "name": "root"}],
                "dependencies": [{"name": "common", "kind": None}],
            },
            {
                "id": "path+common#common@0.3.0",
                "name": "common",
                "version": "0.3.0",
                "license": "MIT OR Apache-2.0",
                "source": None,
                "manifest_path": "/repo/common/Cargo.toml",
                "targets": [{"kind": ["lib"], "name": "common"}],
                "dependencies": [{"name": "dep", "kind": None}],
            },
            {
                "id": "registry+dep#dep@1.2.3",
                "name": "dep",
                "version": "1.2.3",
                "license": "Apache-2.0",
                "source": "registry+https://github.com/rust-lang/crates.io-index",
                "manifest_path": "/cargo/dep/Cargo.toml",
                "targets": [{"kind": ["lib"], "name": "dep"}],
                "dependencies": [],
            },
        ],
        "resolve": {
            "nodes": [
                {
                    "id": "path+root#root@0.3.0",
                    "dependencies": ["path+common#common@0.3.0"],
                },
                {
                    "id": "path+common#common@0.3.0",
                    "dependencies": ["registry+dep#dep@1.2.3"],
                },
                {"id": "registry+dep#dep@1.2.3", "dependencies": []},
            ]
        },
    }
    lock = {
        ("dep", "1.2.3"): {
            "name": "dep",
            "version": "1.2.3",
            "checksum": "a" * 64,
        }
    }
    return metadata, lock


class ReleaseMaterialTests(unittest.TestCase):
    def test_cyclonedx_is_deterministic_and_has_dependency_graph(self) -> None:
        metadata, lock = fixture()
        arguments = dict(
            target="x86_64-unknown-linux-gnu",
            artifact_name="fuckipv6-linux.tar.gz",
            release_version="v0.3.0",
            commit="b" * 40,
            created="2026-10-30T00:00:00Z",
            source_repo="https://example.invalid/fuckipv6",
        )
        first = MATERIAL.build_cyclonedx(metadata, lock, **arguments)
        second = MATERIAL.build_cyclonedx(metadata, lock, **arguments)

        self.assertEqual(first, second)
        self.assertEqual(first["bomFormat"], "CycloneDX")
        self.assertEqual(first["specVersion"], "1.5")
        self.assertEqual(len(first["components"]), 3)
        self.assertEqual(len(first["dependencies"]), 4)
        dep = next(component for component in first["components"] if component["name"] == "dep")
        self.assertEqual(dep["hashes"], [{"alg": "SHA-256", "content": "a" * 64}])

    def test_spdx_has_root_contains_and_dependency_edges(self) -> None:
        metadata, lock = fixture()
        document = MATERIAL.build_spdx(
            metadata,
            lock,
            target="x86_64-unknown-linux-gnu",
            artifact_name="fuckipv6-linux.tar.gz",
            release_version="v0.3.0",
            commit="b" * 40,
            created="2026-10-30T00:00:00Z",
            source_repo=None,
        )

        self.assertEqual(document["spdxVersion"], "SPDX-2.3")
        relationships = {
            (item["spdxElementId"], item["relationshipType"], item["relatedSpdxElement"])
            for item in document["relationships"]
        }
        self.assertIn(
            ("SPDXRef-DOCUMENT", "DESCRIBES", "SPDXRef-Package-fuckipv6-release"),
            relationships,
        )
        self.assertTrue(any(item[1] == "DEPENDS_ON" for item in relationships))

    def test_checksum_manifest_rejects_tampering(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "artifact.bin"
            path.write_bytes(b"release")
            digest = MATERIAL.sha256_file(path)
            manifest = Path(directory) / "artifact.SHA256SUMS"
            MATERIAL.write_manifest(manifest, [("artifact.bin", digest)])

            path.write_bytes(b"tampered")
            listed, _, name = manifest.read_text(encoding="utf-8").strip().partition("  ")
            self.assertEqual(name, "artifact.bin")
            self.assertNotEqual(MATERIAL.sha256_file(path), listed)

    def test_provenance_is_explicitly_unsigned_and_records_lock_hash(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            artifact = Path(directory) / "artifact.tar.gz"
            artifact.write_bytes(b"release")
            document = MATERIAL.build_provenance(
                artifact=artifact,
                artifact_sha256="c" * 64,
                target="x86_64-unknown-linux-gnu",
                release_version="v0.3.0",
                source={"commit": "b" * 40},
                lock_sha256="d" * 64,
                created="2026-10-30T00:00:00Z",
            )

        self.assertFalse(document["verification"]["signed"])
        self.assertFalse(document["verification"]["slsa_attestation"])
        self.assertEqual(document["materials"]["cargo_lock_sha256"], "d" * 64)


if __name__ == "__main__":
    unittest.main()
