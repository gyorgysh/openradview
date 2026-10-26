# Validation scope

OpenRadView is tested as software for personal image exploration. These checks
are not clinical validation, a regulatory determination, monitor calibration or
an independent security audit. A passing synthetic fixture does not prove that
a particular person's image or measurement is suitable for care.

## Automated checks

The workspace covers DICOM/NIfTI decoding and geometry, malformed metadata,
compressed/color/padding samples, labels, source identity, memory bounds,
cancellation, local transports/consent, update manifests and installation rollback.
Sparse source and ZIP64 tests exercise files above 4 GiB without retaining all
pixels. Codec fixtures are generated, with source/license notes beside them.

Opt-in GPU tests exercise actual rendering and offscreen comparisons, including
MPR, oblique, volume/MIP, RGB, missing samples and snapshot provenance. Run the
full feature suite on a supported GPU; a skipped GPU test is not a pass.
Painted UI tests check control geometry, wrapping and stable view parameters.

The macOS native review uses generated inputs and the included licensed public
CT, MRI and X-ray datasets. It checks loading/cancellation, navigation, view
changes, modal placement, labels/export, local agent review/denial/revoke and
update preferences. No private patient files or provider account are needed.
[Development commands](../CONTRIBUTING.md)

## Platform and release acceptance

Apple Silicon/macOS with Metal has native local evidence. Windows GNU and Linux
GNU app/companion cross-builds check compilation/linking only. They do not verify
native file locking, GPU drivers, file dialogs, accessibility, installation or
updater behavior. [Current platform status](platforms.md)

Before distributing a binary, validate the exact clean signed artifact on the
intended OS: launch, supported inputs and multi-GB cancellation, all view modes,
resize/input/accessibility, labels and unsaved changes, local consent/revoke,
installation, updater roundtrip/relaunch, recovery and backup cleanup. Record
source revision, artifact hashes and what was actually tested. Portable release
checks require an artifact-bound [native acceptance record](../packaging/native-acceptance-template.json).
Never fill it by treating a cross-build or a placeholder as runtime evidence.

Signatures, notarization, hashes and exact source/notice inventories establish
specific distribution-integrity properties. They do not establish medical
accuracy, universal OS support or compliance with every jurisdiction. Production
signing/notarization, native portable acceptance and updater roundtrips remain
release gates until completed against the actual artifacts.

## Dependencies and performance

Keep native codec provenance and Cargo advisory checks current before release.
The pre-release review found four upstream maintenance notices (`encoding`,
`zvariant`, `zvariant_derive`, `zbus_names`). Do not hide them with ignore flags,
force an incompatible major version or remove accessibility to make a check green.
A current audit and native source review are required for the release candidate.

Performance depends on image dimensions, compression, geometry, GPU/driver and
memory. Background indexing and bounded caches reduce resident pixel data;
interactive preview/refinement limits expensive work while navigating. The app
makes no guaranteed frame-rate or equivalence-to-clinical-viewer claim.
