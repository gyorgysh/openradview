# Architecture

OpenRadView is a Rust 2024 Cargo workspace. Dependency versions and the toolchain
are pinned. Rendering is native wgpu; egui/eframe owns the desktop shell. macOS
selects Metal; Windows selects DX12/Vulkan and Linux allows Vulkan/OpenGL. Core and
rendering code remain independent of the UI.

| Crate | Responsibility |
| --- | --- |
| `orv-core` | Input indexing/decoding, sample validity, geometry, measurements, labels and serializable scene state |
| `orv-render` | UI-independent slice, oblique and volume/MIP GPU rendering; offscreen snapshots |
| `orv-app` | Native windows, input tools, background workers, local file dialogs and consent |
| `orv-mcp` | Local discovery, approval-only broker, authenticated data transports and scoped tools |
| `orv-agents` | Frozen comparisons and bounded experimental commentary, without provider integrations |
| `orv-updates` | Bounded GitHub update traffic, signed manifests, installation staging and rollback |

## Input, memory and geometry

Metadata-first indexing avoids decoding every image at import. Requested frames
share a bounded LRU; each indexed read checks source identity. Cached-entry locking
is separate from slow I/O/decoding. At most four hot decompression readers are
retained per study cache, with matching-stream reuse and cancellable contention. ZIP/gzip/deflate
processing streams without extracting patient files to disk. Dimensions, payloads,
metadata, expanded data, frame counts and materialized volumes have explicit
bounds. Load, decode, filtering, statistics and export run on cancellable workers.
[Exact limits](supported-formats.md)

RAM-only slice-cache entries, prepared CPU volumes and GPU resources use
process-wide reservation pools with physical-memory-based ceilings. The policy
keeps headroom for the OS/transients and accounts for CPU/GPU competition on
unified-memory systems. Worker activity refreshes RAM counters; idle views do not
poll. Pressure lowers optional cache capacity and restricts new volume preparation
without invalidating live views. No swap-based admission or study-data spill is
introduced. Hardware hints are matched to the selected adapter; missing or
ambiguous capacity retains a conservative GPU ceiling. [Policy and limits](supported-formats.md)

Prepared volumes use one process-wide retained-source reservation pool. Indexed
sources reserve their conservative decoded size before materialization; weak
per-source caches share a live immutable volume without keeping closed data alive.
Resident sources check exact immutable frame identity before reuse. Original frame
order remains available for approval checks even when spatial sorting differs.
GPU views reserve source/mask data, bounded outputs, retired outputs and upload
staging against a separate shared pool, check device limits and handle texture
allocation errors. Reservations remain charged until submitted GPU work finishes.
Uploads flush in bounded batches. Neither pool is a hard OS/driver memory sandbox.

Scalar source context and grayscale choices are immutable and prehashed. Import
workers validate/intern windows and lookup tables under metadata bounds; identical
tables share allocations. Native/snapshot shaders apply one explicit mapping after
rescale/interpolation and before inversion. Conversion explicitly selects the
validated frame transform, independent of decoder metadata precedence. Source/per-frame identity includes
storage, scaling, declared units and table definitions without raw tag text.
Reformats retain typed source context and mark resampled values; mixed units or
lookup tables cannot silently become one spatial presentation.

Scalar values retain an independent validity mask when needed. DICOM padding is
identified in stored codes before rescale; statistics, hashes, labels, CPU/GPU
sampling and memory accounting share this validity domain. Color retains source
RGB code precision separately from sRGB presentation; compositing/resampling use
linear-light sRGB. No color-to-scalar modality intensity is invented.

Validated regular volumes have an affine and inverse mapping between source
indices and LPS millimetres. NIfTI sform/qform method, units and reference code
remain explicit: template/aligned coordinates are not assumed to be patient
coordinates. Unknown geometry stays viewable in 2D without inferred anatomical
labels or physical measurements.

DICOM functional groups are validated before decoding. Explicit per-frame geometry
and numeric/coded acquisition context partition nonspatial dimensions before stack
construction. Acquisition digests participate in source identity without exposing
raw tags. Missing dimension/geometry context keeps source browsing available;
conflicting macros and unsupported value mappings fail. Single-frame decode clones
only its selected functional group, avoiding a full per-frame metadata-table copy.

## Rendering and interaction

The UI paints the last completed acquired image or coherent linked-plane result
while newer work is pending. Workers coalesce obsolete requests and preserve
cancellation. Linked planes share one rendered crosshair/generation; overlays do
not publish a new point before its images are ready. Volumes retain an interactive
preview while idle tiles refine with immutable per-dispatch parameters.

GPU resources are owned per view, while immutable source frames are shared.
Window/level, zoom, pan, interpolation and validity use the same implementation
for desktop and offscreen rendering. Repaint is driven by input, results, active
playback and finite animation. Idle viewing does not require a polling loop.
Measurements map image centres to validated physical coordinates rather than
screen distance. Projections cannot masquerade as measurement planes.

Temporary marks are bounded per series/pane and keyed by source-frame ordinal
or exact reformat geometry, independently of decoded allocation addresses.
Completed marks retain coordinates/results without retaining source pixels.
One cancellable ROI calculation per pane publishes to its original mark ID.
Series previews use one idle-blocking worker, coalesce visible-row requests and
reuse the decoded source cache. At most 32 small textures remain in RAM, with
shared RAM/GPU reservations; closing the study cancels work and drops previews.

Local two-series comparison owns independent latest-frame workers and pane state,
sharing the original study/cache and process-wide admission pools. Optional slice
linking validates header geometry on workers and uses a physical image-center
point with nearest-plane lookup. Bounded navigation tables share prepared-source
RAM admission; comparison does not materialize two volumes. The active pane owns
tools, contrast and single-picture export. Click/drag/scroll or the pane buttons
activate it; passive hover does not change its tools.

Entering comparison revokes live and frozen grants, cancels review/shared work and
removes agent composition. Dual-pane content is never published through MCP.
Leaving comparison returns to its active series and requires fresh Allow to share.
The layout, marks and previews have no persistence or restore path.

Quarter-turn/flip display transforms leave acquisition geometry and source pixels
unchanged. The source-to-viewport map is shared by probes, measurements, label
meshes/brush footprints and overlays; the slice shader applies its inverse.
Transforms participate in scene serialization/hashes and exact-view consent.
Volume projections use their camera orientation and reject 2D transforms.
Scalar slabs sample bounded discrete extrema along an explicit physical plane
normal, excluding missing samples before display mapping. CPU workers publish a
small coherent preview, then refine once input is quiet. Projection parameters
and the sampling grid travel through scenes/snapshots; public image metadata does
not label them as measurement geometry. Measurements refuse projected frames.
No full-depth projection buffer, source copy or persistence is introduced.

Optional physical LPS clipping intersects the original affine source ray domain
before sampling. It changes neither source data nor opacity distance units; its
validated bounds participate in scene hashes, exact approval and snapshots.

The reviewed macOS windowing patch routes application termination through the
same cancellable close requests as window closure. This lets the application
protect unsaved labels and an update installation before exiting; it introduces
no automatic study or label save. Other platforms retain upstream windowing.

Native Linux OpenGL uses the same wgpu renderer, with downlevel device limits for
2D/MPR. Compute-capable drivers can also use volume/MIP; other drivers receive
an explanation when opening those views. `WGPU_BACKEND` selects an allowed native
backend for both the window and offscreen snapshots.

## Scenes, snapshots and agent access

`ViewState` describes display settings; `SceneState` also binds source identity,
series, plane/crosshair, reconstruction and bounded output size. Schema 1 covers
source/anatomical views; schema 2 adds typed oblique and volume parameters.
Hashes distinguish pixels, geometry, validity, color context and view settings.
Pending results never count as completed snapshots.

A snapshot pairs image-only PNG pixels with scene/geometry/source/PNG hashes.
UI annotations and label overlays are excluded. Local export creates a new folder
and does not overwrite existing files. Label maps stay on the source grid and
need matching affine/reference information; no registration is implied.

The approval broker has no catalog, renderer or study access. A private per-user
record and live OS lock advertise its available IPv4 loopback port. The stdio
connector selects an instance and checks its identity before requesting review.
Discovery is not authentication. Browser origins, noncanonical hosts/endpoints,
redirects and proxies are rejected; requests and lifetimes are bounded.

Allow creates an in-memory grant for the selected series, tools and either
approved exact views or series exploration. The data server gets that prepared
source only after review. No tool accepts a path. Revocation cancels work, rejects
late results, drops retained sources and invalidates the transport. Restart
restores no grant. Frozen comparisons share one reviewed image/context with
separate recipient tokens, not a live study browser. [Protocol and consent](agent-access.md)

## Updates and release integrity

Only the update worker contacts GitHub; it receives no study data. A saved choice
is required for background checks. Downloads/installations and restart are
separate user actions. Ed25519 signatures bind exact manifest bytes; target,
version, name, size and hashes are checked before bounded extraction.

Managed installations are staged beside the app, audited and swapped with a
complete rollback copy. Package-manager locations, added or missing files and
unwritable folders keep an external/manual update path. macOS verifies the
installed and staged Developer ID signatures and assesses the staged app with
Gatekeeper. The macOS signature checks sealed bundle contents, but does not
authenticate the separate stapled ticket. On Windows/Linux, managed file contents
are compared with the local build inventory before replacement and backup
removal. That inventory is not authenticated and cannot detect a coordinated
change to itself and a file. Backups with a detected mismatch are preserved.
Explicit restart revokes grants and protects
unsaved labels. [Release process](github-release.md)

Native codec and patched windowing sources, licenses and file inventories are committed under
`vendor/`. Packagers verify them and include the receipts and third-party notices
with binaries. No codec/model download occurs in the installed app.
