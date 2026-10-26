#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Audit bundle contents, provenance, native deployment, signatures and permissions."""
import argparse
import json
from pathlib import Path
import plistlib
import re
import subprocess
import sys

from common import ROOT, inventory, run, sha256


def version_tuple(value):
    parts = tuple(int(part) for part in value.split("."))
    return parts + (0,) * max(0, 3 - len(parts))


def verify(app, release_record=None, distribution=False, notarized=False):
    app = app.resolve(strict=True)
    contents = app / "Contents"
    resources = contents / "Resources"
    build = json.loads((resources / "build.json").read_text())
    updates = "updates" in build["features"]
    shared_updates = updates and build.get("updater",{}).get("protocol") == "ed25519-manifest-v1"
    if updates and not shared_updates:
        raise ValueError("Unsupported updater protocol")
    required_components = {"MacOS", "Resources", "Info.plist", "_CodeSignature"}
    present = {p.name for p in contents.iterdir()}
    allowed = required_components | ({"CodeResources"} if notarized else set())
    if not required_components.issubset(present) or present - allowed:
        raise ValueError("Unexpected or missing bundle component")
    if {p.name for p in (contents / "_CodeSignature").iterdir()} != {"CodeResources"}:
        raise ValueError("Unexpected code-signature resource")
    info = plistlib.loads((contents / "Info.plist").read_bytes())
    build = json.loads((resources / "build.json").read_text())
    if build["schema_version"] != 1 or build["sharing_on_startup"] is not False:
        raise ValueError("Invalid provenance or privacy default")
    if info["CFBundleShortVersionString"] != build["version"] or info["CFBundleVersion"] != build["version"]:
        raise ValueError("Bundle/provenance version mismatch")
    expected_identifier = "com.openradview.qa" if build["features"] == ["qa"] else "com.openradview.app"
    if info["LSMinimumSystemVersion"] != build["minimum_macos"] or info["CFBundleIdentifier"] != expected_identifier:
        raise ValueError("Unexpected bundle identity/deployment target")
    expected = {"OpenRadView"}
    if build["features"] in (["mcp"], ["mcp", "updates"]):
        expected.add("openradview-mcp")
    elif build["features"] not in ([], ["qa"]):
        raise ValueError("Unsupported release feature set")
    if {p.name for p in (contents / "MacOS").iterdir()} != expected:
        raise ValueError("Unexpected or missing bundled executable (including stale MCP companion)")
    required = {"LICENSE", "DISCLAIMER.md", "PRIVACY.md", "OpenRadView.icns", "THIRD_PARTY_NOTICES.txt",
                "SpaceGrotesk-OFL.txt", "IBMPlexMono-OFL.txt", "build.json"}
    if "icon" in build:
        if build["icon"].get("kind") != "apple-icon-composer" or info.get("CFBundleIconName") != "OpenRadView":
            raise ValueError("Invalid layered macOS icon provenance")
        required.add("Assets.car")
    if shared_updates:
        required.add("update-layout.json")
    if {p.name for p in resources.iterdir()} != required:
        raise ValueError("Unexpected or missing bundled resource")
    if shared_updates:
        sys.path.insert(0,str(ROOT / "scripts"))
        import update_protocol
        if build.get("updater") != update_protocol.configuration() or any(key.startswith(("SU","SPU")) for key in info):
            raise ValueError("Shared updater identity or privacy settings differ from policy")
        expected_files = set(inventory(app)) - {"Contents/CodeResources"}
        if set(json.loads((resources / "update-layout.json").read_text())["files"]) != expected_files:
            raise ValueError("Managed installation layout differs from the sealed bundle")
    elif any(key.startswith(("SU", "SPU")) for key in info):
        raise ValueError("Unexpected updater settings")
    files = inventory(app)
    if any(Path(name).suffix.lower() in {".dcm", ".dicom", ".nii", ".log", ".p12", ".key"} for name in files):
        raise ValueError("Private data/credentials must not be bundled")
    binaries = [contents / "MacOS" / name for name in sorted(expected)]
    teams = set()
    for binary in binaries:
        if not binary.stat().st_mode & 0o111:
            raise ValueError("Bundled program is not executable")
        architecture = run(["lipo", "-archs", str(binary)], capture=True).strip()
        expected_arch = "arm64" if build["target"].startswith("aarch64-") else "x86_64"
        if architecture != expected_arch:
            raise ValueError("Binary architecture differs from recorded Cargo target")
        load_commands = run(["vtool", "-show-build", str(binary)], capture=True)
        minima = re.findall(r"minos\s+(\d+(?:\.\d+)+)", load_commands)
        if not minima or any(version_tuple(m) > version_tuple(build["minimum_macos"]) for m in minima):
            raise ValueError("Executable requires a newer macOS than the bundle declares")
        run(["codesign", "--verify", "--strict", str(binary)])
        signature = subprocess.run(["codesign", "-d", "--verbose=4", str(binary)], check=True,
                                   capture_output=True, text=True).stderr
        if build["signing"] == "Developer ID" and "runtime" not in signature:
            raise ValueError("Hardened runtime is missing")
        entitlements = subprocess.run(["codesign", "-d", "--entitlements", ":-", str(binary)],
                                      check=True, capture_output=True).stdout.strip()
        if entitlements and plistlib.loads(entitlements):
            raise ValueError("Unexpected hardened-runtime exception or entitlement")
        if build["signing"] == "Developer ID":
            if "Authority=Developer ID Application:" not in signature or "Timestamp=" not in signature:
                raise ValueError("Developer ID signature or secure timestamp is missing")
            match = re.search(r"^TeamIdentifier=(.+)$", signature, re.MULTILINE)
            teams.add(match[1] if match else None)
        elif build["signing"] != "ad-hoc" or "Signature=adhoc" not in signature:
            raise ValueError("Signing record does not match the actual signature")
    if build["signing"] == "Developer ID" and (len(teams) != 1 or None in teams):
        raise ValueError("All bundled code must have the same Developer ID team")
    run(["codesign", "--verify", "--deep", "--strict", str(app)])
    if distribution and (build["profile"] != "release" or build["source_dirty"] or build["signing"] != "Developer ID" or "qa" in build["features"]):
        raise ValueError("Distribution requires optimized, clean-source, Developer ID-signed artifacts without QA")
    if release_record:
        record = json.loads(release_record.read_text())
        if record.get("archive_verified") is not True or record["build"] != build or record["bundle_files"] != files:
            raise ValueError("Release manifest does not match this sealed bundle")
        for key in ["archive", *(["source_archive"] if not build["source_dirty"] else [])]:
            archive = release_record.parent / record[key]
            if archive.parent.resolve() != release_record.parent.resolve() or sha256(archive) != record[key + "_sha256"]:
                raise ValueError("Release archive hash/path mismatch")
    if notarized:
        run(["xcrun", "stapler", "validate", str(app)])
        run(["spctl", "--assess", "--type", "execute", "--verbose=2", str(app)])
    return build


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("app", type=Path)
    parser.add_argument("--release-record", type=Path)
    parser.add_argument("--distribution", action="store_true")
    parser.add_argument("--notarized", action="store_true")
    args = parser.parse_args()
    build = verify(args.app, args.release_record, args.distribution, args.notarized)
    print(f"Verified {build['version']} {build['profile']} ({build['signing']}); notarized check: {args.notarized}")


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, KeyError, subprocess.CalledProcessError) as error:
        print("Bundle verification failed: " + str(error), file=sys.stderr)
        sys.exit(1)
