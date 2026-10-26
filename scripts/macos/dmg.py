#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Build, sign and mount-audit a branded drag-to-Applications disk image locally."""
import argparse
import importlib.metadata
import json
import os
from pathlib import Path
import plistlib
import re
import subprocess
import sys
import tempfile

from common import DIST, ROOT, inventory, run, sha256, write_json
from verify import verify


def tooling():
    for package, version in [("dmgbuild", "1.6.7"), ("ds_store", "1.3.3"), ("mac_alias", "2.2.3")]:
        if importlib.metadata.version(package) != version:
            raise ValueError("Use the exact hash-pinned installer tooling in target/dmgbuild-venv")


def layout(app, background):
    return {
        "files": [str(app)], "symlinks": {"Applications": "/Applications"},
        "format": "UDZO", "filesystem": "HFS+", "background": str(background),
        "icon": str(app / "Contents/Resources/OpenRadView.icns"),
        "window_rect": ((180, 120), (660, 452)), "default_view": "icon-view",
        "show_toolbar": False, "show_status_bar": False, "show_sidebar": False,
        "show_pathbar": False, "show_tab_view": False, "show_icon_preview": False,
        "icon_size": 110, "text_size": 13, "arrange_by": None,
        "icon_locations": {app.name: (180, 222), "Applications": (480, 222),
                           ".background.tiff": (180, 1100), ".VolumeIcon.icns": (330, 1100), ".DS_Store": (480, 1100), ".fseventsd": (630, 1100)},
    }


def signing_team(path, expected):
    signature = subprocess.run(["codesign", "-d", "--verbose=4", str(path)], check=True, capture_output=True, text=True).stderr
    if expected == "Developer ID":
        if "Authority=Developer ID Application:" not in signature or "Timestamp=" not in signature:
            raise ValueError("Developer ID signature and secure timestamp are required")
    elif expected != "ad-hoc" or "Signature=adhoc" not in signature:
        raise ValueError("Installer signing provenance does not match its signature")
    match = re.search(r"^TeamIdentifier=(.+)$", signature, re.MULTILINE)
    return match[1] if match else None


def audit(image, build, expected_files, *, notarized=False, app_notarized=False, signing="ad-hoc", development=True):
    """Never run bundled code. Mount this image read-only, inspect, then detach."""
    from ds_store import DSStore
    run(["hdiutil", "verify", str(image)])
    run(["codesign", "--verify", "--strict", str(image)])
    team = signing_team(image, signing)
    if notarized:
        if development or signing != "Developer ID" or not app_notarized:
            raise ValueError("Notarized distribution requires a clean signed image and stapled app")
        run(["xcrun", "stapler", "validate", str(image)])
        run(["spctl", "--assess", "--type", "open", "--context", "context:primary-signature", str(image)])
    with tempfile.TemporaryDirectory(prefix="orv-dmg-audit-", dir=DIST) as temporary:
        mount = Path(temporary) / "mounted"
        mount.mkdir()
        subprocess.run(["hdiutil", "attach", "-readonly", "-nobrowse", "-noautoopen", "-mountpoint", str(mount), str(image)],
                       check=True, stdout=subprocess.DEVNULL)
        try:
            allowed = {"OpenRadView.app", "Applications", ".background.tiff", ".VolumeIcon.icns", ".DS_Store", ".fseventsd"}
            root_files = {p.name for p in mount.iterdir()}
            if root_files - allowed or not (allowed - {".fseventsd"}).issubset(root_files):
                raise ValueError("Unexpected or missing installer contents: " + repr(sorted(root_files)))
            shortcut = mount / "Applications"
            if not shortcut.is_symlink() or os.readlink(shortcut) != "/Applications":
                raise ValueError("Installer Applications shortcut is invalid")
            app = mount / "OpenRadView.app"
            if verify(app, distribution=not development, notarized=app_notarized) != build or inventory(app) != expected_files:
                raise ValueError("Installer changed the sealed application")
            if not development and signing_team(app, "Developer ID") != team:
                raise ValueError("Installer and application must use the same Developer ID team")
            with DSStore.open(str(mount / ".DS_Store"), "r") as store:
                if store["OpenRadView.app"]["Iloc"] != (180, 222) or store["Applications"]["Iloc"] != (480, 222):
                    raise ValueError("Installer icon positions do not match the artwork")
                view = store["."]["icvp"]
                if view["iconSize"] != 110 or view["backgroundType"] != 2:
                    raise ValueError("Installer background/icon view is missing")
            return {"root_entries": sorted(root_files), "app_files": expected_files,
                    "background_sha256": sha256(mount / ".background.tiff"), "mount_verified": True}
        finally:
            run(["hdiutil", "detach", str(mount)])


def record_stapled(image, record, verified, log_path):
    if record.get("notarized"):
        raise ValueError("A finalized installer record cannot be rewritten")
    log = json.loads(log_path.read_text())
    if log.get("status") != "Accepted" or log.get("sha256") != record["dmg_sha256"] or log.get("issues"):
        raise ValueError("Notary log must accept the exact pre-staple disk image without issues")
    updated = {**record, "submitted_dmg_sha256": record["dmg_sha256"], "dmg_sha256": sha256(image),
               "notarized": True, "notarization_log_sha256": sha256(log_path), "verified": verified}
    write_json(image.with_suffix(".dmg.json"), updated)
    image.with_suffix(".dmg.sha256").write_text(updated["dmg_sha256"] + "  " + image.name + "\n")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--app", type=Path, default=DIST / "OpenRadView.app")
    parser.add_argument("--output", type=Path)
    parser.add_argument("--sign-identity", default="-")
    parser.add_argument("--development", action="store_true", help="explicitly allow a local ad-hoc/dirty app; never publish it")
    parser.add_argument("--audit", type=Path, help="audit an existing image against its adjacent record")
    parser.add_argument("--notarized", action="store_true", help="require actual stapled image and Gatekeeper checks")
    parser.add_argument("--app-notarized", action="store_true", help="require a previously stapled/Gatekeeper-verified app when building")
    parser.add_argument("--record-stapled", action="store_true", help="finalize hashes only after actual notarized audit and matching accepted log")
    parser.add_argument("--notary-log", type=Path)
    args = parser.parse_args()
    if args.record_stapled and (not args.audit or not args.notarized or not args.notary_log):
        parser.error("--record-stapled requires --audit, --notarized and --notary-log")
    tooling()
    DIST.mkdir(exist_ok=True)
    if args.audit:
        image = args.audit.resolve(strict=True)
        record = json.loads(image.with_suffix(".dmg.json").read_text())
        if not args.record_stapled and sha256(image) != record["dmg_sha256"]:
            raise ValueError("Installer hash differs from its record")
        verified = audit(image, record["build"], record["verified"]["app_files"], notarized=args.notarized,
                         app_notarized=record.get("app_notarized", False), signing=record["signing"], development=record["development"])
        if args.record_stapled:
            record_stapled(image, record, verified, args.notary_log)
        print("Verified sealed installer, native signatures, application and Finder layout")
        return
    app = args.app.resolve(strict=True)
    if app.name != "OpenRadView.app":
        raise ValueError("Installer expects OpenRadView.app")
    build = verify(app, distribution=not args.development, notarized=args.app_notarized)
    if build["features"] not in ([], ["mcp"], ["mcp", "updates"]):
        raise ValueError("Unsupported installer features")
    if not args.development and args.sign_identity == "-":
        raise ValueError("Distribution disk images require an explicit Developer ID identity")
    if args.notarized:
        parser.error("Build the image first, then notarize/staple it separately and use --audit --notarized")
    variant = build.get("edition", "mcp" if build["features"] else "offline")
    architecture = "arm64" if build["target"].startswith("aarch64-") else "x86_64"
    stem = f"OpenRadView-{build['version']}-macos-{architecture}-{variant}" + ("-development" if args.development else "")
    destination = (args.output or DIST / (stem + ".dmg")).resolve()
    if destination.exists() or destination.is_symlink() or destination.with_suffix(".dmg.json").exists():
        raise ValueError("Refusing to overwrite a versioned installer or record; choose a new output/version")
    if destination.suffix != ".dmg":
        raise ValueError("Installer output must end in .dmg")
    destination.parent.mkdir(parents=True, exist_ok=True)
    expected_files = inventory(app)
    from dmgbuild import build_dmg
    with tempfile.TemporaryDirectory(prefix=".orv-installer-", dir=destination.parent) as temporary:
        stage = Path(temporary)
        copied = stage / "OpenRadView.app"
        run(["ditto", str(app), str(copied)])
        background = stage / "background.tiff"
        art = ROOT / "packaging/macos/installer"
        run(["tiffutil", "-cathidpicheck", str(art / "background.png"), str(art / "background@2x.png"), "-out", str(background)])
        image = stage / "OpenRadView.dmg"
        build_dmg(str(image), f"OpenRadView {build['version']}", settings=layout(copied, background), lookForHiDPI=False)
        timestamp = ["--timestamp=none"] if args.sign_identity == "-" else ["--timestamp"]
        run(["codesign", "--force", "--sign", args.sign_identity, *timestamp, str(image)])
        signature = "ad-hoc" if args.sign_identity == "-" else "Developer ID"
        verified = audit(image, build, expected_files, app_notarized=args.app_notarized, signing=signature, development=args.development)
        digest = sha256(image)
        record = {"schema_version": 1, "dmg": destination.name, "dmg_sha256": digest, "build": build,
                  "development": args.development, "notarized": False, "app_notarized": args.app_notarized,
                  "signing": signature, "verified": verified}
        image.rename(destination)
        write_json(destination.with_suffix(".dmg.json"), record)
        destination.with_suffix(".dmg.sha256").write_text(digest + "  " + destination.name + "\n")
    print(f"Built and mount-verified {destination.name}; NOT notarized")


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, KeyError, importlib.metadata.PackageNotFoundError, subprocess.CalledProcessError) as error:
        print("Installer failed: " + str(error), file=sys.stderr)
        sys.exit(1)
