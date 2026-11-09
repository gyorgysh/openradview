# GitHub release process

An annotated `vX.Y.Z` tag runs the same native release checks as a manual
preflight: macOS Apple Silicon, Windows x64 and Linux x64, in that order.
Each host checks formatting, all-target Clippy and nextest in default, shipping
(`mcp,updates`) and all-feature configurations, then builds the workspace and
optimized shipping binaries. Test reports must contain actual cases, both GPU
binaries in the full suite, and no failures, retries or skips. Windows/Linux
also build and exercise their archives, installers and updater.
After every native job passes, the `release` environment waits for the owner's approval.
The protected macOS job builds, signs and notarizes the Apple Silicon app and DMG,
signs all three update manifests, verifies the complete payload and creates a
normal immutable GitHub release. Website deployment is separate.

Manual test builds store their outputs in Actions artifacts; they create no
GitHub releases. No ordinary branch or PR build runs automatically. Native jobs
are serialized, use pinned tools and reusable caches, and retain test packages
for one day and test reports for seven days. Only successful jobs on main or
release tags save caches. Keys separate runner image, architecture, toolchain,
build environment and dependency inputs. The protected macOS job uses the same
runner and cache configuration as its preflight, restoring the completed debug
and optimized builds. A missing cache causes a rebuild; it cannot bypass checks.

For a manual preflight (no signing, notarization, tag or release creation):

```sh
gh workflow run ci.yml --ref main -f platform=all
```

Select `macos`, `windows`, `linux` or `portable` for a narrower investigation.
`portable.yml` is a compatibility entry point into the same checks, rather than
a separate copy of the test/build steps. Release tags always select all three hosts.

## Approval and credentials

The public repository's `release` environment allows only `v*` tags and requires
review by `gyorgysh`. Approve the exact source and native package evidence shown
in that run. Signing keys are available only after this gate. The publisher also
checks the run's actual environment-review history through GitHub's API.

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
The Rust verifier then checks each envelope against its final platform archive. The workflow
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

Before a release, `--arm` checks the environment secret names, rejects signing
credentials at repository scope, verifies the reviewer/tag policy and checks
live release immutability. It does not retrieve secret values. The job token
cannot read that administrator-only immutability setting, so CI requires owner
arming plus the actual review for its exact source. It verifies GitHub's
`immutable: true` response after publication. No administrator token is stored
in CI. Local explicit publication checks the live setting before uploading.

## Build and publish

Release notes come from the matching `CHANGELOG.md` section. When preparing a
release, replace the `Unreleased` heading with the version and date (for example,
`## 1.4.0 - YYYY-MM-DD`) so the release tools can select that section. Review the
notes against the release contents before creating the tag. The annotated tag
must match Cargo.toml, clean source and a commit reachable from main. Existing
releases and assets are never overwritten by the workflow. A failed build does
not publish a release or make its tag immutable. The owner may explicitly
replace an unpublished candidate tag after checking that no release exists,
no signing/notary operation is pending and no run still uses it. Use an exact
object lease for that exceptional replacement; never move a published tag.

Native Windows/Linux checks run default, shipping and full test suites, including actual
GPU rendering and the running-executable update swap/rollback tests. They build
installers and perform real install, payload/version checks and uninstall. Their
artifact-bound CI reports state the limits: these checks do not establish
interactive desktop/accessibility acceptance or an upgrade from an older
published version. Windows/Linux packages remain previews pending those checks.
A full manual [acceptance record](../packaging/native-acceptance-template.json)
can document broader testing without treating compilation as acceptance.

After approval, macOS verifies signatures, Apple tickets/Gatekeeper, installer
layout, exact source and notices. Each signed update manifest binds its own
platform archive; the DMG, Windows Setup EXE and Debian package are manual
installers. A Debian installation uses its package manager for future installs.
All assets must be present and hash-verified before normal publication.

Before submitting the DMG for notarization, the release job exercises the signed
macOS archive and manifest through the updater in a temporary folder. It checks
staging, signing identity, tampering rejection, health probes, swap and backup
cleanup. This uses the candidate for both sides of the check; it does not prove
an older installed app's updater or a graphical restart.
The native jobs additionally exercise the current updater against published
1.0.0 and 1.1.0 signed archives and the live feed on Windows/Linux. They do not
run an older version's updater interface or graphical restart.

A timeout does not mean Apple received nothing. Inspect the preserved submission
ID/log; do not rerun the workflow blindly. Attempts after the first are refused.
Small nonsecret notary records are retained for seven days and completed files
for one day after failure, allowing recovery without duplicate submissions.
[macOS packaging commands](release-macos.md)

For an explicitly reviewed local candidate with all platform evidence:

```sh
python scripts/macos/release.py --publish CANDIDATE --notes-file REVIEWED_NOTES
```

It creates a normal release once, checks exact remote asset digests and requires
server immutability. If an upload or response is uncertain, inspect GitHub before
any further action. Published corrections require a new version. Signing and
notarization establish distribution integrity, not clinical validation.
