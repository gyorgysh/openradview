#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Freeze exact release files; publish a normal immutable release after approval."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile
import tomllib

from common import DIST, ROOT, inventory, run, sha256, write_json
from verify import verify

REPOSITORY = "gyorgysh/openradview"
sys.path.insert(0, str(ROOT / "scripts/portable"))
import release_checks
from github_releases import find_release


def local_file(folder, name):
    if not isinstance(name, str) or Path(name).name != name or name in ("", ".", ".."):
        raise ValueError("Release assets must be plain filenames")
    path = folder / name
    if path.is_symlink() or not path.is_file():
        raise ValueError("Missing or symlinked release asset: " + name)
    return path


def verify_edition(folder, stem, *, require_notarized):
    from dmg import audit, tooling
    tooling()
    archive_record = local_file(folder, stem + ".json")
    image_record = local_file(folder, stem + ".dmg.json")
    z, d = json.loads(archive_record.read_text()), json.loads(image_record.read_text())
    build = z["build"]
    if build["profile"] != "release" or build["source_dirty"] or build["signing"] != "Developer ID" or build["features"] != ["mcp", "updates"] or build.get("edition") != "standard":
        raise ValueError("Only clean optimized Developer ID standard artifacts may be staged")
    if not z.get("archive_verified") or d["build"] != build or d["development"] or d["signing"] != "Developer ID" or d["verified"]["app_files"] != z["bundle_files"]:
        raise ValueError("ZIP and installer must describe the same verified sealed app")
    for name_key, hash_key, record in [("archive", "archive_sha256", z), ("source_archive", "source_archive_sha256", z), ("dmg", "dmg_sha256", d)]:
        if sha256(local_file(folder, record[name_key])) != record[hash_key]:
            raise ValueError("Release artifact hash mismatch")
    notarized = z.get("notarized") is True and d.get("notarized") is True and d.get("app_notarized") is True
    if require_notarized and not notarized:
        raise ValueError("Publication requires notarized/stapled apps AND disk images; no upload performed")
    with tempfile.TemporaryDirectory(prefix=".orv-release-verify-", dir=DIST) as temporary:
        run(["ditto", "-x", "-k", str(local_file(folder, z["archive"])), temporary])
        app = Path(temporary) / "OpenRadView.app"
        # Staging can precede notarization; already stapled inputs still need ticket validation.
        if verify(app, distribution=True, notarized=z.get("notarized") is True) != build or inventory(app) != z["bundle_files"]:
            raise ValueError("ZIP contents differ from the sealed app record")
    audit(local_file(folder, d["dmg"]), build, z["bundle_files"], notarized=d.get("notarized") is True,
          app_notarized=d.get("app_notarized", False), signing="Developer ID", development=False)
    if z["archive"] != stem + ".zip" or d["dmg"] != stem + ".dmg" or build["version"] not in stem:
        raise ValueError("Release record filenames differ from the versioned stem")
    for filename, digest in [(z["archive"], z["archive_sha256"]), (d["dmg"], d["dmg_sha256"])]:
        if local_file(folder, filename.replace(".zip", ".sha256") if filename.endswith(".zip") else filename + ".sha256").read_text() != digest + "  " + filename + "\n":
            raise ValueError("Checksum sidecar differs from its exact archive")
    with tempfile.TemporaryDirectory(prefix=".orv-source-check-", dir=DIST) as temporary:
        source = Path(temporary) / "source.tar.gz"
        run(["git", "archive", "--format=tar.gz", "--prefix=OpenRadView/", "--output=" + str(source), build["source_revision"]])
        if sha256(source) != z["source_archive_sha256"]:
            raise ValueError("Source archive differs from the exact Git revision")
    names = [stem + suffix for suffix in (".zip", ".json", ".sha256", ".dmg", ".dmg.json", ".dmg.sha256")] + [z["source_archive"]]
    for name in names:
        local_file(folder, name)
    return build, names, notarized


def prepare(architecture, portable_targets=()):
    if run(["git", "status", "--porcelain", "--untracked-files=normal"], capture=True).strip():
        raise ValueError("Commit and validate source before freezing release artifacts")
    version = tomllib.loads((ROOT / "Cargo.toml").read_text())["workspace"]["package"]["version"]
    if not re.fullmatch(r"\d+\.\d+\.\d+", version):
        raise ValueError("Expected numeric release version")
    revision = run(["git", "rev-parse", "HEAD"], capture=True).strip()
    stems, names, pending = [], set(), []
    portable = []
    if len(set(portable_targets)) != len(portable_targets):
        raise ValueError("Select each portable target only once")
    for edition in ("standard",):
        stem = f"OpenRadView-{version}-macos-{architecture}-{edition}"
        build, selected, notarized = verify_edition(DIST, stem, require_notarized=False)
        if build["source_revision"] != revision or build["version"] != version or build["features"] != ["mcp", "updates"]:
            raise ValueError("The app must match this exact committed source/version")
        if build.get("updater", {}).get("protocol") != "ed25519-manifest-v1":
            raise ValueError("Release requires the shared signed-manifest updater")
        sys.path.insert(0,str(ROOT / "scripts"))
        import update_protocol
        feed = f"update-{build['target']}.json"
        update_protocol.verify(local_file(DIST,feed),build["target"],local_file(DIST,stem+".zip"),version)
        stems.append(stem); names.update([*selected, feed])
        if not notarized:
            pending.append(edition + ": notarization/stapling remains required")
    for target in sorted(portable_targets):
        selected, accepted = release_checks.verify(DIST, target, version, revision, require_acceptance=False, require_installer=True)
        portable.append({"target": target, "acceptance": accepted, "installer": True})
        names.update(selected)
        if not accepted:
            pending.append(target + ": native runtime/update acceptance remains required")
    assets = [{"name": n, "size": local_file(DIST, n).stat().st_size, "sha256": sha256(local_file(DIST, n))} for n in sorted(names)]
    payload_hash = hashlib.sha256(json.dumps(assets, sort_keys=True).encode()).hexdigest()[:12]
    destination = DIST / "releases" / ("v" + version + "-" + revision[:12] + "-" + payload_hash)
    record = {"schema_version": 1, "repository": REPOSITORY, "tag": "v" + version, "source_revision": revision,
              "editions": stems, "assets": assets, "publication_pending": pending,
              "policy": "Approved protected release build. Publish once with server immutability enabled; never overwrite."}
    record["update_protocol"] = "ed25519-manifest-v1"
    if portable:
        record["portable"] = portable
    if destination.exists():
        if json.loads((destination / "release.json").read_text()) != record:
            raise ValueError("Frozen candidate already exists with different contents; use a new version")
        validate_payload(destination)
        return destination, record
    destination.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix=".candidate-", dir=destination.parent) as temporary:
        staged = Path(temporary) / "payload"
        staged.mkdir()
        for name in sorted(names):
            shutil.copyfile(local_file(DIST, name), staged / name)
        write_json(staged / "release.json", record)
        validate_payload(staged)
        staged.rename(destination)
    return destination, record


def validate_payload(folder):
    record = json.loads(local_file(folder, "release.json").read_text())
    if record.get("schema_version") != 1 or record.get("repository") != REPOSITORY or not re.fullmatch(r"v\d+\.\d+\.\d+", record.get("tag", "")) or not re.fullmatch(r"[a-f0-9]{40}", record.get("source_revision", "")):
        raise ValueError("Invalid frozen release identity")
    version = record["tag"][1:]
    editions = record.get("editions")
    if not isinstance(editions, list) or len(editions) != 1 or not re.fullmatch(r"OpenRadView-" + re.escape(version) + r"-macos-(arm64|x86_64)-standard", editions[0]):
        raise ValueError("Exactly one canonical standard edition is required")
    stem = editions[0]
    source = f"OpenRadView-{version}-source-{record['source_revision'][:12]}.tar.gz"
    protocol = record.get("update_protocol")
    if protocol != "ed25519-manifest-v1":
        raise ValueError("Unknown update protocol")
    target = "aarch64-apple-darwin" if "-macos-arm64-" in stem else "x86_64-apple-darwin"
    feed = f"update-{target}.json"
    canonical = {stem + s for s in (".zip", ".json", ".sha256", ".dmg", ".dmg.json", ".dmg.sha256")} | {source, feed}
    portable = record.get("portable", [])
    if not isinstance(portable, list) or len(portable) > len(release_checks.TARGETS):
        raise ValueError("Invalid portable target selection")
    seen = set()
    for platform in portable:
        if (not isinstance(platform, dict) or set(platform) not in ({"target", "acceptance"}, {"target", "acceptance", "installer"})
                or not isinstance(platform["target"], str)
                or platform["target"] not in release_checks.TARGETS or platform["target"] in seen
                or type(platform["acceptance"]) is not bool or protocol != "ed25519-manifest-v1"):
            raise ValueError("Invalid or duplicate portable release target")
        target = platform["target"]
        seen.add(target)
        archive, _ = release_checks.update_protocol.names(version, target)
        canonical.update({archive, archive + ".sha256", f"update-{target}.json"})
        if platform.get("installer") is True:
            installer_names = release_checks.native_evidence.installer_names(version, target)
            if not installer_names:
                raise ValueError("Missing canonical native installer")
            canonical.update(installer_names)
        elif "installer" in platform:
            raise ValueError("Invalid native installer selection")
        if platform["acceptance"]:
            canonical.add(f"acceptance-{target}.json")
    if not isinstance(record.get("assets"), list) or {a.get("name") for a in record["assets"]} != canonical:
        raise ValueError("Release must contain the exact nonempty app/installer/source/feed asset set")
    expected = {"release.json"}
    for asset in record["assets"]:
        path = local_file(folder, asset["name"])
        if path.stat().st_size != asset["size"] or sha256(path) != asset["sha256"] or asset["name"] in expected:
            raise ValueError("Frozen release asset changed or is duplicated")
        expected.add(asset["name"])
    if {p.name for p in folder.iterdir()} != expected:
        raise ValueError("Unexpected file in the frozen release payload")
    return record


def github(path, *, missing=False):
    result = subprocess.run(["gh", "api", "-H", "X-GitHub-Api-Version: 2026-03-10", path], capture_output=True, text=True)
    if result.returncode:
        if missing and "HTTP 404" in result.stderr:
            return None
        raise ValueError("GitHub preflight failed; verify access/network locally. Nothing was published.")
    return json.loads(result.stdout)


def check_remote_assets(remote, assets):
    expected = {a["name"]: a for a in assets}
    actual = {a["name"]: a for a in remote["assets"]}
    if len(actual) != len(remote["assets"]) or set(actual) != set(expected):
        raise ValueError("Release must contain exactly the frozen assets; no automatic replacement is allowed")
    for name, a in actual.items():
        if a.get("state") != "uploaded" or a.get("size") != expected[name]["size"] or a.get("digest") != "sha256:" + expected[name]["sha256"]:
            raise ValueError("GitHub release asset digest/size/state differs from the frozen payload")


def reviewed_ci(revision):
    if (os.environ.get("GITHUB_ACTIONS") != "true"
            or os.environ.get("GITHUB_REPOSITORY") != REPOSITORY
            or os.environ.get("ORV_RELEASE_APPROVAL_READY") != "reviewers-v1"
            or os.environ.get("GITHUB_SHA") != revision
            or os.environ.get("GITHUB_RUN_ATTEMPT") != "1"
            or not re.fullmatch(r"refs/tags/v\d+\.\d+\.\d+", os.environ.get("GITHUB_REF", ""))):
        raise ValueError("Publication requires the exact approved tagged CI source")
    run_id = os.environ.get("GITHUB_RUN_ID", "")
    if not run_id.isdigit():
        raise ValueError("Invalid release workflow identity")
    repo = github(f"repos/{REPOSITORY}")
    current = github(f"repos/{REPOSITORY}/actions/runs/{run_id}")
    if (repo.get("private") is not False or current.get("head_sha") != revision
            or current.get("run_attempt") != 1 or current.get("event") not in ("push", "workflow_dispatch")
            or current.get("path") != ".github/workflows/release.yml"):
        raise ValueError("Expected the public protected release workflow")
    reviews = github(f"repos/{REPOSITORY}/actions/runs/{run_id}/approvals")
    relevant = [r for r in reviews if any(e.get("name") == "release" for e in r.get("environments", []))]
    if (any(r.get("state") == "rejected" for r in relevant)
            or not any(r.get("state") == "approved" and r.get("user", {}).get("id") == 295578481 for r in relevant)):
        raise ValueError("The owner must approve this run's release environment")


def immutability_gate(revision, *, ci_release=False):
    if ci_release:
        # The job token cannot read Administration settings. Owner arming checks
        # immutability; this source's real environment review is checked here.
        # Publication also requires GitHub's immutable=true response below.
        reviewed_ci(revision)
    elif github(f"repos/{REPOSITORY}/immutable-releases").get("enabled") is not True:
        raise ValueError("Enable GitHub release immutability before publishing")


def publish(folder, notes, *, ci_release=False):
    record = validate_payload(folder)
    # Actual local notarization/signature checks precede ALL GitHub mutations.
    for stem in record["editions"]:
        build, _, _ = verify_edition(folder, stem, require_notarized=True)
        if build["source_revision"] != record["source_revision"] or build["version"] != record["tag"][1:]:
            raise ValueError("Frozen build does not match the release identity")
    for platform in record.get("portable", []):
        options = {"require_acceptance": True}
        if platform.get("installer") is True:
            options["require_installer"] = True
        release_checks.verify(folder, platform["target"], record["tag"][1:], record["source_revision"], **options)
        evidence = release_checks.native_evidence.read(local_file(folder, f"acceptance-{platform['target']}.json"))
        if evidence.get("schema_version") == 2:
            release_checks.native_evidence.verify_run(evidence, lambda path: github(f"repos/{REPOSITORY}/{path}"))
    expected_targets = {"x86_64-pc-windows-msvc", "x86_64-unknown-linux-gnu"}
    if ({p["target"] for p in record.get("portable", [])} != expected_targets
            or any(p.get("installer") is not True or p["acceptance"] is not True for p in record["portable"])):
        raise ValueError("Publication requires both native Windows/Linux installers and acceptance records")
    sys.path.insert(0, str(ROOT / "scripts"))
    import update_protocol
    update_protocol.verify(local_file(folder, f"update-{build['target']}.json"), build["target"], local_file(folder, record["editions"][0]+".zip"), build["version"])
    tag = record["tag"]
    peeled = run(["git", "ls-remote", "origin", f"refs/tags/{tag}^{{}}"], capture=True).split()
    if not peeled or peeled[0] != record["source_revision"]:
        raise ValueError("Push the matching annotated source tag first; tags are never created or moved here")
    if github(f"repos/{REPOSITORY}").get("full_name", "").lower() != REPOSITORY:
        raise ValueError("Unexpected release repository")
    immutability_gate(record["source_revision"], ci_release=ci_release)
    assets = record["assets"] + [{"name": "release.json", "size": (folder / "release.json").stat().st_size, "sha256": sha256(folder / "release.json")}]
    remote = find_release(tag, lambda path: github(f"repos/{REPOSITORY}/{path}"))
    if remote is not None:
        raise ValueError("A release already exists. Inspect it; this tool never overwrites or blindly retries uploads.")
    if notes is None or notes.is_symlink() or not notes.is_file():
        raise ValueError("An explicit reviewed --notes-file is required")
    run(["gh", "release", "create", tag, "--repo", REPOSITORY, "--verify-tag", "--notes-file", str(notes),
         *[str(local_file(folder, a["name"])) for a in assets]])
    result = find_release(tag, lambda path: github(f"repos/{REPOSITORY}/{path}"))
    if result is None or result.get("draft") is not False or result.get("immutable") is not True:
        raise ValueError("Publication state is uncertain; inspect GitHub. Do not retry or replace assets.")
    check_remote_assets(result, assets)
    print("Published immutable release; tag and assets verified by GitHub.")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--publish", type=Path)
    parser.add_argument("--architecture", choices=["arm64", "x86_64"], default="arm64")
    parser.add_argument("--notes-file", type=Path)
    parser.add_argument("--portable-target", action="append", choices=sorted(release_checks.TARGETS), default=[], help="include an exact native portable archive; no cross-build previews")
    args = parser.parse_args()
    if args.publish:
        if args.portable_target:
            parser.error("Portable selection is frozen during preparation, not publication")
        publish(args.publish.resolve(strict=True), args.notes_file)
    else:
        destination, record = prepare(args.architecture, args.portable_target)
        print("Frozen locally: " + str(destination.relative_to(ROOT)))
        print("No upload performed. " + ("; ".join(record["publication_pending"]) or "Protected release approval and explicit publication remain required."))


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, KeyError, subprocess.CalledProcessError) as error:
        print("Release stopped: " + str(error), file=sys.stderr)
        sys.exit(1)
