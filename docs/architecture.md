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

Core volume materials describe bounded piecewise linear luminance and extinction
per physical millimetre, independently of source values and display windows.
They are not implicitly added to existing saved scenes or snapshot parameters.
The CPU gradient reference transforms index derivatives by the affine inverse
transpose, including regular shear. Its boundary stencil stays within the source;
missing samples give no gradient, and constant fields give no surface normal.
Normalizing by component scale avoids an intensity-unit cutoff. These definitions
allow derived rendering to be checked independently of a GPU shader.

Metadata-first indexing avoids decoding every image at import. A bounded metadata
prefix reader applies object-class admission before reading dataset values and
stops at the pixel payload without allocating it. Image admission checks both file
meta and the dataset SOP class, so supplied mask objects do not enter grayscale
decoding through image-like pixel attributes. Requested frames
share a bounded LRU; each indexed read checks source identity. Cached-entry locking
is separate from slow I/O/decoding. At most four hot decompression readers are
retained per study cache, with matching-stream reuse and cancellable contention. ZIP/gzip/deflate
processing streams without extracting patient files to disk. Dimensions, payloads,
metadata, expanded data, frame counts and materialized volumes have explicit
bounds. Load, decode, filtering, statistics and export run on cancellable workers.
[Exact limits](supported-formats.md)

Core label-plane sampling keeps categorical IDs separate from eligibility.
Nearest-voxel cells are half-open from -0.5 to dimension-0.5, with half ties toward
the larger index. An ID on a missing source sample remains stored but is ineligible
for display; completed output validity is intersected separately. Sampling refuses
slab projections and mismatched declared references. Callers must additionally bind
the exact source and completed reformat generation before compositing. Temporary
ID/eligibility buffers are capped at four Mi-pixels and charged to the shared CPU
volume pool. Source counts preserve stored and valid voxels separately; physical
cell volume is the absolute determinant of the source affine, including shear.

Native label overlays use a separate coalescing worker from edits/exports, with
one active and one latest request for at most three completed images. Cache keys
bind weak map/image identities, exact source volume, selected series, view
generation/preview, display settings and texture-side limit. Replaced layers clear
immediately; weak keys do not keep old maps or frame buffers alive. Rasterization
and source counts stay off the UI thread. Prepared/cached textures retain bounded
CPU/GPU reservations, and upload checks the current egui texture-side allowance.
An independent coalescing count worker recomputes only on map changes. Both wait
on condition variables when idle; shutdown changes the predicate under the wait
mutex to avoid a lost wakeup. No label data enters ordinary snapshots or tools.

Seeded label operations traverse source-grid face neighbors with an admitted byte
mask and u32 queue, bounded by the existing label-grid limits. Missing samples and
other IDs stop traversal; cleanup preserves missing stored cells. Queue storage
is transient and the proposal mask retains its reservation. Every immutable label
map owns its own CPU-pool reservation; fallible cancellable checkpoints replace
unbudgeted deep clones. Header-only grid checks precede dense preparation/import.
Native previews bind exact map/source, operation parameters and an invalidation
epoch; source-plane picking uses the inverse display transform. The worker also
prepares the proposed immutable map. Explicit Apply only swaps that map and adds
one undo checkpoint, so no voxel scan occurs on the UI thread. Replaced/cancelled
results cannot publish after input invalidation. Current counts/exports remain
applied-map operations and ordinary agent snapshot contracts do not include labels.

The core read-only segment model retains a separate packed mask and source-plane
coverage for each numeric segment ID. Omitted planes stay distinct from supplied
zero masks; positive bits on missing source samples remain stored, with separate
valid-source counts. Overlap counts source cells once even when three or more
segments share them. Native one-bit frames concatenate before final even-byte
padding, including frame boundaries within bytes. Construction admits retained
masks/coverage/metadata and transient overlap bitsets before allocation, and
cancellation discards the private result. This model requires callers to verify
source association; it does not itself parse or validate DICOM references.

The worker-only selected-file SEG reader verifies explicit SOP class/instance/
frame references inside the selected indexed DICOM series, then maps original
source frames to the exact prepared volume's affine order. Native little-endian
one-bit pixels, matching reference/plane geometry and a single source per mask
frame are required; no resampling or reference paths are followed. Multiframe
sources require an explicit original frame number. Duplicate segment/source-plane
pairs are refused rather than folded across acquisition axes. Metadata parsing
rejects repeated attributes within a dataset while allowing separate frame items;
syntax admission precedes dataset parsing. File/source identity checks surround
import, resources are admitted before parsing/reading, and public results retain
numeric IDs and masks without source UIDs or segment descriptions. Desktop import
is an explicit selected-file action, separate from ordinary image catalogue admission.
Series decoded in memory, such as the bundled demos, have no indexed headers;
each of their frames keeps its SOP class, instance and object frame count so the
same explicit references can be followed. An embedded demo mask is read from the
app's own bytes through the same checks, without writing a file.
FRACTIONAL frames are thresholded to bits at or above half the declared maximum
before the same model is built. LABELMAP frames name no segment: each listed plane
is supplied for every declared segment, zero is background, an undeclared value is
refused, and segments cannot overlap.

Manual labels and supplied segments share allocation-free nearest-cell mapping
for completed thin planes. Supplied planes keep independent u64 positive and
supplied-coverage bitsets, with bit positions bound to numeric segment order;
omitted planes and overlapping segments remain distinguishable. A separate byte
intersects source/output sample validity. Sampling admits 17 bytes per output
pixel before allocation, caps planes at four million pixels and cancels without
publishing partial buffers. Matching geometry alone does not bind a completed
image to the source: the caller must preserve exact source/view identity.

The native labels panel selects either manual labels or read-only supplied masks.
Each keeps its own source association and state; manual undo/dirty guards never
flatten supplied masks into uint8 IDs. A generic overlay cache retains weak map/
completed-image keys, exact source and view generations, visibility/style and
device limits. One coalescing worker samples and prepares charged CPU/GPU textures
for at most three completed planes. Supplied import uses another cancellable worker;
failed/cancelled replacements preserve the previous set, and stale generations
cannot attach. Manual-map removal preserves supplied masks; source closure drops
both. Numeric counts/coverage and overlap are local display controls, with no new
ordinary snapshot or agent-tool scope.

Import reports retain category counts and at most 64 generated item ordinals in
RAM, without filenames, paths, raw tags or parser messages. Detailed errors retain
the report even when no supported image loads. Uncertain representation failures
remain explicitly unsupported-or-invalid; cancellation, changed sources and hard
resource limits remain failures rather than skipped inputs. Reports create no
automatic files and add no agent scope.

DICOMDIR parsing is a bounded, read-only Basic Directory subset. Record offsets
are checked against actual Item boundaries in original file bytes, then traversed
with cycle, hierarchy and depth checks. IMAGE references are hints: the existing
header index verifies the object's own SOP class/instance, transfer syntax, study
and series before accepting it. No patient/study free text reaches the catalogue.
Temporary study numbers preserve declared grouping without exporting identifiers;
unknown study declarations remain ungrouped. Series debug output omits internal
keys and identifying frame geometry.

Local file-set sources retain one shared `cap-std` directory capability. Validated
relative File IDs resolve with unique case matching; later reads reject links and
source replacement and remain inside that original handle even if ambient parent
paths change. There is no canonicalize-then-ambient-open fallback. ZIP references
use a validated, unambiguous member map rooted at the directory index's archive
subdirectory; no extraction or ambient file lookup occurs. Open folder deliberately
scans files independently of indexes, providing an explicit fallback. All paths,
directory records and grouping identifiers remain private core state in RAM.

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
tables share allocations. Native/snapshot shaders apply one explicit VOI mapping
after modality conversion/interpolation and before inversion. Conversion explicitly selects the
validated frame transform, independent of decoder metadata precedence. Source/per-frame identity includes
storage, scaling, declared units and table definitions without raw tag text.
Reformats retain typed source context and mark resampled values; mixed units or
lookup tables cannot silently become one spatial presentation.

Nonlinear modality tables are distinct from VOI tables. The worker maps exact
stored integer codes before interpolation, masking unused bits and sign-extending
the declared stored precision. One root table replaces affine rescale; conflicting
or functional-group table declarations refuse. Native code reads preserve OW's
low-bit-first cell packing across words and frames, independently of word byte
order. Bit-packed table inputs read only the selected bounded range and normalize
one frame for codec metadata, without materializing an entire packed sequence.
Source padding uses the same code reader. Codecs keep their validated decoded
sample representation. No inverse mapping reconstructs discarded stored codes.

Float Pixel Data has its own reader. It never passes through integer Bits Stored,
padding or codec decoding. Indexed reads take one frame's bytes in transfer-syntax
order. Eager reads use the parsed OF values. Both share one sampler that marks
float padding and nonfinite values missing with a finite placeholder, so display
ranges, statistics and sampling all use the existing validity mask. The bounded
metadata prefix stops at the OF element, so import requires that payload to end
the object instead of reading on to look for an integer Pixel Data element.

Supplied overlays and shutters are parsed once per source object from the
bounded metadata prefix into immutable layers shared by its frames. Per-frame
object copies drop overlay bitmaps. Reformats and projections never carry
layers. The app composes one RGBA texture for the shown frame only when the
frame or a layer toggle changes, and paints it through the same image-space mesh
as labels. Export, snapshot and agent paths render without it.

A presentation state is an explicit worker import like supplied masks: it reads
one bounded file, resolves its SOP instance and frame references privately
against the selected indexed series, and returns only image indices and display
settings. The app saves the prior window, inversion and orientation, applies the
state to the 2D view, and restores them on **As acquired** or when navigation
leaves the referenced images. The standard flip-then-rotate order maps to a
counter-rotation followed by a displayed-axis flip.

A real-world value mapping is a third, mutually exclusive source mapping beside
modality rescale and modality tables. Only one linear item is accepted, so each
sample keeps one retained value and no alternative is chosen silently. The scalar
context records its slope, intercept and stored range. That record joins the
context hash only when present, so contexts without it keep their existing
identity. Slices with different mappings do not form a volume.

Scalar context owns the bounded immutable table and an optional fixed-size
descriptor/digest. Table outputs are unsigned; later VOI input interpretation
uses that domain. The metadata omits entries, explanations and raw unit codes.
Matching uniform table identity remains attached to resampled values as an already
applied source mapping. Differing tables stay in 2D. The absent-field serialization
and scalar hash stream remain byte-compatible with existing contexts. Indexed
metadata and resident imports charge retained scalar tables under their existing
bounds; resident admission checks before publishing a newly decoded series.

Scalar values retain an independent validity mask when needed. DICOM padding is
identified in stored codes before rescale or modality lookup; statistics, hashes, labels, CPU/GPU
sampling and memory accounting share this validity domain. Color retains source
RGB code precision separately from sRGB presentation; compositing/resampling use
linear-light sRGB. No color-to-scalar modality intensity is invented.

Validated regular volumes have an affine and inverse mapping between source
indices and LPS millimetres. NIfTI sform/qform method, units and reference code
remain explicit: template/aligned coordinates are not assumed to be patient
coordinates. Unknown geometry stays viewable in 2D without inferred anatomical
labels or physical measurements.

NIfTI acquisition metadata is auxiliary to the existing serialized subvolume
identity. One opaque family capability per header opening distinguishes inputs;
no names, pixel similarity or generated series numbers establish association.
A cancellable import-worker catalog sorts at most4096 numeric tuples for bounded
selection lookup, without retaining source/image capabilities. It refuses missing
or ambiguous tuples. Sidebar choices use the existing series-change and active
comparison-pane paths, preserving revocation, unsaved marks and label ownership.
Reconstructed views close rather than retaining an unchecked cross-time point.
Time metadata is normalized to seconds only for declared time units; frequency
units remain explicit. Invalid timing preserves index navigation. The catalog and
new timing metadata stay in RAM. Existing tuple/complex identity remains in source
hashes and snapshot metadata; no new saved-view or settings fields are added.

DICOM functional groups are validated before decoding. Explicit per-frame geometry
and numeric/coded acquisition context partition nonspatial dimensions before stack
construction. Acquisition digests participate in source identity without exposing
raw tags. Missing dimension/geometry context keeps source browsing available;
conflicting macros and unsupported value mappings fail. Single-frame decode clones
only its selected functional group, avoiding a full per-frame metadata-table copy.

DICOM acquisition selectors are built on an import worker from retained headers.
Known temporal/echo dimensions keep ordinal and pointed value separate. Family
membership uses the private source series/reference key and a validated dimension
organization/pointer/context digest with only those selected dimensions masked.
Every other existing partition field remains bound. Source partition hashing uses
the original unmasked stream, covered by independent golden bytes. Catalogs refuse
inconsistent or ambiguous tuples and retain no source/image capabilities. Descriptor
cache and members are bounded to4096; catalog copies of raw scope text exist only
during preparation.

Optional cine metadata additionally requires every indexed frame to have the same
header/source and its original object ordinal, with no missing or extra frames.
Fixed intervals are constant storage; variable intervals use at most65,536 bounded
entries with fallible allocation and cancellation. Parsing validates the pointer,
decimal representation, nanosecond duration range, trim and delay without using
recommended rates or absolute dates. Invalid optional timing does not alter image
identity or ordinary browsing. These auxiliary catalogs add no saved schema or
agent fields.

Source playback is an explicit RAM-only opt-in bound to the selected catalog's
timing Arc. Replacing its source clears the opt-in and releases that capability.
Changing playback mode revokes live/pending/frozen sharing approval, without adding
timing state to image or scene schemas. Existing completed-view freshness still
governs every requested frame. Manual fps and Loop settings remain separate.

The monotonic scheduler admits one requested image at a time. It waits for that
exact completed image without a polling timer, then uses the interval before the
next source frame. Deadlines use checked Instant arithmetic. Late UI/decode work
never skips frames or queues catch-up requests. Source trim is inclusive; the last
trimmed image stops playback immediately, without an invented end/wrap interval.
Frame Delay stays acquisition metadata. Reported ready-to-ready pace and image
wait include local scheduling; they are not a real-time reproduction guarantee.
Pause clears outstanding timing state for focus, dialogs, manual input, errors,
view/source changes and end. Paused views schedule no continuing timer.

## Rendering and interaction

Explicit image inspection also summarizes the selected series on a cancellable
worker without decoding its other pixels. A shared regular-stack geometry
validator supplies affine construction to both metadata inspection and dense
volumes; its temporary ordering table uses shared RAM admission. Readiness
describes geometry and label-grid size independently of pixel compatibility and
current CPU/GPU admission. Lossy/derived declarations remain typed aggregate
counts, including unknown values, without raw metadata or new snapshot fields.

The UI paints the last completed acquired image or coherent linked-plane result
while newer work is pending. Workers coalesce obsolete requests and preserve
cancellation. Linked planes share one rendered crosshair/generation; overlays do
not publish a new point before its images are ready. Volumes retain an interactive
preview while idle tiles refine with immutable per-dispatch parameters.

GPU resources are owned per view, while immutable source frames are shared.
Offscreen slices can also return a bounded worker-owned RGBA8 target for local
composition, independently of snapshot manifests. Ordinary snapshots encode the
untouched target and retain their image-only contract and existing size caps.
Target textures and readback/encoding working storage have separate admissions;
row copies and encoded-output growth are fallible and bounded. Callers retaining
encoded bytes must account for that storage in their artifact/cache lifetime.
Explicit desktop illustrations capture immutable completed outlines and the
selected applied manual/supplied mask group separately from image-only exports.
A worker-owned egui context uses bundled fonts and shared native coordinate,
outline and categorical-mesh painters on the admitted offscreen target. Ellipse
contours and aggregate projected geometry are bounded before tessellation;
composition CPU/GPU working storage, mask planes and retained artifact/preview
textures have separate admissions. The separate illustration manifest records
layer choices/settings, coordinates and hashes without raw source identifiers.

Preview generations bind input identity and layer choices. A change drops the
previous result/texture and disables Save. Saving rechecks fresh source content
and private reference identity, then writes the exact encoded preview bytes.
Output uses exclusive numbered folders and tracks each successfully created file
for nonrecursive cleanup; a failed second creation cannot delete another writer's
file. The Workspace export lifetime closes both flows on source/mode changes.
Ordinary snapshots/tool schemas retain their image-only contract.

The renderer also provides a separate admitted CPU mosaic for at most 16 bounded
slice tiles. Its fixed-cell layout scales down to a 2048-pixel output side without
changing SceneState or shared snapshot/readback limits. Tiles can be read as RGBA,
copied row by row and released before preparing the next; the mosaic encodes once
with bounded PNG scratch. Duplicate, wrong-sized or incomplete tiles cannot produce
a PNG. Callers must retain separate encoded-output and preview admissions. This
renderer interface does not expose a desktop grid/contact-sheet action by itself.

Desktop sheet capture keeps either three settled MPR planes from one snapshot or
one source capability, captured display settings and a bounded coordinate-only
source-mark collection. Private reference-key copies have their own bounded
admission; empty mark collections keep only raster/ordinal boundaries. Explicit
first/through/step validation yields at most 16
ordinals without eagerly decoding their images on the UI thread. A worker reads,
paints, copies and releases one tile at a time; generic captions and per-tile
scene/layer records enter a separate version 2 illustration manifest. MPR inputs
must share source/volume/point and applied mask identity. Numeric metadata scratch
and aggregate JSON output are bounded separately. Preview generation includes
range/column choices; Save reuses bytes after a fresh source check.

Window/level, zoom, pan, interpolation and validity use the same implementation
for desktop and offscreen rendering. Repaint is driven by input, results, active
playback and finite animation. Idle viewing does not require a polling loop.
Measurements map image centres to validated physical coordinates rather than
screen distance. Projections cannot masquerade as measurement planes.

Temporary marks are bounded per series/pane and keyed by source-frame ordinal
or exact reformat geometry, independently of decoded allocation addresses.
Completed marks retain coordinates/results without retaining source pixels.
One coalescing ROI worker per pane publishes to its original mark ID and geometry
revision. Replaced/cancelled jobs cannot publish, and an older completed result
cannot overwrite a revised mark. The shared idle-blocking job helper also serves
label counts, supplied-mask import and categorical overlay preparation.
Core region APIs also support bounded ellipses and simple polygons, with inclusive
pixel-centre boundaries, conservative topology/degeneracy checks and cancellable
visits without a dense mask. Scalar/RGB code reductions share stable moments;
new wrappers separate continuous geometric area from valid sampled-pixel area.
Physical area uses the validated plane basis; missing or nonrepresentable geometry
keeps area unknown. Existing rectangle APIs and serialized statistics retain their
fields. Native creation supports distance, angle, rectangle, ellipse and simple
polygon marks. ROI candidates remain private until their owning enabled canvas
accepts worker validation for the exact image and tool. Context changes discard
drafts, and reformats admit interaction only on the matching settled thin plane.
Each collection has 128 marks, 4096 total points and 256 vertices per polygon, with
1 MiB of admitted metadata storage retained through pending worker inputs.
Vertex gestures and numeric proposals leave the original mark intact until
worker validation and exact owner/base-revision acceptance. No per-drag sample
scan runs. The 16-entry undo/redo history stores admitted, fallible coordinate and
validated-report pairs by mark ID, without frames or copied source metadata.
Reports travel with their already validated geometry on the unchanged source;
fresh monotonically assigned revisions reject late worker results on undo/redo.
Deletion/clear invalidates history, and source/view closure releases it.
Series previews use one idle-blocking worker, coalesce visible-row requests and
reuse the decoded source cache. At most 32 small textures remain in RAM, with
shared RAM/GPU reservations; closing the study cancels work and drops previews.

Local comparison owns two or four independent latest-frame workers and pane states.
Panes may use the original study/cache or a separately imported local source;
all sources share process-wide admission pools. Extra panes reuse existing source
capabilities when possible and choose other series from the loaded studies. There
are at most four frame workers and four bounded geometry tasks; no additional
study is imported while four panes are present. Optional slice
linking validates header geometry on workers and uses a physical image-center
point with nearest-plane lookup. Bounded navigation tables share prepared-source
RAM admission; comparison does not materialize dense volumes. The active pane owns
tools, contrast and single-picture export. Click/drag/scroll or the pane buttons
activate it; passive hover does not change its tools.

Four panes use a two-by-two grid; two panes stack vertically only when the area is narrower than it is tall.
Only the chosen pair receives slice/display links and reference lines. Changing
the pair clears link opt-ins and pending actions. Extra panes remain outside the
two-pane saved schemas, and comparison-file operations refuse the four-pane
layout. Closing extras requires an explicit keep/discard choice, releases their
workers, marks and GPU slots, and restores the original pair. Marks in all panes
participate in existing source-replacement and workspace-close guards. Image and
illustration exports capture only the active pane.

Fusion draws the pair's second pane over its first as a display-only texture. A
coalescing worker resamples the second pane's current image onto the first
pane's pixel grid only when both pass the same compatibility test as slice
linking and their planes lie within half the second stack's spacing. Sampling
is pixel-centre bilinear and leaves a point missing when any weighted neighbour
is missing, so absent data never becomes zero. The second pane's own window and
inversion map its values; opacity is applied at paint time. Probes, statistics,
picture exports and snapshots never include the fused texture, and the pair
change clears it. There is no registration or transform import.

Optional reference lines use only matching completed frame/view pairs. Core clips
thin-plane intersections to both declared image footprints and rejects unknown or
different private references/NIfTI contexts. Parallel/coplanar cases are explicit.
The UI reserves paint slots beneath other canvas overlays, checks both frame Arcs
and views after input/linking, then fills only coherent slots. Screen clipping
bounds each dashed line to 128 dashes. State stays in RAM; image exports, snapshots
and comparison-file schemas omit reference lines.

Core physical display helpers describe a borrowed source centre and vertical field
of view in millimetres. Pan-only keeps target zoom; zoom-only keeps its physical
centre. Candidates preserve other view fields and reject reference, orientation
or range mismatches. Scalar-window compatibility additionally requires known
units, matching scalar representation/mapping and lookup-table identities.

Independent desktop pan, zoom and scalar-window opt-ins forward typed user changes
once. A single pending generation keeps weak source capabilities, expected views
and exact canvas aspects, without retaining pixels. Matching requested images must
complete first. Ownership, source, target, settings or viewport changes cancel the
pending action; decoded failure indices stay bound to their request. Changes from
the same source coalesce their action masks. Canvas size changes start a fresh
physical-link action for the new layout, preserving physical association. Physical and scalar groups can refuse
independently. Linked target changes do not become input on the following pass.
Replacing a source or restoring views clears these RAM-only opt-ins; saved
comparison schemas and agent scope stay unchanged. Inversion stays per pane.

Entering comparison revokes live and frozen grants, cancels review/shared work and
removes agent composition. Comparison content is never published through MCP.
Leaving comparison returns to its active series and requires fresh Allow to share.
If the original source has unsaved labels, it remains the active source before
leaving comparison; a separate source cannot replace it until the labels are handled.
An explicit comparison file stores source fingerprints, pane views and source
frame/point measurements, without paths, pixels or raw tags. It is bounded and
written without overwriting an existing file, with owner-only file permissions on
Unix; other platforms use their normal destination-folder access controls. Reopening requires both
series to be selected again in order. Cancellable workers bypass the decoded
cache to fingerprint both sources, reject mismatches, and reconstruct marks and
statistics against fresh frames before publishing either pane. Unsaved comparison
measurements and changes to a saved view are protected on replacement or closure.
The file may still be sensitive; previews and study history are not persisted.
Comparison version 2 adds ellipse/polygon kinds with a 2 MiB file limit. Version1
retains its 256 KiB limit and original kinds; it is still written when sufficient.
Deserialization bounds individual point/mark vectors, then checks total points
before source decoding. File workers admit 16 MiB of scratch. Geometry and results
are validated/recomputed from fresh source frames; a late cancelled restore
cannot replace current work.

Quarter-turn/flip display transforms leave acquisition geometry and source pixels
unchanged. The source-to-viewport map is shared by probes, measurements, label
meshes/brush footprints and overlays; the slice shader applies its inverse.
Transforms participate in scene serialization/hashes and exact-view consent.
Volume projections use their camera orientation and reject 2D transforms.
The linked MPR worker retains one immutable thin image per anatomical plane,
keyed by that plane's physical coordinate and requested sampling side. Moving
another axis changes the crosshair without resampling or uploading that image.
Interactive thin updates retain the requested spatial sampling; they remain
provisional for exact-view sharing until input is quiet. A coherent set completes
despite newer pointer input, preventing starvation, while study closure cancels
the worker. Slabs retain their separate preview policy.
Scalar slabs sample bounded discrete extrema or a mean along an explicit physical
plane normal, excluding missing samples before display mapping. The mean sums at
most 257 finite samples in f64 and divides by the valid count. Slab hashes keep
their original projection bytes and give the mean a new one. CPU workers publish a
small coherent preview, then refine once input is quiet. Projection parameters
and the sampling grid travel through scenes/snapshots; public image metadata does
not label them as measurement geometry. Measurements refuse projected frames.
No full-depth projection buffer, source copy or persistence is introduced.

A core-only curved reformat prototype samples a scalar volume along a bounded
manual polyline (at most 256 points, 2048 pixels a side, 4 Mi samples). Columns
follow arc length; rows follow a fixed reference direction projected
perpendicular to each segment, and curves nearly parallel to it are refused.
Repeated points are dropped. Values use the validity-aware trilinear sampler, so
missing and outside samples stay missing. The result has no planar geometry,
which keeps measurements unavailable, and exposes an exact source position per
pixel plus a curve/sampling digest. It has no desktop editor or export yet.

A brick-layout prototype addresses one measured GPU-residency failure: a
generated 4096×512×256 f32 volume is admitted on the CPU but exceeds Metal's
2048-voxel 3D texture side. Bricks own cells along each axis and store one halo
voxel, so brick-local trilinear sampling matches the dense sampler, missing
samples included. Extraction is admitted against the GPU budget one brick at a
time. The dense CPU source stays resident; there is no disk spill. The volume
renderer does not yet ray-march across bricks, so 3D still refuses such volumes.
On the test Mac the dense path's binding limit for ordinary shapes was CPU
admission, not GPU memory.

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
UI annotations, manual labels and supplied masks are excluded. Local export creates a new folder
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

## Local actions and keyboard settings

The desktop action table owns default tool keys, dispatch and the active labels
used by controls and Help. Remapping permits unmodified A–Z or no binding;
duplicate assignments are rejected. Focused controls, open menus and auxiliary
workspace dialogs retain their input. Temporary Space navigation also respects
canvas ownership, so activating a menu button cannot cancel a coordinate edit.

A sleeping worker loads and atomically replaces a bounded `controls.json` in the
per-user application settings folder. Its versioned schema stores only the tool
bindings. It contains no source paths, study history, images, labels or view state.
The active bindings change after a successful save; a failure keeps the old set.
No default file is created just by opening the viewer.

Typed measurement points pass through the same current-image bounds, admitted
coordinate budget, validation worker and canvas acceptance as pointer input.
Changing image or tool clears queued point input. Explicit MPR plane selection
and typed physical coordinates reuse the existing shared point/generation model.
The bundled controls guide has finite manually advanced steps and no loading,
networking or automatic source navigation.
