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
Linux CI also checks software OpenGL rendering and an empty native window under
Xvfb for startup and quiet-window repainting. This developer-only probe opens no
study and changes no consent; it is excluded from distributed packages.

The macOS native review uses generated inputs and the included licensed public
CT, MRI and X-ray datasets. It checks loading/cancellation, navigation, view
changes, modal placement, labels/export, local agent review/denial/revoke and
update preferences. No private patient files or provider account are needed.
[Development commands](../CONTRIBUTING.md)

The release job also checks the signed macOS candidate through the updater's
staging, signature, swap and cleanup paths in a disposable folder, including
wrong-identity and modified-file refusal. It exercises the candidate on both
sides; it does not establish an older installed updater or graphical restart.
Native Windows/Linux jobs exercise the current updater code against the published
1.0.0 and 1.1.0 signed archives, the live feed and download, temporary swaps,
changed-file refusal and backup preservation. These checks run the published
executables' health probes, not their updater interfaces or graphical restart.

## Platform and release acceptance

Apple Silicon/macOS with Metal has native local evidence. Native Windows MSVC and Linux GNU CI run default/full suites, GPU rendering,
loaded-executable swaps/rollback and installer install/probe/uninstall checks.
Their artifact-bound reports preserve actual counts and source/hash identity.
They do not establish interactive file dialogs, accessibility, consumer GPU
performance or an older-version production update roundtrip. [Current platform status](platforms.md)

Before distributing a binary, validate the exact clean signed artifact on the
intended OS: launch, supported inputs and multi-GB cancellation, all view modes,
resize/input/accessibility, labels and unsaved changes, local consent/revoke,
installation, updater roundtrip/relaunch, recovery and backup cleanup. Record
source revision, artifact hashes and what was actually tested. Portable releases include artifact-bound CI evidence with its explicit scope.
The broader [manual acceptance record](../packaging/native-acceptance-template.json)
can document the interactive checks.
Never fill it by treating a cross-build or a placeholder as runtime evidence.

Signatures, notarization, hashes and exact source/notice inventories establish
specific distribution-integrity properties. They do not establish medical
accuracy, universal OS support or compliance with every jurisdiction. Production signing/notarization and exact native package checks gate publication.
Windows/Linux packages retain preview status while broader interactive and
production update acceptance is incomplete.

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
