# SPDX-License-Identifier: GPL-3.0-or-later
"""Offline verification of reviewed third-party source trees and their provenance."""
import hashlib
import json
from pathlib import Path, PurePosixPath
import re
import tomllib


def verify(root):
    root = Path(root).resolve()
    base = root / "vendor"
    manifest = base / "sources.json"
    if not base.exists():
        return {}
    if base.is_symlink() or not manifest.is_file() or manifest.is_symlink() or manifest.stat().st_size > 1024 * 1024:
        raise ValueError("Missing or invalid reviewed vendor source manifest")
    data = json.loads(manifest.read_text())
    if data.get("schema_version") != 1 or not isinstance(data.get("packages"), list):
        raise ValueError("Unsupported vendor source manifest")
    result = {}
    for package in data["packages"]:
        name, version, folder_name = (package.get(k, "") for k in ("name", "version", "path"))
        if not re.fullmatch(r"[a-z0-9][a-z0-9_-]*", name) or folder_name != name:
            raise ValueError("Invalid vendor package location")
        key = name + "@" + version
        if key in result or not re.fullmatch(r"\d+\.\d+\.\d+", version):
            raise ValueError("Duplicate or invalid vendor version")
        folder = base / folder_name
        if folder.is_symlink() or not folder.is_dir():
            raise ValueError("Invalid vendor package directory: " + key)
        expected = package.get("files", {})
        if not isinstance(expected, dict) or not 1 <= len(expected) <= 10_000:
            raise ValueError("Unbounded or missing vendor file inventory: " + key)
        actual, total = set(), 0
        for path in folder.rglob("*"):
            if path.is_symlink() or not path.resolve().is_relative_to(folder):
                raise ValueError("Vendor source cannot contain links: " + key)
            if path.is_dir():
                continue
            if not path.is_file():
                raise ValueError("Invalid vendor source entry: " + key)
            relative = path.relative_to(folder).as_posix()
            actual.add(relative)
            size = path.stat().st_size
            total += size
            if size > 16 * 1024 * 1024 or total > 64 * 1024 * 1024:
                raise ValueError("Vendor source exceeds its byte bound: " + key)
            digest = hashlib.sha256(path.read_bytes()).hexdigest()
            if expected.get(relative) != digest:
                raise ValueError("Vendor source changed or was added: " + key + "/" + relative)
        if actual != set(expected):
            raise ValueError("Vendor source inventory is incomplete: " + key)
        for relative in expected:
            path = PurePosixPath(relative)
            if path.is_absolute() or any(part in (".", "..") for part in path.parts) or "\\" in relative:
                raise ValueError("Invalid vendor source path: " + key)
        cargo = tomllib.loads((folder / "Cargo.toml").read_text())["package"]
        if (cargo["name"], cargo["version"]) != (name, version) or not cargo.get("license"):
            raise ValueError("Vendor Cargo identity differs from its source manifest: " + key)
        native = package.get("native")
        wrapper = package.get("wrapper", {})
        if (native is not None and not re.fullmatch(r"[0-9a-f]{40}", native.get("revision", ""))) or not re.fullmatch(r"[0-9a-f]{64}", wrapper.get("registry_sha256", "")):
            raise ValueError("Vendor source needs exact upstream provenance: " + key)
        patch_names = set()
        for patch in (native or {}).get("patches", []):
            name = patch.get("name", "")
            if not re.fullmatch(r"[a-z0-9][a-z0-9_-]*", name) or name in patch_names:
                raise ValueError("Invalid or duplicate native patch record: " + key)
            patch_names.add(name)
            if expected.get("patches/" + name + ".patch") != patch.get("sha256"):
                raise ValueError("Native patch record differs from the reviewed inventory: " + key)
        result[key] = {"folder": folder, "native": native, "wrapper": wrapper, "inventory_sha256": hashlib.sha256(
            json.dumps(expected, sort_keys=True, separators=(",", ":")).encode()).hexdigest()}
    return result


def receipt(root):
    return {key: {"native": value["native"], "wrapper": value["wrapper"], "inventory_sha256": value["inventory_sha256"]}
            for key, value in verify(root).items()}


if __name__ == "__main__":
    packages = verify(Path(__file__).resolve().parents[2])
    print(f"Verified {len(packages)} reviewed vendor source package(s)")
