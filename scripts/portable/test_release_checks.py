# SPDX-License-Identifier: GPL-3.0-or-later
import copy
import hashlib
import io
import json
from pathlib import Path
import tarfile
import tempfile
import unittest
from unittest.mock import patch
import zipfile

import release_checks as checks

VERSION, REVISION = "1.2.3", "a" * 40


def fixture(folder, target, *, changes=None, extra=None):
    name, stem = checks.update_protocol.names(VERSION, target)
    suffix = ".exe" if "windows" in target else ""
    files = {n: b"original synthetic fixture" for n in (
        "openradview" + suffix, "openradview-mcp" + suffix, "LICENSE", "PRIVACY.md",
        "DISCLAIMER.md", "THIRD-PARTY-NOTICES.txt", "source.tar.gz")}
    layout = {"schema_version": 1, "product": "OpenRadView", "target": target,
              "files": sorted([*files, "build.json", "update-layout.json"])}
    files["update-layout.json"] = json.dumps(layout).encode()
    build = {"version": VERSION, "source_commit": REVISION, "target": target,
             "build_host": target, "source_dirty": False, "profile": "release",
             "features": ["mcp", "updates"], "updater": checks.update_protocol.configuration(),
             "files": {n: hashlib.sha256(b).hexdigest() for n, b in files.items()}}
    build.update(changes or {})
    files["build.json"] = json.dumps(build).encode()
    files.update(extra or {})
    archive = folder / name
    if name.endswith(".zip"):
        with zipfile.ZipFile(archive, "w") as writer:
            for n, content in files.items():
                info = zipfile.ZipInfo(stem + "/" + n)
                info.create_system, info.external_attr = 3, 0o100755 << 16
                writer.writestr(info, content)
    else:
        with tarfile.open(archive, "w:gz", format=tarfile.USTAR_FORMAT) as writer:
            for n, content in files.items():
                info = tarfile.TarInfo(stem + "/" + n)
                info.size = len(content)
                writer.addfile(info, io.BytesIO(content))
    (folder / (name + ".sha256")).write_text(checks.digest(archive) + "  " + name + "\n")
    (folder / f"update-{target}.json").write_text("Synthetic envelope; cryptography tested separately")
    return archive, hashlib.sha256(files["source.tar.gz"]).hexdigest()


class PortableReleases(unittest.TestCase):
    def test_native_release_needs_exact_artifacts_and_completed_acceptance(self):
        for target in ["x86_64-unknown-linux-gnu", "x86_64-pc-windows-msvc"]:
            with self.subTest(target=target), tempfile.TemporaryDirectory() as temporary:
                folder = Path(temporary)
                archive, source_hash = fixture(folder, target)
                with patch.object(checks, "source_digest", return_value=source_hash), \
                        patch.object(checks.update_protocol, "verify") as signature:
                    names, accepted = checks.verify(folder, target, VERSION, REVISION, require_acceptance=False)
                    self.assertFalse(accepted)
                    self.assertIn(archive.name, names)
                    signature.assert_called_once()
                    with self.assertRaises(ValueError):
                        checks.verify(folder, target, VERSION, REVISION, require_acceptance=True)
                    report = {"schema_version": 1, "target": target, "version": VERSION,
                              "source_revision": REVISION, "archive_sha256": checks.digest(archive),
                              "checks": dict.fromkeys(checks.CHECKS, True),
                              "evidence": "Original test-only acceptance fixture, not a product validation record"}
                    path = folder / f"acceptance-{target}.json"
                    path.write_text(json.dumps(report))
                    self.assertTrue(checks.verify(folder, target, VERSION, REVISION, require_acceptance=True)[1])
                    for mutate in ("hash", "gpu", "missing", "evidence"):
                        bad = copy.deepcopy(report)
                        if mutate == "hash": bad["archive_sha256"] = "b" * 64
                        elif mutate == "gpu": bad["checks"]["native_gpu"] = False
                        elif mutate == "missing": del bad["checks"]["update_roundtrip"]
                        else: bad["evidence"] = ""
                        path.write_text(json.dumps(bad))
                        with self.assertRaises(ValueError):
                            checks.verify(folder, target, VERSION, REVISION, require_acceptance=True)

    def test_previews_cross_builds_modified_inventory_and_bad_signatures_refuse_freezing(self):
        target = "x86_64-pc-windows-gnu"
        for changes, extra in [({"source_dirty": True}, None), ({"profile": "debug"}, None),
                               ({"build_host": "aarch64-apple-darwin"}, None),
                               ({"source_commit": "b" * 40}, None), ({}, {"unexpected.txt": b"extra"})]:
            with tempfile.TemporaryDirectory() as temporary:
                folder = Path(temporary)
                _, source_hash = fixture(folder, target, changes=changes, extra=extra)
                with patch.object(checks, "source_digest", return_value=source_hash), \
                        patch.object(checks.update_protocol, "verify"):
                    with self.assertRaises(ValueError):
                        checks.verify(folder, target, VERSION, REVISION, require_acceptance=False)
        with tempfile.TemporaryDirectory() as temporary:
            folder = Path(temporary)
            _, source_hash = fixture(folder, target)
            with patch.object(checks, "source_digest", return_value=source_hash), \
                    patch.object(checks.update_protocol, "verify", side_effect=ValueError("invalid signature")):
                with self.assertRaises(ValueError):
                    checks.verify(folder, target, VERSION, REVISION, require_acceptance=False)

    def test_archive_reader_rejects_links_paths_aliases_and_pax_before_reading_payloads(self):
        with tempfile.TemporaryDirectory() as temporary:
            folder = Path(temporary)
            for name in ("../escape", "App/../escape", "App//file"):
                path = folder / "bad.zip"
                with zipfile.ZipFile(path, "w") as writer:
                    info = zipfile.ZipInfo(name)
                    info.external_attr = 0o100644 << 16
                    writer.writestr(info, b"fixture")
                with self.assertRaises(ValueError): checks.inventory(path, "App")
            path = folder / "bad.tar.gz"
            with tarfile.open(path, "w:gz", format=tarfile.PAX_FORMAT) as writer:
                info = tarfile.TarInfo("App/file")
                info.pax_headers = {"comment": "refuse extended headers"}
                writer.addfile(info, io.BytesIO())
            with self.assertRaises(ValueError): checks.inventory(path, "App")
            for symlink in [False, True]:
                path = folder / "bad.zip"
                with zipfile.ZipFile(path, "w") as writer:
                    for name in ("App/File", "App/file"):
                        info = zipfile.ZipInfo(name)
                        info.create_system = 3
                        info.external_attr = (0o120777 if symlink else 0o100644) << 16
                        writer.writestr(info, b"fixture")
                with self.assertRaises(ValueError): checks.inventory(path, "App")


if __name__ == "__main__":
    unittest.main()
