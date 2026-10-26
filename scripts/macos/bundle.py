#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Build and seal the native bundle; signing and export are explicit options."""
import argparse
import os
from pathlib import Path
import platform
import plistlib
import re
import shutil
import subprocess
import sys
import tempfile

from common import DIST, ROOT, export_release, run, write_json
from notices import collect_notices
import vendor_sources
from icon import compile_icon
sys.path.insert(0, str(ROOT / "scripts"))
import update_protocol


def sign(path, identity):
    options = ["--timestamp=none"] if identity == "-" else ["--timestamp"]
    runtime = [] if identity == "-" else ["--options", "runtime"]
    run(["codesign", "--force", "--sign", identity, *runtime, *options, str(path)])


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--debug", action="store_true", help="development build (release is the default)")
    parser.add_argument("--mcp", action="store_true", help="compatibility flag: MCP is included in the normal app")
    parser.add_argument("--qa", action="store_true", help="separate developer timing build; never a distribution artifact")
    parser.add_argument("--sign-identity", default="-", metavar="DEVELOPER_ID", help="explicit Developer ID identity/hash; default is ad-hoc")
    parser.add_argument("--archive", action="store_true", help="export deterministic ZIP, checksums and source/release manifest")
    args = parser.parse_args()
    if platform.system() != "Darwin":
        parser.error("macOS is required")
    if sys.version_info < (3, 11):
        parser.error("Python 3.11 or newer is required")
    if args.qa and (args.mcp or args.archive or args.sign_identity != "-"):
        parser.error("QA is a separate ad-hoc timing build; it cannot enable MCP, archive or Developer ID signing")
    dirty = bool(run(["git", "status", "--porcelain", "--untracked-files=normal"], capture=True).strip())
    if args.sign_identity != "-" and (args.debug or dirty):
        parser.error("Developer ID artifacts require a clean committed source tree and an optimized build")
    if args.archive and not args.debug and dirty:
        parser.error("Release archives require a clean committed source tree; use --debug for development exports")
    vendor_record = vendor_sources.receipt(ROOT)
    toolchain = run(["rustc", "-vV"], capture=True)
    host = next(line.removeprefix("host: ") for line in toolchain.splitlines() if line.startswith("host: "))
    environment = os.environ.copy()
    environment["MACOSX_DEPLOYMENT_TARGET"] = "13.0"
    features = ["qa"] if args.qa else ["mcp", "updates"]
    command = ["cargo", "build", "--locked", "-p", "orv-app", "--bin", "openradview"]
    if not args.debug:
        command.append("--release")
    if not args.qa:
        command.extend(["-p", "orv-mcp", "--bin", "openradview-mcp"])
    command.extend(["--features", ",".join("orv-app/" + f for f in features)])
    run(command, env=environment)
    notices, count, metadata = collect_notices(host, not args.qa, args.qa, updates=not args.qa)
    icon_folder, icon_info, icon_record = compile_icon()
    version = next(p["version"] for p in metadata["packages"] if p["name"] == "orv-app")
    if not re.fullmatch(r"\d+\.\d+\.\d+", version):
        raise ValueError("macOS bundle versions must use numeric x.y.z Cargo versions")
    profile = "debug" if args.debug else "release"
    binaries = Path(metadata["target_directory"]) / profile
    DIST.mkdir(exist_ok=True)
    target = DIST / ("OpenRadView QA.app" if args.qa else "OpenRadView.app")
    if target.is_symlink():
        raise ValueError("Refusing to replace a symlink at the bundle destination")
    with tempfile.TemporaryDirectory(prefix=".orv-bundle-", dir=DIST) as temporary:
        stage = Path(temporary) / "OpenRadView.app"
        executables = stage / "Contents/MacOS"
        resources = stage / "Contents/Resources"
        executables.mkdir(parents=True)
        resources.mkdir()
        shutil.copyfile(binaries / "openradview", executables / "OpenRadView")
        (executables / "OpenRadView").chmod(0o755)
        if not args.qa:
            shutil.copyfile(binaries / "openradview-mcp", executables / "openradview-mcp")
            (executables / "openradview-mcp").chmod(0o755)
        with (ROOT / "packaging/macos/Info.plist").open("rb") as source:
            info = plistlib.load(source)
        info["CFBundleShortVersionString"] = info["CFBundleVersion"] = version
        info.update(icon_info)
        if args.qa:
            info["CFBundleIdentifier"] = "com.openradview.qa"
            info["CFBundleName"] = info["CFBundleDisplayName"] = "OpenRadView QA"
        with (stage / "Contents/Info.plist").open("wb") as output:
            plistlib.dump(info, output, sort_keys=True)
        for name in ["LICENSE", "DISCLAIMER.md", "PRIVACY.md"]:
            shutil.copyfile(ROOT / name, resources / name)
        for path in sorted((ROOT / "assets/fonts").glob("*-OFL.txt")):
            shutil.copyfile(path, resources / path.name)
        for name in ["OpenRadView.icns", "Assets.car"]:
            shutil.copyfile(icon_folder / name, resources / name)
        (resources / "THIRD_PARTY_NOTICES.txt").write_text(notices)
        build = {"schema_version": 1, "version": version, "profile": profile,
                 "features": features, "target": host,
                 "edition": "qa" if args.qa else "standard",
                 "minimum_macos": "13.0", "source_revision": run(["git", "rev-parse", "HEAD"], capture=True).strip(),
                 "source_dirty": dirty, "source_epoch": int(run(["git", "show", "-s", "--format=%ct", "HEAD"], capture=True)),
                 "rustc": toolchain.splitlines()[0], "dependency_notice_count": count,
                 "vendored_sources": vendor_record,
                 "signing": "ad-hoc" if args.sign_identity == "-" else "Developer ID",
                 "hardened_runtime": args.sign_identity != "-", "sharing_on_startup": False, "icon": icon_record}
        if not args.qa:
            build["updater"] = update_protocol.configuration()
        write_json(resources / "build.json", build)
        # Sign nested code first. No --deep signing and no runtime exceptions.
        if not args.qa:
            sign(executables / "openradview-mcp", args.sign_identity)
        if not args.qa:
            update_protocol.write_layout(stage, host, macos=True)
        sign(stage, args.sign_identity)
        run([sys.executable, str(ROOT / "scripts/macos/verify.py"), str(stage)])
        backup = Path(temporary) / "previous.app"
        if target.exists():
            target.rename(backup)
        try:
            stage.rename(target)
        except OSError:
            if backup.exists():
                backup.rename(target)
            raise
    print(f"Built {target.relative_to(ROOT)} ({build['signing']}, not notarized)")
    if args.archive:
        archive = export_release(target, build)
        print("Exported " + str(archive.relative_to(ROOT)))


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, KeyError, subprocess.CalledProcessError, StopIteration) as error:
        print("Bundle failed: " + str(error), file=sys.stderr)
        sys.exit(1)
