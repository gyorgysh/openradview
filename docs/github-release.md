# GitHub release process

The release workflow builds one Apple Silicon macOS candidate, signs it, notarizes
the app and DMG, signs the shared update manifest, verifies the frozen payload and
uploads a **draft**. It does not publish, change repository visibility or deploy
the website. Windows/Linux assets require native acceptance on their own hosts.

## Approval and credentials

The `release` environment allows `v*` tags only. While the repository is private,
only an owner workflow dispatch with `SIGN AND DRAFT` can start a release runner.
Select the exact annotated version tag. Private tag pushes alone allocate no
runner. Once public, configure required-reviewer protection before enabling tag
builds. The setup helper verifies the actual policy and selects the readiness mode.

There is one runner, a 55-minute limit and no ordinary branch/PR or parallel-platform
release builds. It restores the cache shared with manual checks without saving
per-tag caches. A failed job retains only small nonsecret notary records for seven
days. Review GitHub usage before starting a run.

Store the following only in
[Settings → Environments → release](https://github.com/gyorgysh/openradview/settings/environments),
under **Environment secrets**. Do not use repository secrets, workflow inputs,
checked-in files or chat. The Apple identity must belong to the intended distributor.

| Secret | Required value |
| --- | --- |
| `DEVELOPER_ID_APP` | Exact Developer ID Application identity or certificate hash |
| `DEVELOPER_ID_P12_BASE64` | That identity and private key as base64 P12 |
| `DEVELOPER_ID_P12_PASSWORD` | The P12's nonempty password |
| `TEAM_ID` | Apple team ID, checked against the signed app |
| `NOTARY_ISSUER_ID` | App Store Connect team API issuer UUID |
| `NOTARY_KEY_ID` | API key ID |
| `NOTARY_KEY_P8_BASE64` | API private key as base64 P8 |
| `UPDATER_ED25519_PRIVATE_KEY` | Base64 Ed25519 seed or seed/public pair matching `packaging/updater.json` |

The signer checks the derived public key before signing the exact update JSON.
The Rust verifier then checks the envelope against the final ZIP. The workflow
uses a disposable runner keychain and private temporary credential files, restores
its keychain settings and removes its files during cleanup. Credentials are removed
from the environment before Cargo/packaging subprocesses run. The dedicated manifest
signer receives only its update key. Missing or mismatched credentials stop the run;
there is no unsigned fallback.

With an authenticated owner/admin `gh` session:

```sh
python3 scripts/ci/release-policy.py --configure
python3 scripts/ci/release-policy.py --arm
```

Before each release, rerun `--arm`. It checks environment secret names, rejects
release credentials at repository scope, verifies the tag/reviewer policy and
checks live release immutability. It never retrieves secret values. The job-scoped
GitHub token cannot read that admin-only setting, so the draft-only job uses the
owner's preflight confirmation. No administrator token is stored in CI. Final
publication always rechecks the live setting through the owner's admin session.

## Build and review

Release notes come from the matching version section in `CHANGELOG.md`; no duplicate per-version document is needed. The annotated tag must
match Cargo.toml, clean source and a commit reachable from main. Actions are pinned
by commit ID. Existing release versions cannot be overwritten.

Run **Approved macOS release draft** for that tag, then review the exact app, DMG,
source archive, notices, hashes, manifest and draft assets. Perform native launch,
viewing/consent checks and a production update/restart/recovery roundtrip before
publication. Signing/notarization does not establish clinical validation or
regulatory clearance. See [validation scope](validation.md).

A wait timeout does not mean Apple received nothing. Inspect the preserved
submission ID/log and resume the local notarization helper; blind workflow reruns
and duplicate uploads are refused. Keep the exact submitted source and bytes.
[macOS packaging commands](release-macos.md)

## Publish separately

Download the complete frozen candidate into a new local folder. Verify its files
against release.json, review the release notes and finish the acceptance checks.
Then use the separate explicit command:

```sh
python scripts/macos/release.py --publish CANDIDATE
```

It rechecks signatures/notarization, the annotated source tag, signed feed, exact
GitHub asset digests and live server immutability before publication. Published
tags and assets stay immutable; corrections require a new version. Public release
and website deployment remain explicit owner actions.
