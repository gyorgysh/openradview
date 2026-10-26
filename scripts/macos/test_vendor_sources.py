# SPDX-License-Identifier: GPL-3.0-or-later
import hashlib
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
import notices
import vendor_sources


class VendorIntegrity(unittest.TestCase):
    def fixture(self, root):
        folder = root / "vendor/fixture"
        folder.mkdir(parents=True)
        (folder / "Cargo.toml").write_text('[package]\nname="fixture"\nversion="1.0.0"\nlicense="MIT"\n')
        (folder / "LICENSE").write_text("Original synthetic license fixture")
        package = {"name": "fixture", "version": "1.0.0", "path": "fixture",
                   "native": {"name": "Fixture", "version": "2.0.0", "repository": "https://example.invalid/fixture", "revision": "a" * 40},
                   "wrapper": {"registry_sha256": "b" * 64},
                   "files": {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in folder.iterdir()}}
        (root / "vendor/sources.json").write_text(json.dumps({"schema_version": 1, "packages": [package]}))
        return folder

    def test_changed_missing_added_and_linked_sources_are_rejected(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            folder = self.fixture(root)
            self.assertEqual(len(vendor_sources.verify(root)), 1)
            license = folder / "LICENSE"
            original = license.read_bytes()
            license.write_text("changed")
            with self.assertRaises(ValueError): vendor_sources.verify(root)
            license.unlink()
            with self.assertRaises(ValueError): vendor_sources.verify(root)
            license.write_bytes(original)
            (folder / "unreviewed.h").write_text("extra native code")
            with self.assertRaises(ValueError): vendor_sources.verify(root)
            (folder / "unreviewed.h").unlink()
            license.unlink()
            license.symlink_to(folder / "Cargo.toml")
            with self.assertRaises(ValueError): vendor_sources.verify(root)

    def test_local_dependency_notices_are_included_but_unknown_paths_fail(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            folder = self.fixture(root)
            (root / "assets/licenses").mkdir(parents=True)
            (root / "assets/licenses/index.json").write_text('{"packages":{}}')
            demos = root / "assets/demos"
            demos.mkdir()
            for name in ("CREDITS.txt", "CC-BY-4.0.txt", "CC-BY-3.0.txt"): (demos / name).write_text("Synthetic notice")
            metadata = {"workspace_members": ["own-code"], "packages": [
                {"id": "own-code", "source": None, "name": "orv-core", "version": "1.0.0", "license": "GPL-3.0-or-later", "manifest_path": str(root / "own/Cargo.toml")},
                {"id": "fixture", "source": None, "name": "fixture", "version": "1.0.0", "license": "MIT", "manifest_path": str(folder / "Cargo.toml")}]}
            tree = "orv-core v1.0.0\nfixture v1.0.0"
            with patch.object(notices, "ROOT", root):
                with patch.object(notices, "run", side_effect=[json.dumps(metadata), tree]):
                    text, count, _ = notices.collect_notices("fixture-target", False)
                    self.assertEqual(count, 1)
                    self.assertIn("Original synthetic license fixture", text)
                    self.assertIn("Bundled native code: Fixture 2.0.0", text)
                metadata["packages"][1]["manifest_path"] = str(root / "unreviewed/Cargo.toml")
                with patch.object(notices, "run", side_effect=[json.dumps(metadata), tree]):
                    with self.assertRaises(ValueError): notices.collect_notices("fixture-target", False)


if __name__ == "__main__": unittest.main()
