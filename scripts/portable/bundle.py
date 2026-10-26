#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Local Linux/Windows archives. No publishing, signing, installation or network requests."""
import argparse
import gzip
import hashlib
import json
import os
from pathlib import Path
import shutil
import struct
import subprocess
import sys
import tarfile
import tempfile
import time
import tomllib
import zipfile

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts/macos"))
from notices import collect_notices
import vendor_sources
sys.path.insert(0, str(ROOT / "scripts"))
import update_protocol

TARGETS = {"x86_64-unknown-linux-gnu", "aarch64-unknown-linux-gnu", "x86_64-pc-windows-gnu", "x86_64-pc-windows-msvc", "aarch64-pc-windows-msvc"}
SYSTEM_DLLS = {"kernel32.dll", "ntdll.dll", "shell32.dll", "user32.dll", "dwmapi.dll", "imm32.dll", "userenv.dll", "uxtheme.dll", "ws2_32.dll", "bcryptprimitives.dll", "bcrypt.dll", "dxgi.dll", "gdi32.dll", "ole32.dll", "oleaut32.dll", "propsys.dll", "setupapi.dll", "uiautomationcore.dll", "combase.dll", "rpcrt4.dll", "advapi32.dll", "d3d12.dll", "d3dcompiler_47.dll", "dwrite.dll", "crypt32.dll", "secur32.dll", "winmm.dll", "version.dll", "shcore.dll", "ucrtbase.dll", "dbghelp.dll", "dinput8.dll"}

def run(args, *, env=None):
    return subprocess.check_output(args, cwd=ROOT, env=env, text=True).strip()

def digest(path):
    with path.open("rb") as f:
        return hashlib.file_digest(f, "sha256").hexdigest()

def pe_imports(path):
    """Read PE32+ import descriptors; fail before archiving unknown runtime DLLs."""
    b = path.read_bytes()
    if len(b)<64:
        raise ValueError("PE file is truncated")
    pe = struct.unpack_from("<I", b, 60)[0]
    if b[:2] != b"MZ" or b[pe:pe+4] != b"PE\0\0":
        raise ValueError("Not a PE executable")
    count, size = struct.unpack_from("<H", b, pe+6)[0], struct.unpack_from("<H", b, pe+20)[0]
    optional = pe+24
    if struct.unpack_from("<H", b, optional)[0] != 0x20b:
        raise ValueError("Expected a 64-bit PE executable")
    sections = []
    for i in range(count):
        at = optional+size+i*40
        virtual_size, virtual, raw_size, raw = struct.unpack_from("<IIII", b, at+8)
        sections.append((virtual, raw_size, raw))
    def offset(rva):
        for virtual, raw_size, raw in sections:
            if virtual <= rva < virtual+raw_size and raw+rva-virtual < len(b):
                return raw+rva-virtual
        raise ValueError("PE address is outside file-backed sections")
    rva = struct.unpack_from("<I", b, optional+120)[0]
    if not rva:
        return []
    at = offset(rva)
    names = set()
    for i in range(256):
        record = struct.unpack_from("<IIIII", b, at+i*20)
        if not any(record):
            return sorted(names)
        start = offset(record[3])
        end = b.find(b"\0", start, start+260)
        if end == -1:
            raise ValueError("Invalid DLL name")
        name = b[start:end].decode("ascii").lower()
        if "/" in name or "\\" in name:
            raise ValueError("DLL import contains a path")
        names.add(name)
    raise ValueError("Too many DLL imports")

def inspect_binary(path, target):
    if "windows" in target:
        imports = pe_imports(path)
        unknown = [n for n in imports if n not in SYSTEM_DLLS and not n.startswith(("api-ms-win-", "ext-ms-win-"))]
        if unknown:
            raise ValueError("Non-system DLL imports require review; no incomplete archive will be made: " + ", ".join(unknown))
        return {"system_dll_imports": imports}
    executable = shutil.which("readelf") or shutil.which("llvm-readelf")
    if not executable:
        raise ValueError("Install binutils/readelf to inspect Linux dependencies")
    output = run([executable, "--dynamic", str(path)])
    import re
    needed = re.findall(r"\(NEEDED\).*?\[(.*?)\]", output)
    versions = run([executable, "--version-info", str(path)])
    glibc = sorted(set(re.findall(r"\bGLIBC_([0-9.]+)", versions)), key=lambda s:tuple(int(n) for n in s.split('.')))
    return {"needed_libraries": sorted(needed), "maximum_glibc_symbol": glibc[-1] if glibc else None}

def archive(folder, destination, epoch):
    """Only regular staged files; fixed order, timestamp and executable permissions."""
    files = sorted(folder.rglob("*"))
    if any(p.is_symlink() for p in files):
        raise ValueError("Archive symlinks are not allowed")
    with destination.open("xb") as output:
        try:
            if destination.suffix == ".zip":
                stamp = time.gmtime(max(315532800, epoch))[:6]
                with zipfile.ZipFile(output, "w", zipfile.ZIP_DEFLATED, compresslevel=9) as z:
                    for p in files:
                        if p.is_file():
                            info = zipfile.ZipInfo(p.relative_to(folder.parent).as_posix(), stamp)
                            info.create_system = 3
                            info.external_attr = (0o100755 if os.access(p, os.X_OK) else 0o100644) << 16
                            info.compress_type = zipfile.ZIP_DEFLATED
                            z.writestr(info, p.read_bytes())
            else:
                with gzip.GzipFile(fileobj=output, mode="wb", filename="", mtime=epoch) as gz:
                    with tarfile.open(fileobj=gz, mode="w") as tar:
                        for p in files:
                            if p.is_file():
                                info = tarfile.TarInfo(p.relative_to(folder.parent).as_posix())
                                info.size = p.stat().st_size
                                info.mtime = epoch
                                info.mode = 0o755 if os.access(p, os.X_OK) else 0o644
                                with p.open("rb") as content:
                                    tar.addfile(info, content)
        except BaseException:
            output.close()
            destination.unlink(missing_ok=True)
            raise


def main():
    vendor_record = vendor_sources.receipt(ROOT)
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--target", choices=sorted(TARGETS))
    parser.add_argument("--development", action="store_true", help="debug preview; cross-host builds and dirty source allowed")
    args = parser.parse_args()
    host = next(s[6:] for s in run(["rustc", "-vV"]).splitlines() if s.startswith("host: "))
    target = args.target or host
    if target not in TARGETS:
        parser.error("Use the macOS bundle script on macOS, or select a supported portable target")
    dirty = bool(run(["git", "status", "--porcelain", "--untracked-files=normal"]))
    if not args.development and (dirty or target != host):
        parser.error("Optimized portable archives require clean committed source and the matching native host")
    version = tomllib.loads((ROOT / "Cargo.toml").read_text())["workspace"]["package"]["version"]
    commit = run(["git", "rev-parse", "HEAD"])
    epoch = int(run(["git", "show", "-s", "--format=%ct", "HEAD"]))
    env = os.environ.copy()
    if "windows-gnu" in target:
        env["CXXSTDLIB_STATIC_"+target.replace("-","_")] = "1"
    if "windows-msvc" in target:
        if env.get("CARGO_ENCODED_RUSTFLAGS"):
            parser.error("Use RUSTFLAGS, not CARGO_ENCODED_RUSTFLAGS, for the static MSVC runtime build")
        env["RUSTFLAGS"] = env.get("RUSTFLAGS", "") + " -C target-feature=+crt-static"
    command = ["cargo", "build", "--locked", "--offline", "-p", "orv-app", "-p", "orv-mcp", "--bin", "openradview", "--bin", "openradview-mcp", "--features", "orv-app/mcp,orv-app/updates", "--target", target]
    if not args.development:
        command.append("--release")
    subprocess.run(command, cwd=ROOT, env=env, check=True)
    profile = "debug" if args.development else "release"
    binaries = ROOT / "target" / target / profile
    suffix = ".exe" if "windows" in target else ""
    dependencies = {name:inspect_binary(binaries / (name+suffix), target) for name in ("openradview", "openradview-mcp")}
    notices, count, _ = collect_notices(target, True, updates=True)
    dist = ROOT / "dist"
    dist.mkdir(exist_ok=True)
    stem = f"OpenRadView-{version}-{target}" + ("-development" if args.development else "")
    extension = ".zip" if "windows" in target else ".tar.gz"
    output = dist / (stem+extension)
    if output.exists():
        raise ValueError("Archive already exists; choose a new version or move the old artifact")
    with tempfile.TemporaryDirectory(prefix=".orv-portable-", dir=dist) as temporary:
        folder = Path(temporary) / stem
        folder.mkdir()
        for name in ("openradview", "openradview-mcp"):
            shutil.copyfile(binaries / (name+suffix), folder / (name+suffix))
            (folder / (name+suffix)).chmod(0o755)
        for name in ("LICENSE", "DISCLAIMER.md", "PRIVACY.md", "README.md"):
            shutil.copyfile(ROOT / name, folder / name)
        for font_notice in sorted((ROOT / "assets/fonts").glob("*-OFL.txt")):
            shutil.copyfile(font_notice,folder/font_notice.name)
        (folder / "THIRD-PARTY-NOTICES.txt").write_text(notices, encoding="utf-8")
        shutil.copyfile(ROOT / "docs/platforms.md", folder / "PLATFORMS.md")
        if "linux" in target:
            shutil.copyfile(ROOT / "assets/brand/app-icon.png", folder / "openradview.png")
            (folder / "openradview.desktop").write_text("[Desktop Entry]\nType=Application\nName=OpenRadView\nComment=Personal image exploration\nExec=openradview\nIcon=openradview\nTerminal=false\nCategories=Education;Science;\n")
        if not dirty:
            source = subprocess.check_output(["git", "archive", "--format=tar", "--prefix=openradview-source/", "HEAD"], cwd=ROOT)
            with (folder / "source.tar.gz").open("xb") as f:
                with gzip.GzipFile(fileobj=f, mode="wb", filename="", mtime=epoch) as gz:
                    gz.write(source)
        update_protocol.write_layout(folder, target)
        manifest = {"version":version,"source_commit":commit,"source_dirty":dirty,"target":target,"build_host":host,"profile":profile,"features":["mcp","updates"],"updater":update_protocol.configuration(),"signed":False,"runtime_validation":"Not established by packaging; see PLATFORMS.md", "dependency_notices":count,"vendored_sources":vendor_record,"binary_dependencies":dependencies,"files":{p.name:digest(p) for p in sorted(folder.iterdir()) if p.is_file()}}
        (folder / "build.json").write_text(json.dumps(manifest, indent=2, sort_keys=True)+"\n")
        archive(folder, output, epoch)
    (dist / (output.name+".sha256")).write_text(digest(output)+"  "+output.name+"\n")
    print("Built local unsigned archive:", output)

if __name__ == "__main__":
    main()
