# SPDX-License-Identifier: GPL-3.0-or-later
"""Collect notices for exact, target-selected normal Cargo dependencies offline."""
import json
from pathlib import Path

from common import ROOT, run, sha256
import vendor_sources

PREFIXES = ("license", "licence", "copying", "copyright", "notice")


def collect_notices(target, mcp, qa=False, updates=False):
    if mcp and qa:
        raise ValueError("MCP and QA are separate build variants")
    selected = (["mcp"] if mcp else (["qa"] if qa else [])) + (["updates"] if updates else [])
    features = ["--features", ",".join("orv-app/" + f for f in selected)] if selected else []
    metadata = json.loads(run(["cargo", "metadata", "--locked", "--offline", "--format-version", "1",
                               "--filter-platform", target, *features], capture=True))
    tree = run(["cargo", "tree", "--locked", "--offline", "-p", "orv-app", "-e", "normal",
                "--prefix", "none", "--target", target,
                *(["--features", ",".join(selected)] if selected else [])], capture=True)
    keys = {tuple(line.split()[:2]) for line in tree.splitlines() if len(line.split()) >= 2}
    index = json.loads((ROOT / "assets/licenses/index.json").read_text())["packages"]
    text = ["OpenRadView third-party dependency notices\n",
            "Exact normal dependencies selected by Cargo for this target/build feature set.\n",
            "Original license/copyright text follows; these notices are not relicensed under GPL.\n"]
    count = 0
    vendored = vendor_sources.verify(ROOT)
    workspace = set(metadata.get("workspace_members", []))
    for package in sorted(metadata["packages"], key=lambda p: (p["name"], p["version"])):
        if (package["name"], "v" + package["version"]) not in keys or package.get("id") in workspace:
            continue
        key = package["name"] + "@" + package["version"]
        folder = Path(package["manifest_path"]).parent
        if not package["source"]:
            reviewed = vendored.get(key)
            if not reviewed or folder.resolve() != reviewed["folder"]:
                raise ValueError("Review this local third-party dependency before packaging: " + key)
        if not package["license"]:
            raise ValueError("Missing dependency license declaration: " + key)
        local = sorted(p for p in folder.iterdir() if p.is_file() and p.name.lower().startswith(PREFIXES))
        notices = []
        for path in local:
            if not path.resolve().is_relative_to(folder.resolve()):
                raise ValueError("Dependency notice escapes its package: " + key)
            notices.append((path.name, path.read_text()))
        entry = index.get(key)
        requires_supplement = any(k.startswith(package["name"] + "@") and v.get("supplemental")
                                  for k, v in index.items())
        if requires_supplement and (not entry or not entry.get("supplemental")):
            raise ValueError("Review exact-version bundled-code notices before packaging: " + key)
        if not notices or (entry and entry.get("supplemental")):
            entry = index.get(key)
            if not entry or entry["license"] != package["license"]:
                raise ValueError("Review and supply exact-version notices before packaging: " + key)
            for notice in entry["notices"]:
                path = ROOT / "assets/licenses" / notice["file"]
                if not path.resolve().is_relative_to((ROOT / "assets/licenses").resolve()) or sha256(path) != notice["sha256"]:
                    raise ValueError("Curated dependency notice failed integrity check: " + key)
                notices.append((notice["source"], path.read_text()))
        if not notices:
            raise ValueError("No copyright/license text available: " + key)
        text.append(f"\n{'=' * 72}\n{key}\nLicense: {package['license']}\n")
        if key in vendored and vendored[key]["native"]:
            native = vendored[key]["native"]
            text.append(f"Bundled native code: {native['name']} {native['version']}\n"
                        f"Upstream: {native['repository']} @ {native['revision']}\n")
        if key in vendored:
            text.append(f"Reviewed source inventory SHA-256: {vendored[key]['inventory_sha256']}\n")
        for source, content in notices:
            text.append(f"\n--- {source} ---\n{content}\n")
        count += 1
    text.append("\n" + "=" * 72 + "\nBundled public image datasets (separate from the software license)\n")
    for name in ("CREDITS.txt", "CC-BY-4.0.txt", "CC-BY-3.0.txt"):
        text.append("\n" + (ROOT / "assets/demos" / name).read_text() + "\n")
    return "".join(text), count, metadata
