# Contributing

OpenRadView is GPL-3.0-or-later. Sign off commits with `git commit -s` to certify
that you can contribute the work under the project's licence. Retain dependency,
font, codec and dataset notices. Do not commit personal scans, credentials or
identifying screenshots; use generated fixtures or the credited public demos.

## Build and checks

Install Rust through rustup using `rust-toolchain.toml`, CMake and your native
platform development tools. The default development viewer omits the optional
MCP and update integrations; normal desktop bundles include them.

```sh
cargo fmt --all --check
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
cargo nextest run --locked --workspace
cargo nextest run --locked --workspace --all-features
cargo build --locked --workspace
```

Use cargo-nextest 0.9.149. The full suite requires a working GPU: a skipped render
test does not validate rendering. Check native UI changes on the affected OS.
For a minimum-window native review, build the QA-only binary with
`ORV_QA_COMPACT=1 cargo build --locked -p orv-app --features qa`; its initial
window is 860×640. Omit that build variable for normal-size QA. Distribution
builds exclude QA and this override.
The manual Linux package job also runs OpenGL rendering under Mesa/Xvfb and a
QA-only native startup/idle check. This opens no study, disables input controls
and closes after four seconds. Its temporary aggregate contains timing/count
results; normal bundles exclude QA. These checks do not cover file dialogs or
consumer GPU performance.
[Platform requirements](docs/platforms.md) describe Linux and Windows tooling.

Measure generated GPU workloads explicitly:

```sh
cargo run --locked --release -p orv-render --example latency
cargo run --locked --release -p orv-render --example latency -- --padding
```

The JSON identifies MIP and Composite separately, fixed view parameters, complete
refined frames, individual tiles and complete sequences of fenced tile submissions.
It also records adapter/requested limits and shared GPU admission before/after
renderer teardown. Reservations are not measured driver memory or free VRAM.
Reslice cases compute three moving 512-side grids on that same uploaded source:
thin, 65-sample maximum and 257-sample mean. Those timings exclude display mapping,
readback and native presentation; the reslicer is a backend prototype.
Material cases use an explicit four-knot optical mapping and headlight, with
512-side interactive and 1024/2048-side refined complete compute. They retain
linear float16 output and exclude final presentation. The 2048 case has only
four measured samples; inspect the JSON sample counts before comparing timings.
For generated CPU label edit/checkpoint timings, run
`cargo run --locked --release -p orv-core --example benchmark_labels`. Its JSON
separates brush edits, full-grid fill, shared checkpoints and raw byte-copy timing;
the latter is not a timing of the former application. The generated source shares
one constant plane across slices, and label reservations exclude source storage,
allocator overhead and process RSS. It measures no native input/display latency.
Schema 2 also measures supplied mask construction, raw row copies and categorical
source-plane sampling. It compares packed binary, full fractional and two-plane
fractional storage on the same generated grid. Input/staging admission is separate
from retained mask bytes. Construction has three samples; row/plane cases have
120. Fractional values remain original bytes with a declared maximum and meaning;
the sampled overlay uses the existing half-maximum categorical view.

For complete GPU label revision uploads, run
`cargo run --locked --release -p orv-render --example label_latency`. The workload
includes CPU checkpoints/paint, target allocation, a full prior-grid GPU copy,
changed tile uploads and the upload fence. It records raw uploaded/copied bytes
and app admission separately. It also includes mask-only first-cell/depth
projection of empty, one-cell-plane and filled maps at a fixed camera, using one
complete dispatch or a complete sequence of individually fenced 16-row batches.
Projection timings include fresh output allocation and exclude source optical
integration, readback, native input and display. The 512-side cases have 120 full
and 30 tiled samples; 2048-side cases have only four of each. An untimed full
cell/ID readback checks for arithmetic errors and ineligible reported hits at both
sizes; it is not an exhaustive CPU ray comparison. Inspect the per-case scope.

Schema 3 adds physical source composition ending at one opaque manual-mask hit.
It separates cached-mask composition, cached-mask 16-row submissions and complete
mask projection plus composition. Each complete result checks a four-byte GPU
status before publication; row composition fences only after the final batch.
The generated homogeneous source has an independent optical reference. Untimed
readback checks every output pixel for finite monochrome radiance/alpha and 256
sampled rays against f64 cell traversal plus Beer-Lambert optics. Source and label
uploads, presentation and pixel readback are outside these composition timings.
The sample counts remain 120 full/30 row sequences at 512 and four each at 2048.
Schema 4 adds supplied binary/fractional masks with full or two-plane coverage.
It reports fresh selected-ID packing/upload (12 samples), exact-owner reuse
(120), packed texture sizes and bytes, projection and checked source composition.
Every packed word and source-plane address is checked in untimed readback;
reported mask hits and optical outputs use the same verification scopes above.
IDs remain u16 and source dimensions remain independent of compact plane storage.

Use an external process-memory tool for RSS/footprint. GPU-fenced command timings
exclude native input, display/vsync and file decoding; native interaction and
quiet-window acceptance remain separate checks. This example opens no study and
records no user input or source paths.

Generate fresh local input without using patient files:

```sh
cargo run -p orv-core --example generate_fixture -- test-data/synthetic
cargo run -p orv-core --example generate_formats -- test-data/synthetic-formats
cargo run -p orv-core --example generate_media -- test-data/generated-media
cargo run -p orv-core --example generate_media -- test-data/generated-labels --label-phantom
cargo run -p orv-core --example generate_media -- test-data/generated-masks --segmentation-phantom
cargo run -p orv-core --example generate_comparison -- test-data/generated-comparison
cargo run -p orv-core --example generate_acquisition -- test-data/generated-acquisition
cargo run -p orv-core --example generate_dicom_acquisition -- test-data/generated-dicom-acquisition
cargo run -p orv-core --example generate_modality -- test-data/generated-modality
cargo run -p orv-core --example generate_float -- test-data/generated-float
cargo run -p orv-core --example generate_real_world -- test-data/generated-real-world
```

The mask fixture adds three generated boxes with overlap, sparse source planes
and a large numeric ID, plus a wrong-reference file that import must refuse. It
contains no anatomy and exercises the documented input subset.
The acquisition fixtures encode time, component, higher axes and source slices
in numeric pixel values, with complex views and valid/frequency/unknown/invalid
timing headers.
The DICOM acquisition fixtures include separate dimension ordinals/values, shuffled
spatial frames, fixed/variable cine timing and explicit unscoped/invalid cases.
The modality fixtures include numeric nonlinear tables, a regular stack, declared
and unknown units, stored padding and a conflicting-transform refusal case.
The float fixtures add a regular stack, big-endian extremes, NaN/infinite/padded
samples, a fractional window and a refused rescale case. The real-world fixtures
cover integer and float linear mappings, a private unit and refused alternatives.

## Design and review

Keep loading, decoding, statistics, exports and long-running work off the UI
thread. Preserve cancellation, resource bounds, exact source/view identities and
consent checks. Core/render crates must remain independent of the UI. Use the
pinned dependency APIs and add a regression test for a substantive correctness
fix. Comments should explain a constraint or decision that the code alone does
not make clear.

[Architecture](docs/architecture.md), [format limits](docs/supported-formats.md),
[agent protocol](docs/agent-access.md) and [validation](docs/validation.md) describe
the boundaries contributors must preserve. Source and release changes do not
establish clinical validation or regulatory approval. Describe the app as a viewer
for personal education and use the intended-use wording in DISCLAIMER.md.

Development CI runs manually. Release jobs have a separate approval process;
do not add ordinary push/PR jobs, dispatch workflows or publish artifacts as a
routine development check. [Release setup](docs/github-release.md)
