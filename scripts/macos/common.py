# SPDX-License-Identifier: GPL-3.0-or-later
"""Local release helpers. No app data, credentials, network or shell evaluation."""
import hashlib
import json
import os
import re
from pathlib import Path
import stat
import subprocess
import tempfile
import time
import zipfile

ROOT = Path(__file__).resolve().parents[2]
DIST = ROOT / "dist"


def run(arguments, *, capture=False, env=None):
    return subprocess.run(arguments, cwd=ROOT, check=True, text=True,
                          stdout=subprocess.PIPE if capture else None,
                          env=env).stdout


def sha256(path):
    with path.open("rb") as source:
        return hashlib.file_digest(source, "sha256").hexdigest()


def write_json(path, value):
    path.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n")


def inventory(app):
    result = {}
    for path in sorted(app.rglob("*")):
        if path.is_symlink():
            raise ValueError("App bundles must contain regular files, not symbolic links")
        if path.is_file():
            result[path.relative_to(app).as_posix()] = sha256(path)
    return result


def deterministic_zip(app, destination, epoch):
    """Stable path order, timestamps and modes; never include filesystem extras."""
    stamp = time.gmtime(max(315532800, min(epoch, 4354819198)))[:6]
    temporary = destination.with_suffix(".zip.tmp")
    try:
        with zipfile.ZipFile(temporary, "w", zipfile.ZIP_DEFLATED, compresslevel=9) as archive:
            for path in sorted(app.rglob("*")):
                if path.is_symlink():
                    raise ValueError("Update archives must not contain symbolic links")
                if not path.is_file():
                    continue
                info = zipfile.ZipInfo(path.relative_to(app.parent).as_posix(), stamp)
                info.create_system = 3
                mode = 0o755 if os.access(path, os.X_OK) else 0o644
                info.external_attr = (stat.S_IFREG | mode) << 16
                info.compress_type = zipfile.ZIP_DEFLATED
                archive.writestr(info, path.read_bytes(), compress_type=zipfile.ZIP_DEFLATED,
                                 compresslevel=9)
        temporary.replace(destination)
    finally:
        temporary.unlink(missing_ok=True)


def export_release(app, build, *, notarized=False):
    architecture = run(["lipo", "-archs", str(app / "Contents/MacOS/OpenRadView")], capture=True).strip()
    variant = build.get("edition", "mcp" if build["features"] else "offline")
    stem = f"OpenRadView-{build['version']}-macos-{architecture}-{variant}"
    if build["profile"] == "debug":
        stem += "-debug"
    archive = DIST / (stem + ".zip")
    if build.get("updater", {}).get("protocol") != "ed25519-manifest-v1":
        raise ValueError("Release archives require the shared signed-manifest updater")
    deterministic_zip(app, archive, build["source_epoch"])
    # Prove the distributed container retained the sealed files, executable
    # modes and (when present) Apple's ticket after a fresh extraction.
    from verify import verify
    with tempfile.TemporaryDirectory(prefix=".orv-archive-check-", dir=DIST) as temporary:
        run(["ditto", "-x", "-k", str(archive), temporary])
        extracted = Path(temporary) / app.name
        verify(extracted, distribution=build["signing"] == "Developer ID", notarized=notarized)
        if inventory(extracted) != inventory(app):
            raise ValueError("Distributed ZIP differs from the validated sealed app")
    record = {"schema_version": 1, "build": build, "notarized": notarized,
              "archive_verified": True,
              "archive": archive.name, "archive_sha256": sha256(archive),
              "archive_format": "deterministic ZIP",
              "bundle_files": inventory(app)}
    if not build["source_dirty"]:
        source = DIST / f"OpenRadView-{build['version']}-source-{build['source_revision'][:12]}.tar.gz"
        run(["git", "archive", "--format=tar.gz", "--prefix=OpenRadView/",
             "--output=" + str(source), build["source_revision"]])
        record["source_archive"] = source.name
        record["source_archive_sha256"] = sha256(source)
    write_json(DIST / (stem + ".json"), record)
    (DIST / (stem + ".sha256")).write_text(record["archive_sha256"] + "  " + archive.name + "\n")
    return archive


def release_notes(version):
    """Use one reviewed changelog section for both source and release notes."""
    if not re.fullmatch(r"\d+\.\d+\.\d+", version):
        raise ValueError("Expected a stable release version")
    text = (ROOT / "CHANGELOG.md").read_text()
    headings = list(re.finditer(r"^## (.+)$", text, re.MULTILINE))
    selected = [i for i, heading in enumerate(headings) if re.match(re.escape(version) + r"(?:\s|$)", heading[1])]
    if len(selected) != 1:
        raise ValueError("Changelog must contain exactly one section for this version")
    index = selected[0]
    end = headings[index + 1].start() if index + 1 < len(headings) else len(text)
    body = text[headings[index].end():end].strip()
    if not body:
        raise ValueError("Release changelog section is empty")
    base = f"https://github.com/gyorgysh/openradview/blob/v{version}/"
    body = re.sub(r"\]\(([^)]+)\)", lambda m: m[0] if ":" in m[1] or m[1].startswith("#") else "](" + base + m[1] + ")", body)
    notice = "For personal education; not clinically validated. Do not use for diagnosis, treatment or decisions about care. Ask a qualified clinician to interpret your scans."
    return f"# OpenRadView {version}\n\n{notice}\n\n{body}\n"
