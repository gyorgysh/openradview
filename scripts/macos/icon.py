# SPDX-License-Identifier: GPL-3.0-or-later
"""Compile Apple's layered macOS icon once per source/Xcode combination."""
import hashlib
import json
from pathlib import Path
import plistlib
import tempfile

from common import ROOT, run, sha256, write_json


def compile_icon():
    source = ROOT / "packaging/macos/OpenRadView.icon"
    files = {p.relative_to(source).as_posix(): sha256(p) for p in sorted(source.rglob("*")) if p.is_file()}
    xcode = run(["xcodebuild", "-version"], capture=True).strip()
    identity = {"source": files, "xcode": xcode, "minimum_macos": "13.0"}
    digest = hashlib.sha256(json.dumps(identity, sort_keys=True).encode()).hexdigest()
    parent = ROOT / "target/macos-icons"
    parent.mkdir(parents=True, exist_ok=True)
    cache = parent / digest
    expected = ["Assets.car", "OpenRadView.icns", "icon-info.plist"]
    valid = False
    if (cache / "cache.json").is_file():
        record = json.loads((cache / "cache.json").read_text())
        valid = record.get("identity") == identity and all((cache / n).is_file() and sha256(cache / n) == record.get("files", {}).get(n) for n in expected)
    if not valid:
        with tempfile.TemporaryDirectory(prefix="icon-", dir=parent) as temporary:
            output = Path(temporary)
            run(["xcrun", "actool", str(source), "--compile", str(output),
                 "--platform", "macosx", "--target-device", "mac", "--minimum-deployment-target", "13.0",
                 "--app-icon", "OpenRadView", "--output-partial-info-plist", str(output / "icon-info.plist"),
                 "--output-format", "human-readable-text"])
            if not all((output / n).is_file() for n in expected):
                raise ValueError("Apple's icon compiler did not produce the layered and fallback icon")
            cache.mkdir(exist_ok=True)
            for n in expected:
                (output / n).replace(cache / n)
            write_json(cache / "cache.json", {"identity": identity, "files": {n: sha256(cache / n) for n in expected}})
    info = plistlib.loads((cache / "icon-info.plist").read_bytes())
    if info.get("CFBundleIconName") != "OpenRadView" or info.get("CFBundleIconFile") not in ("OpenRadView", "OpenRadView.icns"):
        raise ValueError("Unexpected compiled macOS icon identity")
    source_digest = hashlib.sha256(json.dumps(files, sort_keys=True).encode()).hexdigest()
    return cache, info, {"kind": "apple-icon-composer", "source_sha256": source_digest, "xcode": xcode}
