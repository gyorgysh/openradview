# SPDX-License-Identifier: GPL-3.0-or-later
"""Public update configuration/layout and unsigned manifests. Never reads signing keys."""
import argparse
import base64
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess

ROOT = Path(__file__).resolve().parents[1]

def configuration():
    value = json.loads((ROOT / "packaging/updater.json").read_text())
    if value.get("schema_version") != 1 or value.get("repository") != "gyorgysh/openradview" or len(base64.b64decode(value.get("public_ed25519_key", ""), validate=True)) != 32:
        raise ValueError("Invalid public updater configuration")
    return {"protocol": "ed25519-manifest-v1", "repository": value["repository"], "public_ed25519_key": value["public_ed25519_key"]}

def write_layout(folder, target, *, macos=False):
    relative = "Contents/Resources/update-layout.json" if macos else "update-layout.json"
    files = {p.relative_to(folder).as_posix() for p in folder.rglob("*") if p.is_file()}
    if any(p.is_symlink() for p in folder.rglob("*")):
        raise ValueError("Managed update layouts cannot contain symlinks")
    files.add(relative)
    files.add("Contents/_CodeSignature/CodeResources" if macos else "build.json")
    (folder / relative).write_text(json.dumps({"schema_version":1,"product":"OpenRadView","target":target,"files":sorted(files)},indent=2,sort_keys=True)+"\n")

def names(version, target):
    if target.endswith("apple-darwin"):
        arch = "arm64" if target.startswith("aarch64-") else "x86_64"
        return f"OpenRadView-{version}-macos-{arch}-standard.zip", "OpenRadView.app"
    stem = f"OpenRadView-{version}-{target}"
    return stem + (".zip" if "windows" in target else ".tar.gz"), stem

def unsigned(archive, version, target):
    if not re.fullmatch(r"\d+\.\d+\.\d+", version) or target not in ("aarch64-apple-darwin","x86_64-apple-darwin","x86_64-unknown-linux-gnu","aarch64-unknown-linux-gnu","x86_64-pc-windows-gnu","x86_64-pc-windows-msvc","aarch64-pc-windows-msvc"):
        raise ValueError("Unsupported stable version or update target")
    name, bundle = names(version, target)
    if archive.is_symlink() or archive.name != name or not 0 < archive.stat().st_size <= 512*1024*1024:
        raise ValueError("Update archive identity/size mismatch")
    with archive.open("rb") as source:
        digest = hashlib.file_digest(source,"sha256").hexdigest()
    return (json.dumps({"schema_version":1,"repository":configuration()["repository"],"version":version,"target":target,"archive":name,"bundle":bundle,"bytes":archive.stat().st_size,"sha256":digest},sort_keys=True,separators=(",",":"))+"\n").encode()

def verify(path, target, archive=None, version=None):
    environment = os.environ.copy()
    # Verification/build scripts need no GitHub write token or CI signing secrets.
    for name in ("GH_TOKEN", "GITHUB_TOKEN", "UPDATER_ED25519_PRIVATE_KEY", "DEVELOPER_ID_P12_BASE64", "DEVELOPER_ID_P12_PASSWORD", "NOTARY_KEY_P8_BASE64"):
        environment.pop(name, None)
    subprocess.run(["cargo","run","--locked","--offline","-p","orv-updates","--example","verify_manifest","--",str(path),target],cwd=ROOT,check=True,env=environment)
    if archive is not None:
        envelope=json.loads(path.read_text())
        if base64.b64decode(envelope["manifest"],validate=True) != unsigned(archive,version,target):
            raise ValueError("Signed manifest differs from the exact archive/version")

def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument("archive",type=Path)
    parser.add_argument("--version",required=True)
    parser.add_argument("--target",required=True)
    parser.add_argument("--signature-file",type=Path,help="public base64 signature supplied by the owner; no private-key argument")
    args=parser.parse_args()
    raw=unsigned(args.archive,args.version,args.target)
    if args.signature_file is None:
        path=args.archive.parent / f"update-{args.target}.raw.json"
        with path.open("xb") as output:output.write(raw)
        print("Unsigned manifest prepared:",path)
    else:
        signature=args.signature_file.read_text().strip()
        if len(base64.b64decode(signature,validate=True))!=64:raise ValueError("Expected one base64 Ed25519 signature")
        path=args.archive.parent / f"update-{args.target}.json"
        envelope={"manifest":base64.b64encode(raw).decode(),"signature":signature}
        output=path.open("x")
        try:
            with output:json.dump(envelope,output,separators=(",",":"));output.write("\n")
            verify(path,args.target,args.archive,args.version)
        except BaseException:
            path.unlink(missing_ok=True)
            raise
        print("Verified signed update envelope:",path)
if __name__=="__main__":main()
