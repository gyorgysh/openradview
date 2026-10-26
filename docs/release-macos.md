# macOS packaging

Normal bundles include the MCP companion and shared signed-manifest updater.
Build from the pinned Rust toolchain with Xcode and Python 3.11+ installed:

```sh
sh scripts/bundle-macos.sh
python3 scripts/macos/verify.py dist/OpenRadView.app
python3 -m unittest discover -s scripts/macos -p 'test_*.py'
```

The default bundle is optimized and ad-hoc signed for local development. It is not
a notarized distribution. `--debug` builds a development profile; `--qa` is a
separate timing harness and cannot be signed or archived as a release.

## Sign and notarize

Commit and validate clean source before Developer ID signing. Use the existing
signing identity and a separately configured notarytool Keychain profile:

```sh
sh scripts/bundle-macos.sh --sign-identity "$ORV_SIGN_IDENTITY" --archive
python3 scripts/macos/verify.py dist/OpenRadView.app --distribution
# Preflight only:
python3 scripts/macos/notarize.py dist/OpenRadView.app
# Explicit Apple submission:
python3 scripts/macos/notarize.py dist/OpenRadView.app --submit --profile "$ORV_NOTARY_PROFILE"
```

Nested code is signed before the app, with hardened runtime, secure timestamps
and no entitlement exceptions. Verification checks native architecture, deployment
floor, team, resources, notices, provenance and updater layout. Timestamping and
notarization contact Apple. Do not put credentials in source or logs.

The notarization helper records state before upload, requires an Accepted log
matching the submitted ZIP hash, staples and checks Gatekeeper, then regenerates
the final archive/manifest. Resume an existing recorded ID rather than resubmitting
after a timeout:

```sh
python3 scripts/macos/notarize.py dist/OpenRadView.app --resume SUBMISSION_UUID --profile "$ORV_NOTARY_PROFILE"
```

## Build the installer

Install the hash-pinned DMG tooling and use the already notarized app:

```sh
python3 -m venv target/dmgbuild-venv
target/dmgbuild-venv/bin/pip install --require-hashes -r packaging/macos/installer/requirements.txt
target/dmgbuild-venv/bin/python scripts/macos/dmg.py --sign-identity "$ORV_SIGN_IDENTITY" --app-notarized
```

The helper copies the sealed app, adds the installer artwork and Applications
shortcut, signs the DMG, mounts it read-only and audits its contents/Finder layout.
Existing versioned outputs are not overwritten. A local preview uses `--development`;
it remains ad-hoc and cannot be published as notarized.

The DMG needs its own explicit submission, Accepted log and staple:

```sh
xcrun notarytool submit "$ORV_DMG" --keychain-profile "$ORV_NOTARY_PROFILE" --wait --output-format json
xcrun notarytool log SUBMISSION_UUID --keychain-profile "$ORV_NOTARY_PROFILE" > dist/dmg-notary-log.json
xcrun stapler staple "$ORV_DMG"
target/dmgbuild-venv/bin/python scripts/macos/dmg.py --audit "$ORV_DMG" --notarized --record-stapled --notary-log dist/dmg-notary-log.json
```

Inspect the authoritative status before proceeding; a timeout is not permission
to upload again. Finalization requires the accepted pre-staple image hash and
actual ticket/Gatekeeper checks.

## Freeze a release candidate

The final notarized app ZIP is the updater payload; the DMG is the manual installer.
Stable ZIP ordering, modes and timestamps preserve exact source identity, but
signing/tickets can vary between runs. Fresh extraction is audited against the
sealed bundle. Bundles and update archives contain regular files only; no framework
links or external resource forks are required.

Prepare an unsigned manifest after the ZIP is final:

```sh
python3 scripts/update_protocol.py "$ORV_FINAL_ZIP" --version "$ORV_VERSION" --target aarch64-apple-darwin
```

The approved CI workflow signs it with the existing environment key. For local
signing, pass `UPDATER_ED25519_PRIVATE_KEY` only to `scripts/ci/sign-manifest.mjs`,
with the raw JSON and a new signature output path. Do not print or save the key.
Create and verify the envelope from the public signature file, then freeze:

```sh
python3 scripts/update_protocol.py "$ORV_FINAL_ZIP" --version "$ORV_VERSION" --target aarch64-apple-darwin --signature-file "$ORV_SIGNATURE_FILE"
target/dmgbuild-venv/bin/python scripts/macos/release.py
```

Ignored `dist/releases/` candidates contain the exact ZIP/DMG, source, notices,
checksums and target-bound signed feed. Their folder names bind source and payload
hashes. They are never modified; changed bytes require a new candidate/version.
Portable inputs need the same source/version and artifact-bound native acceptance.
[Portable release inputs](platforms.md#frozen-release-inputs)

The separate [GitHub workflow](github-release.md) performs signing/notarization
only after its approval trigger and uploads a draft. Final publication requires
explicit review, a matching annotated source tag, exact remote digests and release
immutability. While the repository is private, anonymous feed/download requests
cannot succeed; no GitHub token is embedded in the app.
