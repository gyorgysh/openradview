#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Preflight by default. Upload or resume only with an explicit action/profile."""
import argparse
import json
from pathlib import Path
import subprocess
import sys
import uuid

from common import DIST, deterministic_zip, export_release, inventory, run, sha256, write_json
from verify import verify


def signed_inventory(app):
    files = inventory(app)
    # This extra ticket file is allowed only after stapler validation, never
    # as an arbitrary extra resource in a normal build (see stapler(1)).
    files.pop("Contents/CodeResources", None)
    return files


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("app", type=Path)
    action = parser.add_mutually_exclusive_group()
    action.add_argument("--submit", action="store_true", help="explicitly upload this signed app to Apple")
    action.add_argument("--resume", type=uuid.UUID, metavar="SUBMISSION_ID", help="check an existing recorded submission, without uploading again")
    parser.add_argument("--profile", help="existing notarytool Keychain profile; never pass account passwords here")
    args = parser.parse_args()
    args.app = args.app.resolve(strict=True)
    if (args.submit or args.resume) and not args.profile:
        parser.error("an existing --profile is required for an explicit upload/status query")
    has_ticket = (args.app / "Contents/CodeResources").exists()
    build = verify(args.app, distribution=True, notarized=bool(args.resume) and has_ticket)
    if not args.submit and not args.resume:
        print("Notarization preflight passed. No Apple upload or credential access performed.")
        return
    DIST.mkdir(exist_ok=True)
    record_path = DIST / "notarization-submission.json"
    if args.resume:
        record = json.loads(record_path.read_text())
        prior_id = (record.get("submission") or {}).get("id")
        if (prior_id is not None and prior_id != str(args.resume)) or record["build"] != build or record["bundle_files"] != signed_inventory(args.app):
            raise ValueError("Resume requires the exact recorded submission and unchanged bundle")
        command = ["xcrun", "notarytool", "info", str(args.resume)]
    else:
        if record_path.exists():
            previous = json.loads(record_path.read_text())
            if previous["build"] == build and previous["bundle_files"] == signed_inventory(args.app):
                raise ValueError("This bundle already has a submission record; use --resume with its ID before considering another upload")
        archive = DIST / ".orv-notarization-upload.zip"
        deterministic_zip(args.app.resolve(), archive, build["source_epoch"])
        record = {"build": build, "bundle_files": signed_inventory(args.app), "upload_sha256": sha256(archive)}
        command = ["xcrun", "notarytool", "submit", str(archive), "--wait", "--timeout", "15m"]
        # A killed process or missing response must not make the upload look as
        # though it never happened. Preserve the unknown state before starting.
        record["submission"] = None
        record["phase"] = "submitting"
        write_json(record_path, record)
    result = subprocess.run([*command, "--keychain-profile", args.profile, "--output-format", "json"],
                            capture_output=True, text=True)
    # Preserve the service's authoritative ID/status even on a processing timeout.
    # Do not retry an upload merely because the synchronous wait ended.
    try:
        record["submission"] = json.loads(result.stdout)
    except json.JSONDecodeError:
        raise ValueError("No structured notary result; no automatic retry performed. Check the existing submission/profile with notarytool.") from None
    if not isinstance(record["submission"], dict):
        raise ValueError("Invalid structured notary result; prior unknown submission state is preserved")
    record["phase"] = "response_received"
    write_json(record_path, record)
    if result.returncode or record["submission"].get("status") != "Accepted":
        raise ValueError("Not accepted yet. Inspect dist/notarization-submission.json and resume that ID; no ticket was stapled")
    identifier = str(uuid.UUID(record["submission"]["id"]))
    log_result = subprocess.run(["xcrun", "notarytool", "log", identifier, "--keychain-profile", args.profile],
                                check=True, capture_output=True, text=True)
    log = json.loads(log_result.stdout)
    if not isinstance(log, dict):
        raise ValueError("Invalid notary log; no ticket was stapled")
    write_json(DIST / "notarization-log.json", log)
    if log.get("status") != "Accepted" or log.get("sha256", "").lower() != record["upload_sha256"]:
        raise ValueError("Notary log does not match the exact uploaded archive; no ticket was stapled")
    if log.get("issues"):
        raise ValueError("Inspect the recorded notary log warnings/issues before finalizing distribution")
    run(["xcrun", "stapler", "staple", str(args.app)])
    verify(args.app, distribution=True, notarized=True)
    archive = export_release(args.app, build, notarized=True)
    # export_release also verifies the actual ZIP after a fresh extraction.
    record["phase"] = "stapled_and_archive_verified"
    write_json(record_path, record)
    print("Stapled, Gatekeeper-verified and exported " + archive.name)


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, KeyError, subprocess.CalledProcessError) as error:
        print("Notarization stopped: " + str(error), file=sys.stderr)
        sys.exit(1)
