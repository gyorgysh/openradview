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

Use `cargo run --locked --release -p orv-core --example benchmark_labels --
--physical` for schema 3 physical stroke timings on the same generated
512×512×128 source. It checks every cell against an independent capsule/plane
calculation, then alternates add/erase for 120 samples after four warmups while
retaining 16 history revisions. Worker proposal/apply/history and the prepared
Apply operation are measured separately. Sub-microsecond Apply samples include
timer overhead and quantization; use the complete worker timing for stroke cost.
Source construction, full-grid
verification, GPU work and native input/display are outside these timings.
Label admission includes live COW revisions and proposals; it is not process RSS.

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

For generated CPU cell-boundary geometry and edit timings, run
`cargo run --locked --release -p orv-core --example surface_latency`. The workload
uses a 512×512×128 grid with empty, filled and one-cell-plane masks. Fresh
extraction counts and emits compact quad descriptors; one/four-tile brush edits
rebuild changed and face-adjacent chunks. Three fresh-build samples and 30 edit
samples are reported separately. Untimed checks verify every face against source
membership and each final edited output against a fresh extraction. Comparison
staging is admitted separately. Source frames share one constant plane; storage
admission is not RSS. These are worker CPU timings, without smooth interpolation,
expanded vertex/index buffers, GPU/native input or presentation. Unchanged-owner
reuse timings are functional checks near clock resolution.

For immutable GPU quad-buffer uploads, run
`cargo run --locked --release -p orv-render --example surface_upload_latency`.
It uses the same generated grid/cases, reports 12 fresh uploads and 120 complete
checkpoint/brush/CPU-geometry/GPU-upload updates, and records changed/reused
chunks, packed bytes and admission separately. Fresh upload excludes extraction;
edit timings include it. Untimed readback checks every initial/final descriptor
and an extra changed state against current CPU geometry. This example excludes
3D rasterization and native presentation. Admission is not driver memory/RSS.

For complete known-interface projections, run
`cargo run --locked --release -p orv-render --example surface_projection_latency`.
It uses the same generated grid with a fixed oblique camera, requesting exactly
512 or 2048 pixels per side. Timings include fresh output allocation, analytic
ray-plane fragments, every fenced draw batch, output drop and retirement fence.
Source/geometry preparation and upload, source optics, readback and native
presentation are excluded. The 512 cases have 30 samples; 2048 has only four.
Untimed readback checks every owner/side/grid flag, finite depth and normal, plus
256 independent f64 box rays per case. Output is an open known-interface surface,
not the existing closed-mask image. Inspect the per-case scope/sample counts.

Schema 2 adds cached ordered source composition, individually fenced 16-row
sequences and complete projection plus composition with the previous completed
front retained during replacement. Each output checks GPU arithmetic before
publication. Untimed readback checks every composed pixel for finite grayscale
and alpha, plus 256 independent f64 optical rays per path/case. Composition uses
a homogeneous fully valid source with explicit constant optics and two-sided
source/surface headlight. It does not measure missing-stencil performance, native
latency or RSS. At 512, cached composition has 30 samples and row/combined paths
have 12; each 2048 path has only four. Inspect the JSON scope before comparison.

Schema 3 measures a separate one-pixel local probe of that completed front,
including per-call allocation, the shared optical calculation, 56-byte readback,
GPU status/completed-color checks, independent f64 membership traversal and
result drop/retirement. The original viewport coordinates are preserved. Pipeline
creation is excluded; the fixed centre pixel has 120 samples at 512 and 30 at
2048. Untimed checks compare its interface and foreground transmission with the
independent box/optical reference. This measures a worker operation, not native
cursor or edit latency, and creates no study-sharing or editing interface.

Use `cargo run --locked --release -p orv-render --example
surface_projection_latency -- --accelerated` for schema 4. It uses the same unique
512×512×128 sparse fixture as the scalar material benchmark, an unlit source and
two-sided surface headlight. Cached checked composition and local probes compare
dense and accelerated paths in rotating interleaved order. Complete images include
fresh output allocation, uniforms, status readback, drop and retirement fences.
The 512 composition cases have 12 samples after three warmups; 2048 has four after
one. The complete 16-row sequences fence every batch. Probe cases have 60/20
samples at 512/2048 after three warmups, using the original centre pixel.

Every composed pixel must match, including tiled and combined projection-plus-
composition paths. Independent f64 optical rays and box/face membership check
radiance and probe attenuation. Completed images and probe results retain their
exact classification owner. Source/geometry/classification/pipeline preparation,
verification readback and native input/display are excluded from timings. No
missing-stencil throughput, native latency or source-residency claim follows.

For generated source-page upload/reuse timings, run
`cargo run --locked --release -p orv-render --example brick_residency_latency`.
It retains unique 512×512×128 scalar planes, with and without missing samples,
and uploads four requested 64/128-cell pages without a dense GPU texture. Fresh
four-page upload has three samples after one warmup; unchanged reuse and
one-of-four replacement have 40/12 samples after three warmups for 64/128 cells.
Replacement keeps the old generation until the new one completes and verifies
that the other three page Arcs are shared. Timings include preparation, old
resource drop and retirement fences. Untimed readback checks every retained
scalar bit and validity byte, halos included, before and after replacement.
Source/pipeline construction, verification, image rendering and native display
are excluded. Recorded replacement admission is sampled after preparation and
excludes transient staging peaks; it is neither process RSS nor measured VRAM.
This example validates GPU source inputs, without a bricked ray marcher.

For complete paged optical images, run
`cargo run --locked --release -p orv-render --example paged_material_latency`.
It uses unique 512×512×128 planes with a fixed oblique view, a nonmonotonic luminance curve,
sparse unlit/headlight, missing headlight and dense headlight cases at 512, plus
sparse headlight at 2048. Eight-slot and full-layout caches are measured separately.
A cold first image has one sample. Warm constrained streaming has three samples
after one warmup; fully cached 512 rendering has twelve after three, and 2048
has two after three. `--quick` is a development smoke run with one source/lighting
case and two samples; it does not replace the full workload.

Paged timings include new immutable output and row-state allocations, source
uploads, cache replacement, every status fence, old-front drop and retirement.
Fully resident images use one dispatch; constrained residency uses sixteen-row
waves with original sample positions and no quality reduction. The dense baseline
reuses its source, pipeline and output and times dispatch/fence only, so these are
explicitly different scopes. Initial/final untimed readback requires every float16
pixel to match the dense image. Source generation, dense upload, pipeline/cache
construction, verification and native input/presentation are excluded. Admission
is sampled after complete images and excludes transient peaks, driver allocation
and RSS. Inspect per-image uploaded bytes and wave counts: small caches may still
stream heavily. These are worker/backend timings and do not establish native FPS.

`--wide` runs a separate 4096×512×256 logical source with a 128-slot atlas and
128×128 output. All 256 frames share one constant 8 MiB CPU plane; source
admission still counts the full 2 GiB grid. It is a one-sample functional and
physical-ray check, not a unique-data throughput or RSS benchmark. The JSON states
whether the actual device refuses that width on the dense path. The timing excludes verification and final image/source teardown. Every output
pixel is checked against independent f64 physical attenuation, and CPU/GPU
reservations must return to their starting values.

Use an external process-memory tool for RSS/footprint. GPU-fenced command timings
exclude native input, display/vsync and file decoding; native interaction and
quiet-window acceptance remain separate checks. This example opens no study and
records no user input or source paths.

Measure scalar range preparation with
`cargo run --locked --release -p orv-core --example volume_ranges`. It creates
unique 512×512×128 scalar planes for a sparse ellipsoid and one-vertex plate,
missing-data and dense-field cases, comparing 32/64/128-cell tile sides. Seven
samples follow one warmup. Timings include the worker scan, summary admission,
allocation and drop; source construction, GPU upload/rendering and native display
are excluded. Independent checks retain global extrema and complete cell
coverage, and verify reservation release. Counts of zero-extinction vertex
intervals are preparation metrics; these do not prove rounded GPU interpolation
bounds or measure an implemented skip path.

For the opt-in GPU path, run
`cargo run --locked --release -p orv-render --example material_ranges_latency`.
It uses distinct 512×512×128 planes with sparse geometric features, missing data
and a dense field, comparing the legacy renderer, dense reference and
certified tile skipping. Full 512 frames have 20 samples after three warmups;
2048 frames have four samples after one warmup. The three modes run in rotating
interleaved order. Timings include immutable uniforms,
encoding and a complete GPU fence, with resident outputs. Preparation, verification
readback and native input/display are excluded. The 512 cases also time complete
16-row sequences with a fence per batch.

Every output pixel must match the dense reference, and tiled output must
retain that sample phase. The example records differences from the legacy image,
checks 256 independent f64 optical rays for each unlit path and reports admission
and final reservation release. Headlight equivalence is checked against the dense
GPU reference; the independent optical rays use unlit material. Inspect the
workload and sample counts before comparing timings. Dense and missing sources
may have no transparent tiles; no native speedup or residency claim follows.

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
