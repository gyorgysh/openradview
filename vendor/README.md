# Reviewed third-party source

These Cargo path patches preserve the selected Rust wrapper APIs while updating
native decoders and the complete-local-file decoding policy. See
[architecture](../docs/architecture.md) and
[validation](../docs/validation.md).

`sources.json` records original crate archive checksums, native revisions, patch
order, local adaptations and the SHA-256 of every included file. The patches in
`openjpeg-sys/patches/` apply in manifest order to the recorded OpenJPEG 2.5.4
source. Native tools, tests, sample data and unused JPIP code are omitted. No additional native
source download or system codec lookup occurs during the ordinary pinned build.

Run `python3 scripts/macos/vendor_sources.py` before building after any source
change. A hash mismatch requires source/license review; do not refresh the
inventory just to make the check pass. Packaging includes these packages in its
notices and build provenance instead of treating every local path as our own code.

Original MIT, BSD-2-Clause, BSD-3-Clause and the jpeg2k wrapper's MIT/Apache-2.0
license declarations/notices are retained. They are not relicensed by the app's
GPL header policy. Source formatting is preserved; Rust workspace formatting and
linting apply to the application's own crates. Modified native code remains under
its original compatible source license.
