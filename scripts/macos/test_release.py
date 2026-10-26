# SPDX-License-Identifier: GPL-3.0-or-later
"""Release integrity/regression checks; native signing is verified separately."""
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch
import zipfile
import copy
import base64
import os

import common
import bundle
import update_protocol
import notarize
import notices
import release
import dmg


class ReleaseTests(unittest.TestCase):
    def test_release_notes_select_one_changelog_section_and_bind_relative_links(self):
        with tempfile.TemporaryDirectory() as folder, patch.object(common,"ROOT",Path(folder)):
            changelog = Path(folder)/"CHANGELOG.md"
            changelog.write_text("# Changelog\n\n## 1.0.0 — In preparation\n\nInitial release. [Privacy](PRIVACY.md)\n\n## 0.1.0\n\nOld internal note.\n")
            notes = common.release_notes("1.0.0")
            self.assertIn("not clinically validated",notes)
            self.assertIn("/blob/v1.0.0/PRIVACY.md",notes)
            self.assertNotIn("Old internal note",notes)
            for text in ("## 1.0.0\n", "## 2.0.0\nOther", "## 1.0.0\nOne\n## 1.0.0\nDuplicate"):
                changelog.write_text(text)
                with self.assertRaises(ValueError): common.release_notes("1.0.0")

    def test_ci_publication_requires_the_exact_run_and_owner_environment_review(self):
        revision = "a" * 40
        environment = {"GITHUB_ACTIONS": "true", "GITHUB_REPOSITORY": release.REPOSITORY,
                       "ORV_RELEASE_APPROVAL_READY": "reviewers-v1", "GITHUB_SHA": revision,
                       "GITHUB_RUN_ATTEMPT": "1", "GITHUB_RUN_ID": "123", "GITHUB_REF": "refs/tags/v1.0.0"}
        review = {"state": "approved", "user": {"id": 295578481}, "environments": [{"name": "release"}]}
        run = {"head_sha": revision, "run_attempt": 1, "event": "push", "path": ".github/workflows/release.yml"}
        responses = {f"repos/{release.REPOSITORY}": {"private": False},
                     f"repos/{release.REPOSITORY}/actions/runs/123": run,
                     f"repos/{release.REPOSITORY}/actions/runs/123/approvals": [review]}
        with patch.dict(os.environ, environment, clear=True), patch.object(release, "github", side_effect=lambda p: responses[p]):
            release.immutability_gate(revision, ci_release=True)
            for key, value in (("GITHUB_SHA", "b" * 40), ("GITHUB_RUN_ATTEMPT", "2"),
                               ("GITHUB_REF", "refs/heads/main"), ("ORV_RELEASE_APPROVAL_READY", "manual-private-v1")):
                with patch.dict(os.environ, {key: value}), self.assertRaises(ValueError):
                    release.immutability_gate(revision, ci_release=True)
            for key, value in (("state", "rejected"), ("user", {"id": 1}), ("environments", [{"name": "other"}])):
                original = review[key]; review[key] = value
                with self.assertRaises(ValueError): release.immutability_gate(revision, ci_release=True)
                review[key] = original
            run["head_sha"] = "b" * 40
            with self.assertRaises(ValueError): release.immutability_gate(revision, ci_release=True)
        with patch.object(release, "github", return_value={"enabled": False}), self.assertRaises(ValueError):
            release.immutability_gate(revision)

    def test_shared_update_layout_manifest_and_overwrite_guard(self):
        with tempfile.TemporaryDirectory() as folder:
            root=Path(folder);app=root/"OpenRadView.app"
            resources=app/"Contents/Resources";resources.mkdir(parents=True)
            (resources/"build.json").write_text("{}")
            update_protocol.write_layout(app,"aarch64-apple-darwin",macos=True)
            layout=json.loads((resources/"update-layout.json").read_text())
            self.assertEqual(set(layout["files"]),{"Contents/Resources/build.json","Contents/Resources/update-layout.json","Contents/_CodeSignature/CodeResources"})
            archive=root/"OpenRadView-1.2.3-macos-arm64-standard.zip";archive.write_bytes(b"synthetic archive")
            raw=update_protocol.unsigned(archive,"1.2.3","aarch64-apple-darwin")
            record=json.loads(raw);self.assertEqual(record["bytes"],17);self.assertEqual(record["bundle"],"OpenRadView.app")
            signature=root/"signature.txt";signature.write_text(base64.b64encode(bytes(64)).decode())
            envelope=root/"update-aarch64-apple-darwin.json";envelope.write_bytes(b"preserve existing immutable feed")
            args=["update_protocol.py",str(archive),"--version","1.2.3","--target","aarch64-apple-darwin","--signature-file",str(signature)]
            with patch.object(sys,"argv",args), self.assertRaises(FileExistsError):update_protocol.main()
            self.assertEqual(envelope.read_bytes(),b"preserve existing immutable feed")
            envelope.unlink()
            with patch.object(sys,"argv",args),patch.object(update_protocol,"verify",side_effect=ValueError("bad signature")), self.assertRaises(ValueError):update_protocol.main()
            self.assertFalse(envelope.exists())

    def test_release_zip_preserves_regular_files_and_refuses_all_symlinks(self):
        with tempfile.TemporaryDirectory() as folder:
            app = Path(folder) / "OpenRadView.app"
            resources = app / "Contents/Resources"; resources.mkdir(parents=True)
            (resources / "example.txt").write_text("synthetic resource")
            original = common.inventory(app)
            archive = Path(folder) / "app.zip"
            common.deterministic_zip(app, archive, 1792900000)
            original_archive = archive.read_bytes()
            extracted = Path(folder) / "extract"
            common.run(["ditto", "-x", "-k", str(archive), str(extracted)])
            self.assertEqual(common.inventory(extracted / app.name), original)
            link = resources / "link"
            for target in ("example.txt", "/etc/hosts", "../../../outside", "link"):
                if link.is_symlink(): link.unlink()
                link.symlink_to(target)
                with self.assertRaises(ValueError): common.inventory(app)
                with self.assertRaises(ValueError): common.deterministic_zip(app, archive, 1792900000)
                self.assertEqual(archive.read_bytes(), original_archive)

    def test_frozen_payload_rejects_empty_edition_bypass_extra_assets_and_tampering(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            stem = "OpenRadView-1.0.0-macos-arm64-standard"
            names = [stem + s for s in (".zip", ".json", ".sha256", ".dmg", ".dmg.json", ".dmg.sha256")] + ["OpenRadView-1.0.0-source-" + "a" * 12 + ".tar.gz", "update-aarch64-apple-darwin.json"]
            assets = []
            for name in names:
                path = root / name
                path.write_bytes(b"synthetic release fixture")
                assets.append({"name": name, "size": path.stat().st_size, "sha256": common.sha256(path)})
            record = {"schema_version": 1, "repository": release.REPOSITORY, "tag": "v1.0.0", "update_protocol": "ed25519-manifest-v1", "source_revision": "a" * 40, "editions": [stem], "assets": assets}
            manifest = root / "release.json"
            common.write_json(manifest, record)
            self.assertEqual(release.validate_payload(root), record)
            for field, value in (("editions", []), ("editions", [stem, stem]), ("assets", []), ("assets", assets + [assets[0]]), ("update_protocol", None), ("update_protocol", "unknown")):
                changed = copy.deepcopy(record)
                changed[field] = value
                common.write_json(manifest, changed)
                with self.assertRaises(ValueError):
                    release.validate_payload(root)
            common.write_json(manifest, record)
            (root / names[0]).write_bytes(b"tampered")
            with self.assertRaises(ValueError):
                release.validate_payload(root)

    def test_github_asset_digest_mismatch_blocks_publication(self):
        asset = {"name": "synthetic.dmg", "size": 12, "sha256": "a" * 64}
        remote = {"assets": [{"name": asset["name"], "size": 12, "state": "uploaded", "digest": "sha256:" + asset["sha256"]}]}
        release.check_remote_assets(remote, [asset])
        for key, value in (("digest", "sha256:" + "b" * 64), ("state", "new"), ("size", 13)):
            changed = copy.deepcopy(remote)
            changed["assets"][0][key] = value
            with self.assertRaises(ValueError):
                release.check_remote_assets(changed, [asset])

    def test_normal_release_creates_once_without_draft_and_checks_immutability(self):
        with tempfile.TemporaryDirectory() as temporary:
            folder = Path(temporary)
            stem = "OpenRadView-1.2.3-macos-arm64-standard"
            revision = "a" * 40
            names = {stem + s for s in (".zip", ".json", ".sha256", ".dmg", ".dmg.json", ".dmg.sha256")}
            names.update({"OpenRadView-1.2.3-source-" + revision[:12] + ".tar.gz", "update-aarch64-apple-darwin.json"})
            platforms = []
            for target in ("x86_64-pc-windows-msvc", "x86_64-unknown-linux-gnu"):
                archive, _ = update_protocol.names("1.2.3", target)
                names.update({archive, archive + ".sha256", f"update-{target}.json", f"acceptance-{target}.json"})
                names.update(release.release_checks.native_evidence.installer_names("1.2.3", target))
                platforms.append({"target": target, "acceptance": True, "installer": True})
            for name in names:
                (folder / name).write_bytes(b"synthetic exact artifact")
            record = {"schema_version": 1, "repository": release.REPOSITORY, "tag": "v1.2.3",
                      "source_revision": revision, "editions": [stem], "update_protocol": "ed25519-manifest-v1",
                      "portable": platforms, "assets": [{"name": n, "size": (folder / n).stat().st_size,
                      "sha256": common.sha256(folder / n)} for n in sorted(names)]}
            common.write_json(folder / "release.json", record)
            assets = [*record["assets"], {"name": "release.json", "size": (folder / "release.json").stat().st_size,
                                        "sha256": common.sha256(folder / "release.json")}]
            remote = {"draft": False, "immutable": True, "assets": [
                {"name": a["name"], "size": a["size"], "digest": "sha256:" + a["sha256"], "state": "uploaded"} for a in assets]}
            notes = folder.parent / "notes.md"; notes.write_text("Reviewed release notes")
            commands = []
            def command(args, **kwargs):
                commands.append(args)
                return revision + " refs/tags/v1.2.3^{}" if args[:2] == ["git", "ls-remote"] else ""
            def api(path):
                return {"enabled": True} if path.endswith("immutable-releases") else {"full_name": release.REPOSITORY}
            build = {"source_revision": revision, "version": "1.2.3", "target": "aarch64-apple-darwin"}
            with patch.object(release, "verify_edition", return_value=(build, [], True)), \
                    patch.object(release.release_checks, "verify"), \
                    patch.object(release.release_checks.native_evidence, "read", return_value={"schema_version": 1}), \
                    patch.object(update_protocol, "verify"), patch.object(release, "run", side_effect=command), \
                    patch.object(release, "github", side_effect=api), \
                    patch.object(release, "find_release", side_effect=[None, remote]):
                release.publish(folder, notes)
            creation = [c for c in commands if c[:3] == ["gh", "release", "create"]]
            self.assertEqual(len(creation), 1)
            self.assertNotIn("--draft", creation[0])
            with patch.object(release, "verify_edition", side_effect=ValueError("Bad sealed app")), \
                    patch.object(release, "github") as github, self.assertRaises(ValueError):
                release.publish(folder, notes)
            github.assert_not_called()

    def test_staging_validates_existing_notary_tickets_before_publication_is_required(self):
        for archive_notarized, image_notarized in ((False, False), (True, False), (True, True)):
            with self.subTest(archive=archive_notarized, image=image_notarized), tempfile.TemporaryDirectory() as temporary:
                folder = Path(temporary)
                stem = "OpenRadView-1.0.0-macos-arm64-standard"
                build = {"profile": "release", "source_dirty": False, "signing": "Developer ID",
                         "features": ["mcp", "updates"], "edition": "standard", "version": "1.0.0",
                         "source_revision": "a" * 40}
                source_name = "OpenRadView-1.0.0-source-" + "a" * 12 + ".tar.gz"
                for name in (stem + ".zip", stem + ".dmg", source_name):
                    (folder / name).write_bytes(b"synthetic sealed payload")
                files = {"Contents/MacOS/OpenRadView": "a" * 64}
                if archive_notarized:
                    files["Contents/CodeResources"] = "b" * 64
                archive = {"build": build, "notarized": archive_notarized, "archive_verified": True,
                           "archive": stem + ".zip", "archive_sha256": common.sha256(folder / (stem + ".zip")),
                           "source_archive": source_name, "source_archive_sha256": common.sha256(folder / source_name),
                           "bundle_files": files}
                image = {"build": build, "notarized": image_notarized, "app_notarized": archive_notarized,
                         "development": False, "signing": "Developer ID", "verified": {"app_files": files},
                         "dmg": stem + ".dmg", "dmg_sha256": common.sha256(folder / (stem + ".dmg"))}
                common.write_json(folder / (stem + ".json"), archive)
                common.write_json(folder / (stem + ".dmg.json"), image)
                for name, digest in ((stem + ".zip", archive["archive_sha256"]), (stem + ".dmg", image["dmg_sha256"])):
                    sidecar = name[:-4] + ".sha256" if name.endswith(".zip") else name + ".sha256"
                    (folder / sidecar).write_text(digest + "  " + name + "\n")

                def command(arguments):
                    if arguments[:2] == ["git", "archive"]:
                        output = next(arg.removeprefix("--output=") for arg in arguments if arg.startswith("--output="))
                        Path(output).write_bytes((folder / source_name).read_bytes())

                with patch.object(release, "DIST", folder), patch.object(release, "run", side_effect=command), \
                        patch.object(release, "verify", return_value=build) as verify_app, \
                        patch.object(release, "inventory", return_value=files), patch.object(dmg, "tooling"), \
                        patch.object(dmg, "audit") as audit_image:
                    result, _, notarized = release.verify_edition(folder, stem, require_notarized=False)
                    self.assertEqual(result, build)
                    self.assertEqual(notarized, archive_notarized and image_notarized)
                    self.assertEqual(verify_app.call_args.kwargs, {"distribution": True, "notarized": archive_notarized})
                    self.assertEqual(audit_image.call_args.kwargs["notarized"], image_notarized)
                    self.assertEqual(audit_image.call_args.kwargs["app_notarized"], archive_notarized)
                    if not notarized:
                        verify_app.reset_mock()
                        with self.assertRaisesRegex(ValueError, "Publication requires notarized"):
                            release.verify_edition(folder, stem, require_notarized=True)
                        verify_app.assert_not_called()

    def test_portable_payload_is_exact_and_native_acceptance_blocks_before_github(self):
        with tempfile.TemporaryDirectory() as temporary:
            folder = Path(temporary)
            stem = "OpenRadView-1.2.3-macos-arm64-standard"
            target = "x86_64-unknown-linux-gnu"
            archive, _ = update_protocol.names("1.2.3", target)
            names = [stem + suffix for suffix in (".zip", ".json", ".sha256", ".dmg", ".dmg.json", ".dmg.sha256")]
            names += ["OpenRadView-1.2.3-source-" + "a" * 12 + ".tar.gz", "update-aarch64-apple-darwin.json",
                      archive, archive + ".sha256", "update-" + target + ".json"]
            for name in names:
                (folder / name).write_bytes(b"original synthetic release fixture")
            record = {"schema_version": 1, "repository": release.REPOSITORY, "tag": "v1.2.3",
                      "source_revision": "a" * 40, "editions": [stem], "update_protocol": "ed25519-manifest-v1",
                      "portable": [{"target": target, "acceptance": False}],
                      "assets": [{"name": n, "size": (folder / n).stat().st_size, "sha256": common.sha256(folder / n)} for n in names]}
            common.write_json(folder / "release.json", record)
            self.assertEqual(release.validate_payload(folder), record)
            with patch.object(release, "verify_edition", return_value=({"source_revision": "a" * 40, "version": "1.2.3"}, [], True)), \
                    patch.object(release.release_checks, "verify", side_effect=ValueError("Native acceptance is required")) as native, \
                    patch.object(release, "github") as github:
                with self.assertRaises(ValueError):
                    release.publish(folder, None)
                native.assert_called_once_with(folder, target, "1.2.3", "a" * 40, require_acceptance=True)
                github.assert_not_called()
            for portable in ([{"target": target, "acceptance": True}],
                             [{"target": target, "acceptance": False}] * 2,
                             [{"target": "unknown", "acceptance": False}]):
                bad = copy.deepcopy(record)
                bad["portable"] = portable
                common.write_json(folder / "release.json", bad)
                with self.assertRaises(ValueError):
                    release.validate_payload(folder)

    def test_installer_finalization_requires_matching_accepted_log(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            image = root / "synthetic.dmg"
            image.write_bytes(b"synthetic image")
            record = {"dmg_sha256": "a" * 64, "notarized": False}
            log = root / "notary.json"
            for result in ({"status": "Accepted", "sha256": "b" * 64}, {"status": "In Progress", "sha256": "a" * 64}, {"status": "Accepted", "sha256": "a" * 64, "issues": ["warning"]}):
                common.write_json(log, result)
                with self.assertRaises(ValueError):
                    dmg.record_stapled(image, record, {}, log)
                self.assertFalse(image.with_suffix(".dmg.json").exists())
            common.write_json(log, {"status": "Accepted", "sha256": "a" * 64, "issues": None})
            dmg.record_stapled(image, record, {"mount_verified": True}, log)
            final = json.loads(image.with_suffix(".dmg.json").read_text())
            self.assertTrue(final["notarized"])
            self.assertEqual(final["dmg_sha256"], common.sha256(image))
            with self.assertRaises(ValueError):
                dmg.record_stapled(image, final, {}, log)

    def test_qa_cannot_be_shipped_or_combined_with_sharing(self):
        for option in (["--archive"], ["--mcp"], ["--sign-identity", "synthetic-identity"]):
            with self.subTest(option=option), patch.object(sys, "argv", ["bundle.py", "--qa", *option]), \
                    patch.object(bundle.platform, "system", return_value="Darwin"), \
                    patch.object(bundle, "run") as command:
                with self.assertRaises(SystemExit) as rejected:
                    bundle.main()
                self.assertEqual(rejected.exception.code, 2)
                command.assert_not_called()

    def test_zip_is_stable_preserves_executability_and_refuses_symlinks(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            app = root / "OpenRadView.app"
            binary = app / "Contents/MacOS/OpenRadView"
            binary.parent.mkdir(parents=True)
            binary.write_bytes(b"synthetic program")
            binary.chmod(0o755)
            resource = app / "Contents/Resources/demo.txt"
            resource.parent.mkdir()
            resource.write_text("synthetic resource")
            a, b = root / "a.zip", root / "b.zip"
            common.deterministic_zip(app, a, 1792900000)
            resource.touch()
            common.deterministic_zip(app, b, 1792900000)
            self.assertEqual(common.sha256(a), common.sha256(b))
            with zipfile.ZipFile(a) as archive:
                self.assertEqual(len({i.date_time for i in archive.infolist()}), 1)
                self.assertTrue(archive.getinfo("OpenRadView.app/Contents/MacOS/OpenRadView").external_attr >> 16 & 0o111)
            (app / "outside").symlink_to(root)
            with self.assertRaises(ValueError):
                common.deterministic_zip(app, b, 1792900000)
            self.assertFalse(b.with_suffix(".zip.tmp").exists())

    def test_missing_or_changed_exact_version_notice_is_rejected_offline(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            licenses = root / "assets/licenses"
            licenses.mkdir(parents=True)
            package = root / "published-package"
            package.mkdir()
            body = licenses / "notice.txt"
            body.write_text("Original copyright/license notice")
            index = {"packages": {"fixture@1.0.0": {"license": "MIT", "notices": [
                {"file": body.name, "sha256": common.sha256(body), "source": "synthetic fixture"}]}}}
            (licenses / "index.json").write_text(json.dumps(index))
            demos = root / "assets/demos"
            demos.mkdir()
            for name in ("CREDITS.txt", "CC-BY-4.0.txt", "CC-BY-3.0.txt"):
                (demos / name).write_text("Public dataset notice fixture: " + name)
            metadata = {"packages": [{"source": "registry", "name": "fixture", "version": "1.0.0", "license": "MIT", "manifest_path": str(package / "Cargo.toml")}]}
            with patch.object(notices, "ROOT", root):
                with patch.object(notices, "run", side_effect=[json.dumps(metadata), "fixture v1.0.0"]):
                    text, count, _ = notices.collect_notices("fixture-target", False)
                    self.assertIn(body.read_text(), text)
                    self.assertIn("Public dataset notice fixture: CREDITS.txt", text)
                    self.assertEqual(count, 1)
                # A native wrapper's root notice does not replace the bundled codec's notice.
                (package / "LICENSE").write_text("Wrapper MIT notice")
                index["packages"]["fixture@1.0.0"]["supplemental"] = True
                (licenses / "index.json").write_text(json.dumps(index))
                with patch.object(notices, "run", side_effect=[json.dumps(metadata), "fixture v1.0.0"]):
                    text, _, _ = notices.collect_notices("fixture-target", False)
                    self.assertIn("Wrapper MIT notice", text)
                    self.assertIn(body.read_text(), text)
                body.write_text("tampered")
                with patch.object(notices, "run", side_effect=[json.dumps(metadata), "fixture v1.0.0"]):
                    with self.assertRaises(ValueError):
                        notices.collect_notices("fixture-target", False)
                metadata["packages"][0]["version"] = "1.0.1"
                with patch.object(notices, "run", side_effect=[json.dumps(metadata), "fixture v1.0.1"]):
                    with self.assertRaises(ValueError):
                        notices.collect_notices("fixture-target", False)

    def test_unknown_notary_response_keeps_record_and_blocks_duplicate_submission(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            app = root / "OpenRadView.app"
            app.mkdir()
            (app / "synthetic").write_text("non-sensitive fixture")
            build = {"source_epoch": 1792900000, "version": "fixture"}
            argv = ["notarize.py", str(app), "--submit", "--profile", "synthetic-profile"]
            failed = subprocess.CompletedProcess([], 1, stdout="", stderr="not logged")
            with patch.object(sys, "argv", argv), patch.object(notarize, "DIST", root), \
                    patch.object(notarize, "verify", return_value=build), \
                    patch.object(notarize.subprocess, "run", return_value=failed) as service:
                with self.assertRaises(ValueError):
                    notarize.main()
                record = json.loads((root / "notarization-submission.json").read_text())
                self.assertEqual(record["phase"], "submitting")
                self.assertIsNone(record["submission"])
                with self.assertRaises(ValueError):
                    notarize.main()
                self.assertEqual(service.call_count, 1, "Unknown state must not trigger another upload")

    def test_accepted_status_with_wrong_archive_log_never_staples(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            app = root / "OpenRadView.app"
            app.mkdir()
            (app / "synthetic").write_text("fixture")
            build = {"source_epoch": 1792900000, "version": "fixture"}
            accepted = subprocess.CompletedProcess([], 0, stdout=json.dumps({
                "id": "8c7cb503-4593-4edb-ad67-5b4a2afe89b6", "status": "Accepted"}))
            wrong_log = subprocess.CompletedProcess([], 0, stdout=json.dumps({
                "status": "Accepted", "sha256": "0" * 64, "issues": None}))
            argv = ["notarize.py", str(app), "--submit", "--profile", "synthetic-profile"]
            with patch.object(sys, "argv", argv), patch.object(notarize, "DIST", root), \
                    patch.object(notarize, "verify", return_value=build), \
                    patch.object(notarize.subprocess, "run", side_effect=[accepted, wrong_log]), \
                    patch.object(notarize, "run") as finalization:
                with self.assertRaisesRegex(ValueError, "exact uploaded archive"):
                    notarize.main()
                finalization.assert_not_called()
                self.assertTrue((root / "notarization-log.json").is_file())


if __name__ == "__main__":
    unittest.main()
