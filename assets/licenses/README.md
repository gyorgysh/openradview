# Dependency notices omitted from published crate archives

`index.json` maps exact package versions to original upstream license/copyright notices. Sources use the commit recorded in the published crate's `.cargo_vcs_info.json` where available. The published DICOM 0.10.0 VCS objects are unavailable upstream, so their notices use its inspected `v0.10.0` release commit `2941c54cae434dd63a54d0a5546b3b0df42e9f6c`. CC0 table crates retain the public-domain dedication from their published source header. Text files use their SHA-256 as filenames and preserve the original text.

The packagers first use notice files included in each selected Cargo package. It uses this reviewed fallback only when those are absent, verifies its hash and matching license declaration, and fails for any unreviewed package/version. It does not fetch licenses or contact upstream repositories during packaging. Notices in this directory retain their original licenses, not the project's GPL license. Build selection uses normal dependencies for the actual host target and optional app feature, excluding development-only and other-platform packages.

Two upstream omissions are recorded explicitly: `dispatch` 0.2.0 declares MIT but supplies no standalone license/copyright file at its published revision, so its original manifest/authorship is retained together with standard SPDX MIT terms, without inventing a copyright year. The CC0 `encoding_index_tests` helper retains its original manifest declaration because it has no copyright header. These are exact-version records, not blanket approval for future releases.

The `sparkle-updater@0.1.0` package and exact upstream revision declare MIT but ship no separate license/copyright file. Its notice records the published author and this limitation, followed by the canonical MIT terms from pinned SPDX license-list-data v3.27.0; it does not invent an upstream copyright notice. The Sparkle SDK ships its complete original LICENSE separately in the app.

`openjpeg-sys@1.0.12` bundles OpenJPEG 2.5.3. Its BSD-2-Clause notice is in
`vendor/LICENSE`, not the package root, so the exact published file is curated here.
`charls-sys@2.4.5` has an MIT wrapper and a separately licensed BSD-3-Clause CharLS
implementation in `charls/LICENSE.md`. Its supplemental record is **always appended**
alongside the root MIT notice. Both native implementations are statically linked;
these permissive notices are retained independently of the app's GPL licence.

Portable target records also include AccessKit consumer/Windows at published
commit1bbcf100942bac96c2c3a4a91cb67b0b20201a24, Unix/AT-SPI/consumer at
f40dfc01a0c0e76de535969f82fb35e19513737d, clipboard-win5.4.3 at
ed27008ae5d1f10cfbb266de039f0fb35fd05a56 and SPIR-V SDK1.4.341 bindings at
8afc3d0ac8e158128cd1410bb2e4b4c26ab11bb4. Each exact source/license is hash checked;
this does not approve future versions or compiler runtime redistribution.
