# SPDX-License-Identifier: GPL-3.0-or-later
"""Read-only verification of portable release inputs, including native QA records."""
import gzip
import hashlib
import json
from pathlib import Path, PurePosixPath
import re
import stat
import struct
import subprocess
import sys
import tarfile
import tempfile
import zipfile

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))
import update_protocol
import native_evidence

TARGETS = {"x86_64-unknown-linux-gnu", "aarch64-unknown-linux-gnu", "x86_64-pc-windows-gnu", "x86_64-pc-windows-msvc", "aarch64-pc-windows-msvc"}
CHECKS = {"format", "clippy", "default_tests", "full_tests", "native_gpu", "file_folder_zip", "large_study_cancel", "navigation_resize", "accessibility", "labels_export", "consent_revoke", "unsaved_close", "update_roundtrip"}


def local(folder, name):
    path = folder / name
    if path.is_symlink() or not path.is_file():
        raise ValueError("Missing or linked portable release input: " + name)
    return path


def digest(path):
    with path.open("rb") as source:
        return hashlib.file_digest(source, "sha256").hexdigest()


def inventory(archive, bundle):
    """Hash bounded regular entries without extracting or executing binaries."""
    hashes, captured, aliases, total = {}, {}, set(), 0

    def entry(name, size, source):
        nonlocal total
        path = PurePosixPath(name)
        if (len(hashes) >= 8192 or size < 0 or total + size > 2 * 1024**3
                or "\\" in name or path.is_absolute() or any(p in ("", ".", "..") for p in name.split("/"))
                or len(path.parts) < 2 or path.parts[0] != bundle):
            raise ValueError("Invalid or unbounded portable archive entry")
        relative = "/".join(path.parts[1:])
        if relative.casefold() in aliases:
            raise ValueError("Duplicate or case-aliased portable archive entry")
        aliases.add(relative.casefold())
        total += size
        capture = relative in ("build.json", "update-layout.json")
        if capture and size > 1024**2:
            raise ValueError("Unbounded portable build metadata")
        chunks, hashed, read = [], hashlib.sha256(), 0
        while read < size:
            chunk = source.read(min(512 * 1024, size - read))
            if not chunk:
                break
            read += len(chunk)
            if read > size:
                raise ValueError("Portable entry exceeded its declared length")
            hashed.update(chunk)
            if capture:
                chunks.append(chunk)
        if read != size:
            raise ValueError("Truncated portable archive entry")
        hashes[relative] = hashed.hexdigest()
        if capture:
            record = json.loads(b"".join(chunks))
            if not isinstance(record, dict):
                raise ValueError("Portable build metadata must be an object")
            captured[relative] = record

    if archive.name.endswith(".zip"):
        # Refuse oversized central-directory allocation before ZipFile parses it.
        with archive.open("rb") as raw:
            size = archive.stat().st_size
            raw.seek(max(0, size - 65557))
            tail = raw.read(65557)
        at = tail.rfind(b"PK\x05\x06")
        if at < 0 or at + 22 > len(tail):
            raise ValueError("Missing portable ZIP end record")
        _, disk, start_disk, on_disk, count, directory_size, offset, comment = struct.unpack_from("<4s4H2IH", tail, at)
        if (disk or start_disk or count != on_disk or count > 8192 or directory_size > 8 * 1024**2
                or at + 22 + comment != len(tail) or offset + directory_size != size - len(tail) + at):
            raise ValueError("Invalid or unbounded portable ZIP directory")
        with zipfile.ZipFile(archive) as source:
            entries = source.infolist()
            if len(entries) > 8192:
                raise ValueError("Too many portable ZIP entries")
            for item in entries:
                mode = item.external_attr >> 16
                if item.is_dir() or not stat.S_ISREG(mode) or item.flag_bits & 1:
                    raise ValueError("Portable ZIP needs only regular unencrypted files")
                with source.open(item) as content:
                    entry(item.filename, item.file_size, content)
    else:
        # Plain packager headers only: inspect size/type before consuming data.
        # TarFile would read PAX/long-name payloads before yielding an entry.
        with gzip.open(archive, "rb") as source:
            while True:
                header = source.read(512)
                if header == bytes(512):
                    if source.read(512) != bytes(512):
                        raise ValueError("Incomplete portable TAR terminator")
                    tail = source.read(10241)
                    if len(tail) > 10240 or any(tail):
                        raise ValueError("Unexpected portable TAR trailing data")
                    break
                if len(header) != 512:
                    raise ValueError("Truncated portable TAR header")
                item = tarfile.TarInfo.frombuf(header, "utf-8", "strict")
                if not item.isreg():
                    raise ValueError("Portable TAR needs only plain regular files")
                entry(item.name, item.size, source)
                padding = (-item.size) % 512
                if source.read(padding) != bytes(padding):
                    raise ValueError("Invalid portable TAR entry padding")
    return hashes, captured


def source_digest(revision):
    """Match the deterministic tracked source member emitted by the packager."""
    epoch = int(subprocess.check_output(["git", "show", "-s", "--format=%ct", revision], cwd=ROOT))
    with tempfile.TemporaryDirectory(prefix="orv-source-check-") as temporary:
        raw, compressed = Path(temporary) / "source.tar", Path(temporary) / "source.tar.gz"
        subprocess.run(["git", "archive", "--format=tar", "--prefix=openradview-source/", "--output=" + str(raw), revision], cwd=ROOT, check=True)
        with raw.open("rb") as source, compressed.open("xb") as output:
            with gzip.GzipFile(fileobj=output, mode="wb", filename="", mtime=epoch) as gz:
                while chunk := source.read(512 * 1024):
                    gz.write(chunk)
        return digest(compressed)


def acceptance(path, target, version, revision, archive_hash, installer_hash=None):
    if path.stat().st_size > 64 * 1024:
        raise ValueError("Native acceptance record is too large")
    record = json.loads(path.read_text())
    if record.get("schema_version") == 2:
        native_evidence.ci_acceptance(record, target=target, version=version, revision=revision,
                                      archive_hash=archive_hash, installer_hash=installer_hash)
        return
    if (record.get("schema_version") != 1 or record.get("target") != target
            or record.get("version") != version or record.get("source_revision") != revision
            or record.get("archive_sha256") != archive_hash
            or not isinstance(record.get("checks"), dict) or set(record["checks"]) != CHECKS
            or any(value is not True for value in record["checks"].values())
            or not isinstance(record.get("evidence"), str) or not 1 <= len(record["evidence"].strip()) <= 4096):
        raise ValueError("Native acceptance must cover this exact archive and every required check")


def verify(folder, target, version, revision, *, require_acceptance, require_installer=False):
    if target not in TARGETS or not re.fullmatch(r"\d+\.\d+\.\d+", version) or not re.fullmatch(r"[a-f0-9]{40}", revision):
        raise ValueError("Unsupported portable release identity")
    name, bundle = update_protocol.names(version, target)
    archive = local(folder, name)
    if not 0 < archive.stat().st_size <= 512 * 1024**2:
        raise ValueError("Portable release archive exceeds its bound")
    checksum = local(folder, name + ".sha256")
    archive_hash = digest(archive)
    if checksum.read_text() != archive_hash + "  " + name + "\n":
        raise ValueError("Portable checksum differs from the exact archive")
    hashes, metadata = inventory(archive, bundle)
    build, layout = metadata.get("build.json", {}), metadata.get("update-layout.json", {})
    if (build.get("version") != version or build.get("source_commit") != revision
            or build.get("target") != target or build.get("build_host") != target
            or build.get("source_dirty") is not False or build.get("profile") != "release"
            or build.get("features") != ["mcp", "updates"]
            or build.get("updater") != update_protocol.configuration()):
        raise ValueError("Portable release requires an exact clean optimized native-host build")
    if (layout.get("schema_version") != 1 or layout.get("product") != "OpenRadView"
            or layout.get("target") != target or not isinstance(layout.get("files"), list)
            or any(not isinstance(name, str) for name in layout["files"])
            or len(layout["files"]) != len(hashes) or set(layout["files"]) != set(hashes)
            or build.get("files") != {n: h for n, h in hashes.items() if n != "build.json"}):
        raise ValueError("Portable file inventory differs from its exact archive")
    suffix = ".exe" if "windows" in target else ""
    required = {"openradview" + suffix, "openradview-mcp" + suffix, "LICENSE", "PRIVACY.md", "DISCLAIMER.md", "THIRD-PARTY-NOTICES.txt", "source.tar.gz"}
    if not required <= set(hashes) or hashes["source.tar.gz"] != source_digest(revision):
        raise ValueError("Portable source/notices do not match the committed release")
    feed = local(folder, f"update-{target}.json")
    update_protocol.verify(feed, target, archive, version)
    names = [name, checksum.name, feed.name]
    installer_hash = None
    if require_installer:
        selected, installer_hash = native_evidence.installer(folder, target, version, revision, archive, hashes,
                                                             digest=digest, local=local)
        names.extend(selected)
    report_name = f"acceptance-{target}.json"
    report = folder / report_name
    accepted = report.exists()
    if accepted:
        acceptance(local(folder, report_name), target, version, revision, archive_hash, installer_hash)
        names.append(report_name)
    elif require_acceptance:
        raise ValueError("Native runtime/update acceptance is required before publication: " + target)
    return names, accepted
