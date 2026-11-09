# Architecture

OpenRadView is a Rust 2024 Cargo workspace. Dependency versions and the toolchain
are pinned. Rendering is native wgpu; egui/eframe owns the desktop shell. macOS
selects Metal; Windows selects DX12/Vulkan and Linux allows Vulkan/OpenGL. Core and
rendering code remain independent of the UI. Native instances share the same
validation flags. Windows keeps shader optimization enabled in debug builds:
FXC cannot lower some bounded, dynamically indexed loops with optimization off.
Backend validation remains enabled in debug builds. Shaders select vector lanes
with masks instead of runtime component writes, which FXC otherwise tries to
unroll across Naga's bounded-loop guards.

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
GPU scalar interpolation preserves equal endpoints exactly, preventing rounded
constant samples from introducing a lighting normal. Dense, classified and paged
sources use the same interpolation rule.
Normalizing by component scale avoids an intensity-unit cutoff. These definitions
allow derived rendering to be checked independently of a GPU shader.

`GpuVolumeSource` retains one admitted upload, optional validity texture and
source VOI tables, with its creating device/queue. Projection renderers can share
it without another source allocation. Source staging and per-renderer outputs
have separate reservations, retired only after submitted work completes.
When validity is present, each texture byte packs the eight clamped positive
stencil vertices, with the source voxel in bit zero. Interpolation tests only
nonzero-weight contributors; direct voxel users read bit zero. Packing uses one
admitted CPU plane on the worker and does not add a second GPU validity volume.
Core `ReformatGrid` prepares physical geometry, dimensions and slab sampling
without image allocation; CPU resampling uses that same grid. An opaque RAM-only
owner binds each grid to the exact source volume. Equal geometry does not make
grids interchangeable. Derived scalar and color reformats retain CPU admission
through their immutable `Frame`, including clones held by tools or exports.
Volume construction validates scalar finiteness and binary mask lengths and
values, so CPU interpolation can check the immutable masks directly for its
nonzero-weight source samples.

The scalar GPU reslicer has separate compute-only tiled outputs and worker-only
completed frames. The latter accumulates source coordinates as signed 24.40
integers using u32 carry arithmetic. Preflight bounds partial sums and coordinate
uncertainty against the CPU's f64 evaluation. Interpolation and slab accumulation
remain f32, with conservative error and possible-contributor checks. Uncertain
validity or arithmetic requests exact CPU sampling for that pixel, including all
declared slab samples. Subnormal source values and unsupported coordinate/device
limits refuse this path. Missing nonzero-weight contributors invalidate a thin
sample; slabs omit missing samples and retain the declared interval count.

Up to three requested planes share one bounded readback and cancellable fence.
Only complete results escape: required CPU corrections are uploaded before an
immutable GPU image and its matching CPU `Frame` are published. Requested sizes
are preserved; absent slots do no work. Completed images retain source, metadata
and pixel admission independently of renderer reuse or teardown. The slice
presenter can bind their scalar/validity texture directly while tools retain the
matching CPU pixels. Its constructor uses the image's own GPU context, and later
uploads require the same exact source owner; raw native device IDs can repeat
across instances. The existing display mapping, VOI and transforms are shared.
Native linked MPR offers GPU handles once to its worker, which uploads one scalar
source and reuses it across navigation. Completed planes bind directly for display;
labels, measurements and local picture/sheet exports retain their matching CPU
Frames. Color, unsupported arithmetic/device limits or refused GPU admission use
the same requested CPU grids. A failed GPU path is retired for that view session,
avoiding repeated upload or precision failures on each pointer event.
Exact anatomical thin and slab grids use the source's world bounds and the
plane's normal coordinate. Moving the linked point within a plane therefore
does not change the grid through pivot-dependent floating-point rounding; only
exactly matching anatomical axes take this path.
Leaving linked MPR suspends that worker and releases its GPU source, plane cache
and completed snapshot. Position, display settings and annotations stay in the
session. Returning rebuilds from the same CPU volume on demand; inactive views
do not reserve a second dense GPU source beside the active 3D view.

The native `MPR + 3D` layout adds a fourth pane to the same session, using a
2×2 grid in taller windows and four columns when less height is available.
Optional MPR controls fold away; layout and active-plane selection stay visible.
It prepares
volume/material renderers on a worker from the exact `GpuVolumeSource` retained
by completed MPR images, without uploading another dense scalar source. If MPR
uses its CPU fallback, 3D uploads that same immutable CPU `Volume` owner; it does
not materialize the series again. An older CPU snapshot is withheld from this
binding while a newer GPU request is pending. Leaving the layout cancels and
drops its 3D worker/renderers; leaving MPR also suspends the reformat worker.
The shared label UI resolves the current source-bound map, revision and ID each
frame, including while 3D settings are closed. Region work retains its existing
one-opaque-ID subset and coherent immutable completion rules.

The workbench projects the completed MPR point and source-axis lines through the
3D camera, with that camera's pan/zoom/roll and target aspect. Immutable region
fronts retain their actual camera even while a new camera is pending. The source
volume guide is withheld if its current presentation does not match the desired
camera. Guides are not depth tested and provide no picking/editing authority.
3D orbit, display settings and fitting do not change MPR display or position.
Combined views are local: entering this layout revokes single-view approvals,
denies queued connections and withholds source-substitute exports. Canonical
single-view reconstruction and sharing contracts do not describe a workbench.

Linked cuts select a lower or higher LPS halfspace through the completed MPR
point on each anatomical axis. They live separately from manual clipping and
intersect it only in the rendered `VolumeView` clone, shared by source, mask,
cell-face and probe paths. Unselected axes enclose all eight affine half-cell
corners with a source-cell margin, preserving outer mask faces under shear and
reflection. Empty ordered bounds and unsupported numerical ranges show an
explicit empty/unavailable scene and suspend region work, revoking pending probe
authority. They never substitute an uncut source or construct a cap. Resetting
linked cuts preserves manual bounds; neither operation changes MPR sampling,
source/label data, counts or the physical brush extent. Cuts and source-plane
visibility are separate controls; neither provides independent plane poses.

The `section_volume` backend composes up to three opaque, two-sided thin
`GpuReformatImage` planes and an optional known cell-face interface with source
optics. Each plane retains its corrected
scalar/validity texture and exact GPU source owner. Physical camera rays are
projected into pixel/normal coordinates in f64 before upload, avoiding f32
subtraction of large LPS positions. Display pan, zoom and flips do not move plane
geometry. Scalar bilinear validity, VOI functions/tables, MONOCHROME1 and inversion
follow slice presentation; mapped grayscale is converted to linear luminance.
Source rays, section planes and cell-face depth share an origin relative to the
source centre, shifted onto the perpendicular camera plane in f64. A large camera
target displacement along its viewing direction does not change depth ordering.
The nearest valid actor stops source integration; missing plane stencils can
expose farther actors. Equal-depth planes use stable slot order, and a known
interface wins a coincident plane tie. Its actual affine normal supplies surface
lighting. The interface must match the exact source, camera, navigation, size,
clip and depth reference; unknown and clipped interfaces stay open. Manual clips
apply to source rays and actor hits. Plane clipping evaluates six halfspaces in
the plane's own pixel coordinates, with f64 differences and normalized
coefficients prepared before upload. A coplanar cut therefore keeps the entire
plane without a physical tolerance or reconstructed f32 world-position noise;
parallel planes outside the bounds remain excluded. Unrepresentable nonzero
coefficients refuse rendering. Complete contiguous row sequences retain
exact actor/view/material identity and check a GPU arithmetic flag before
publishing immutable radiance. Old completed images keep their owners/admission
through cancellation or renderer teardown.

An optional worker-only section probe uses the same actor ordering and optical
function as the completed image. Its 56-byte readback includes the original pixel's
cell/depth, unquantized radiance, foreground transmission, selected actor and
completed color. It checks arithmetic and color, then independently traverses the
CPU mask in f64 and intersects the thin planes with their completed CPU validity
stencils. A covering plane or foreground transmission below 5% withholds the
region hit. CPU/GPU ordering or membership disagreement and exact face-edge
contacts are ambiguous. An exactly zero plane-normal residual proves a shared
physical face contact without a tolerance; adjacent representable foreground
planes still cannot gain a hit through GPU rounding. Real backfaces remain
eligible where clipping or unknown coverage leaves the surface open.

`SectionPickFrame` retains the job's originating revocable `PickScope`.
`SectionPickState` must match the live CPU request as well as the exact prepared
scene, including source/map/revision/ID, plane owners, VOI/inversion, interface,
material, lights, camera/navigation/clip and size. A matching completed frame alone
cannot establish the live desired state. The result repeats this check before
use. Lazy probe-pipeline preparation, per-call admission and cancellable waits
stay off the UI thread; probe failure leaves display available. This read-only
backend grants no editing, export or disclosure authority.

`probe_region` checks at most 4096 original-image pixels and 16 rows in one
worker operation. Single clicks and rectangles share one compiled 8×8 entry
point, avoiding differing arithmetic between entry points near a visibility
decision. The one-pixel readback remains 56 bytes. Larger rectangles use separate
aligned cell/depth/optical/color row pitches, with CPU working storage and GPU
output/readback admitted before allocation. Each returned row-major sample keeps
its original full-image coordinates and includes empty, obscured and ambiguous
outcomes. Every sample passes the same color and independent f64 checks; an error
or revoked scope returns no partial batch. The retained batch must match the live
scene and requested rectangle before use. This is visibility proof infrastructure;
it does not yet implement surface painting or grant mutation permission.

`SectionFaceErase` is a separate worker proposal operation over explicit checked
batches, with at most 65,536 original-raster samples in total, including duplicate
regions. It requires one exact completed image and originating scope, a current
manual mask, and an erase clip matching the completed scene. Only Hit outcomes
contribute faces to the core `FaceErase` proposal. Covered, empty and ambiguous
samples contribute no cells; outcome counts remain available for review. The
private preview retains the original image witness, and Apply rechecks its whole
scene/scope plus the exact manual base, path and brush settings. Scope revocation,
cancellation or memory failure returns no partial edit. This operation converts
an explicit sparse selection; it does not capture a native gesture, interpolate
across covered input centres, edit supplied masks or authorize disclosure. Native
integration must additionally retain/revalidate its live CPU request, edit session
and ordered input epoch before publishing a revision.

`propose_erase_stamp` converts a checked original-image click and physical sphere
radius into that proposal. A Hit anchors the sphere at its source-cell centre.
The proof rectangle expands the projected sphere by the largest affine half-cell
corner extent, so a cell's visible face is included even when its centre projects
elsewhere. A conditioned f64 camera inverse, absolute-coordinate rounding bound
and extra pixel border conservatively bound the rectangle; unresolved zooms are
refused. These guards widen only the proof rectangle, never the physical brush
or a ray. The original raster is partitioned into at most 256×16 rectangles, each
using the existing color/optical/independent geometry checks. The total cap stays
65,536 pixels; oversize requests ask for a smaller radius or zoom out rather than
reducing resolution. The transient batch directory is admitted before allocation.
Cancellation/revocation releases all unpublished batches and edits. The native
one-click erase control uses this worker operation without lowering resolution.

The native workbench passes the completed GPU MPR images and each plane's display
window/inversion to a separate coalescing worker. Its optional region uses the
exact current manual or supplied map, including a physical edit proposal. Shared
cell-face preparation caches geometry/upload/projection by their original owners;
Apply can retain the prepared preview owner, while Undo returns to the original
map. Requested viewport pixels, capped at 2048 per side, and the explicitly
selected quality remain fixed during motion. One complete image finishes before
the latest same-data camera/plane request starts. Compatible prior frames remain
labelled updating, and current cursor guides wait for an exact completed match.
Source/map/ID/revision changes clear the old front and cancel data preparation.
Idle views submit no work. Slabs, absent corrected GPU planes, unavailable region
selections and empty cuts show a reason instead of a substitute source image.

Native cell-face preparation caches can be cloned for private correction work.
The immutable projector pipeline and initial uploaded/projected owners are shared;
subsequent geometry, upload and projection changes replace only the fork's cache.
Section composition accepts that private projection without replacing the main
face cache or completed front. Original visibility witnesses remain bound to the
base image, and dropping/cancelling a fork releases unpublished map owners.

**Surface erase stamp** is a separate native manual-label tool in MPR + 3D. It
requires the applied scalar-source map, selected nonzero visible ID and no busy
or competing label operation. Choosing a face arms one primary click; secondary
navigation and Space overrides keep their existing behavior. The job binds the
exact base/ID/series, linked-input epoch, viewport id/layer/rect/clip, originating
scene/scope and physical radius. Navigation, focus loss, popups, Escape and changed
inputs discard unpublished work. The original scene stays in the main viewport;
a separate window renders the private mask using the same camera, image size,
planes, clipping and optics. Apply rechecks the latest live view and label owner
at publication, then uses the existing one-step history transaction. No-op Apply
preserves history. Cancel releases the proposal; selecting read-only Pick surface
creates a fresh inspection scope. Erase mode can use section composition with no
visible source-plane actors; ordinary inactive-plane rendering stays unchanged.
The stamp never moves the linked point. Continuous surface paths, surface Add and
derived smooth geometry remain separate capabilities.

Source-plane mode retires the previous region renderer and revokes its pick
scope. Mask-cell mode is disabled while planes are active. **Pick surface** uses
the separate section probe, with a job-originating scope renewed for every desired
scene change. A camera round trip or viewport/id/layer/clip change cannot retag a
retained front; a new completed image must carry the renewed scope. A separate
bounded worker shares the renderer's engine mutex and rechecks the live CPU scene
and exact prepared owners before returning a checked cell centre to MPR.
Source planes receive the latest shared MPR window and inversion directly, even
before sibling pane display states finish updating. Pending MPR input, navigation,
double-click Fit, disabled controls, Escape, focus loss and popups clear pending
probes. A newer MPR generation cancels a probe without rerendering an unchanged
3D scene. Suspension and teardown revoke authority and release workers. Hiding
the planes restores the stored geometry choice, with picking off until explicitly
enabled. Choosing Volume or MIP hides the planes; enabling planes selects Material.
Independent plane poses and workbench sharing/export are not implemented.

The workbench's physical label tool proposes a sphere or thick disk at the
completed thin MPR point, or a continuous sweep drawn on one thin plane. The
disk normal follows the active anatomical plane;
radius and symmetric thickness use millimetres and source-cell centres under the
full source affine. It is an explicit volumetric stamp that can reach hidden
cells, independent of 3D display clipping. Supplied masks remain read-only.
Blank manual labels are created on a worker over the exact shared CPU volume.
`PhysicalStroke` prepares a private COW map on the label worker. The native input
scope binds the exact base/source, selected series, MPR generation/point, shape,
label, action, radius, thickness and protected IDs. Hidden nonzero IDs are protected;
source validity and competing-ID rules also apply. Pending navigation, slabs,
changed inputs or another label operation invalidate/cancel the proposal.

Drawing owns primary input in the Crosshair tool without moving the linked MPR
point or reducing its sampling. One charged path holds at most 8192 physical
points. Ordered raw pointer events preserve turns within an input batch; stationary
positions deduplicate. Display rotation/flips and pan/zoom are inverted before
the completed plane's physical geometry is used. Capture retains that exact Frame
Arc, view transform, viewport bounds, anatomical plane and input epoch. Navigation,
source/view changes, loss of pointer input, leaving the displayed image or capacity
refusal discard the whole captured path. A captured path cannot splice another
pane. Release submits the continuous sweep to the label worker and reopens the
review window. Escape or selecting another tool stops drawing. The point-preview
action explicitly replaces a drawn path, and an edit request made while the 3D
source is preparing remains available to the MPR session.

Thin MPR overlays and the manual 3D region use that same immutable preview map.
The 3D marker checks the actual painted region's map and ID: it shows
**PHYSICAL PREVIEW ONLY** for that proposal and **PREVIEW NOT IN 3D** while a
different region, source-only fallback or unavailable frame is shown. With a
linked cut the labels also identify that cut. The marker does not resize the 3D
viewport. Narrow windows put the edit entry inside 3D
settings. Counts and explicit label
exports retain the applied map; illustration capture refuses an unapplied preview.
Apply revalidates the scope and consumes the prepared proposal once, creating one
undo checkpoint. A no-op retains the original map/history. Closing the tool window
keeps its proposal for inspection; leaving the layout discards it. Visible-surface
correction remains a separate tool; these volume/plane sweeps can reach hidden
cells and do not claim surface visibility as mutation authority.

`Labels::seeded_region` is a worker-only, two-class propagation primitive on a
rectangular acquired scalar slice. Include and exclude seeds are fixed; source
padding and competing IDs are barriers. Four-neighbor synchronous updates use
scalar-domain affinity scaled by the valid, unprotected rectangle's range. A
constant range has unit affinity. Equal strength preserves an existing class;
equally strong improvements favor exclusion. Zero-strength attacks do not label
cells. The display window is not an input.

The calculation admits its two iteration buffers and retained seed/result masks
before allocation. The rectangle is limited to 1,048,576 cells, with bounded
iterations and 67,108,864 cell updates. Missing, contradictory or protected seeds,
cancellation, memory pressure and nonconvergence return no proposal. A completed
proposal retains the original label/source owner, seed set, parameters and class
decisions. Apply builds a private revision, preserving barriers, unclassified
cells and other IDs. Its caller must match the original base to the current
revision before publishing. The native source-image editor accepts typed points
or drawn include/exclude hints and an explicit propagation rectangle. Pointer
segments are filled on the source pixel grid; each stroke is one removable hint
and is rolled back on cancellation or the seed limit. An active gesture is tied
to its exact label map, selected ID, source slice, display geometry and viewport;
a changed scope cancels the gesture before it can continue on a different view.
The seed, index, group and worker-copy buffers are bounded and admitted
separately; changes to source,
map, slice, selected ID, rectangle, seeds or iteration limit invalidate the
private preview. Apply accepts only the current base and input epoch, and adds one
undo checkpoint. Counts and exports retain applied labels; illustration capture
refuses an unapplied preview. No trained model or anatomical recognition is used.

Agent panel/review/live sessions and unseen inbound requests select canonical CPU
reconstruction. Backend changes increment the request generation, invalidate the
plane cache and reject old-mode previews; the GPU source stays available for local
navigation afterward. Inbound requests remain unseen in the approval-only broker
until the canonical frame is settled. Canonical preparation grants no access.
Local picture readiness uses the actual completed Frame independently of sharing
readiness. The saved/shared scene reconstruction contract remains CPU-based.

The separate material volume backend integrates piecewise optical materials into
linear RGBA16F targets up to 2048 pixels per side. Its optional two-sided headlight
uses domain-clipped source secants and the affine inverse transpose. Constant or
missing gradient stencils retain unlit material instead of supplying a boundary
normal. Optical depth uses physical ray distances, with a stable small-depth
opacity approximation. Final presentation interpolates linear radiance and encodes
once for the framebuffer. Materials are explicit scalar-to-optics mappings;
grayscale windows/VOI tables are not applied, and display inversion is refused.
Native scalar volume controls expose a bounded material-point editor and lighting
toggle. Invalid drafts retain the last valid material. The upload worker builds
both renderers on one source; the callback key includes optical points and lighting.
Material interaction uses up to 512 pixels per side; idle refinement uses up to
2048 in 16-row batches, presenting only completed targets. The editor shares
bounded dialog placement, Escape handling and numeric Enter/Tab focus behavior.
This style has no saved/shared scene representation or picture capture yet:
entering it revokes live/pending approval, denies agent view application and
disables sharing/export instead of substituting the source-style image.

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
bind weak map/image identities, exact source volume, selected series, view kind,
display settings and texture-side limit. Unchanged image identities retain their
textures across MPR generations; changed planes clear immediately and only those
planes are resampled. Weak keys do not keep old maps or frame buffers alive.
Rasterization and source counts stay off the UI thread. Prepared/cached textures retain bounded
CPU/GPU reservations, and upload checks the current egui texture-side allowance.
An independent coalescing count worker recomputes only on map changes. Both wait
on condition variables when idle; shutdown changes the predicate under the wait
mutex to avoid a lost wakeup. No label data enters ordinary snapshots or tools.

Seeded label operations traverse source-grid face neighbors with an admitted byte
mask and u32 queue, bounded by the existing label-grid limits. Missing samples and
other IDs stop traversal; cleanup preserves missing stored cells. Queue storage
is transient and the proposal mask retains its reservation.

`CombinePreview` prepares an explicitly scoped union, intersection, subtraction
or copy from a selected manual or supplied mask into one manual ID. Both inputs
must retain the same prepared source. Unknown operand coverage, missing source
samples and competing IDs remain unchanged and are counted separately. The
preview retains its exact target and operand revisions and an admitted change
mask; applying it requires the current identities to match and creates a private
tiled revision. Cancellation or allocation failure publishes no partial edit.
The primitive does not itself provide a native control or authorize sharing.

Manual label maps use immutable 32-cubed tiles with clipped edge allocations.
A missing tile means stored zero IDs throughout; it does not replace source
sample validity. Checkpoints copy a bounded directory and share unchanged tiles.
The first actual write to a shared tile copies only that tile; erasing its last
nonzero ID releases the empty tile. Directory, tile, revision and export scratch
allocations retain CPU-pool reservations until their final reader drops them.
Cancellation or admission failure discards the unpublished private checkpoint.
Logical source-order iteration and bounded export chunks avoid a dense duplicate.

`PhysicalStroke` prepares a private tiled label revision on a worker for an
explicit source-space volumetric sphere or plane brush. Consecutive physical
points define continuous capsules, including end spheres; there is no sampling
at display-frame intervals. A plane brush projects the path onto its declared
physical plane and sweeps a disk through an explicit symmetric thickness. Closed
source-cell centre membership uses the fitted affine in millimetres, including
shear and reflection. Clips, scalar validity or color alpha, protected existing
IDs and competing categorical IDs are checked before writes. Supplied masks have
no mutation entry point. This is not a visible-surface correction tool.

The proposal retains its exact base Arc, complete ordered input path, edit
settings and read-only preview map. Apply consumes it only when the base, path
and all settings still match; the caller also revalidates its live
source/session/tool scope and input epoch. A real edit returns the already
prepared revision; a no-op retains the original map and history identity. There
is no voxel scan during Apply. Metadata, the retained input path, temporary segment
bounds, directories and changed COW tiles are admitted; cancellation or failure drops all unpublished
changes. The input is bounded to 8192 points and preflight caps aggregate bounding
box visits at 64 Mi cells, counting overlaps. Radius is 0.000001–256 mm; plane
thickness is 0.000001–512 mm. Coordinates and source extents relative to the source
origin are limited to one billion millimetres. Inverse-derived pruning boxes use
a full-cell margin and checked affine residual/arithmetic bounds; unsafe pruning
retains a full bounded scan. These backend proposals are not native 3D tools or
saved/sharable scenes and grant no pick, edit or disclosure authority.

`FaceErase` prepares a separate, worker-only erase proposal from an explicit
manual-mask face selection and physical sphere sweep. The selection carries the
exact mask key and revision; every descriptor is checked against original valid
selected membership and a known nonselected neighbour or true grid boundary.
Missing neighbours leave open boundaries. Only selected source-cell centres
inside the sweep and clip can change, and duplicate faces change a cell once.
Erasing a front cell never discovers another layer within that proposal.
The immutable preview retains its original faces, path, settings and base Arc;
Apply rechecks the exact base/path/settings and consumes the prepared result.
No-op proposals retain the original map identity. Up to 65,536 input faces and
8192 path points are admitted, with at most 64 Mi face/segment tests, plus charged
path/face metadata, temporary segment bounds and changed COW storage. Cancellation
or failure discards all unpublished changes. This core geometry operation cannot
prove optical visibility: a view tool must independently retain and revalidate
the originating completed image, visibility proofs and live edit/input epoch.
It does not enable a native surface brush or supplied-mask editing.

RAM-only object/revision keys distinguish replacement maps and no-op operations.
Tile differences bind an exact previous object/revision, including undo/redo;
these keys contain no source tags and are not serialized. Native history retains
up to 16 edits, with shared tiles charged once across all live revisions. A real
new edit clears redo; a no-op preserves history. The exported revision marker
retains keys without keeping a full map alive, and unsaved state follows that
exact revision. GPU label uploads use these tile differences. Native overlay
preparation still rebuilds its layers.

Header-only grid checks precede dense preparation/import.
Native previews bind exact map/source, operation parameters and an invalidation
epoch; source-plane picking uses the inverse display transform. The worker also
prepares the proposed immutable map. Explicit Apply only swaps that map and adds
one undo checkpoint, so no voxel scan occurs on the UI thread. Replaced/cancelled
results cannot publish after input invalidation. Current counts/exports remain
applied-map operations and ordinary agent snapshot contracts do not include labels.

The core read-only segment model retains independent samples and source-plane
coverage for each numeric segment ID. Binary and label-map memberships stay
bit-packed. Fractional masks retain original bytes only for supplied planes, with
the declared maximum and probability/occupancy meaning. Per-plane addresses avoid
a dense byte grid per ID. The categorical view used by counts, overlays and mask
rays applies the fixed half-maximum threshold without replacing those raw values.
Omitted planes stay distinct from supplied
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
binary, fractional or label-map pixels, matching reference/plane geometry and a
single source per mask frame are required; no resampling or reference paths are
followed. Multiframe
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
FRACTIONAL preflight admits retained source-plane bytes and addresses before
reading the payload. Staging can copy one selected raw row while returning known
coverage separately; unknown planes fill staging with zero but remain unknown.
The 128 MiB sample limit covers packed binary masks or retained fractional bytes;
coverage/metadata and temporary overlap checks also need shared admission.
LABELMAP frames name no segment: each listed plane
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

The core read-only mask view selects one nonzero manual ID or one declared supplied
segment without copying payloads. Object keys and immutable revision keys are
opaque RAM-only values; supplied segments and manual revisions use the same key
contract. Membership, known coverage and source validity are distinct conditions.
Unknown planes remain distinguishable from known zero masks, and overlapping IDs
are queried independently.

An allocation-free CPU ray reference traverses positive-length nearest-cell
intervals in physical millimetres through the full inverse source affine. Its
categorical domain is [-0.5, dimension-0.5), separate from the scalar renderer's
voxel-centre interpolation domain. Parallel upper faces are outside; internal
half ties choose the larger cell, and tied edge/corner crossings advance all axes
without inventing zero-length cells. Physical clips preserve partial cell lengths.
The first eligible hit ignores unknown coverage and missing source samples.
A caller must retain and revalidate the exact source, visible mask key/revision
and completed scene before using a hit for an edit. GPU traversal is checked
against this reference. Native picking remains separate.

`MaskOwner` retains an immutable manual or supplied map for local worker requests;
it adds no persistence or disclosure authorization. The CPU `CellSurface` backend
derives quad descriptors at exact half-cell boundaries for one selected ID.
Positive cells contribute faces next to known, source-valid negative cells.
Unknown coverage and missing source samples leave open interfaces. Outer grid
truncation faces are marked separately. IDs and overlapping supplied maps stay
separate; fractional inputs use the existing half-maximum categorical view.

Descriptors retain global integer corner keys, with f64 physical geometry and
inverse-transpose normals available under shear and reflected axes. This is an
independent cell-boundary reference, without a smooth interpolant or topology
repair. Diagonal contacts can be non-manifold. Open unknown boundaries do not
claim equivalence to a closed-volume first-hit visualization. Source-cell counts
remain authoritative. Surfaces do not authorize picks or edits.

`surface::field` separately defines source-bound inputs for a derived trilinear
mask field. Lattice nodes are source-cell centres: manual/binary membership is
-1/+1, while fractional samples retain their exact `2*value-maximum` amplitudes.
The zero level is half maximum. Original encoding, categorical counts and source
storage stay unchanged. Unknown supplied coverage and source validity remain
separate; a cell needs all eight known valid corners. Finite-grid bounds are not
exterior zero padding. Immutable owners retain source/map/ID/revision identity
without a dense field copy.

Canonical global edge/face keys and reduced rational edge positions agree across
cell owners. Bounded integer products select strict bilinear contour pairings;
exact saddles and zero-corner/edge contacts are explicit outcomes, without an
epsilon or an invented empty face. Parallel-section determinant coefficients
and their internal extrema remain exact. These extrema are candidates, not a
complete body-diagonal connectivity test. No complete extractor, singularity
regularization, manifold guarantee or native smooth geometry follows from this
field layer. The existing exact cell-face view remains independent.

`MaskField::affine_cell` recognizes crossing cells whose eight values are exactly
an affine field. It intersects the plane and source-centre cube with exact
rational edge coordinates, retains the resulting convex polygon, and orients
its fan and inverse-transpose physical normal outward from the selected phase.
Unknown or invalid corners, no-interior cases and non-affine fields remain
distinct. Adjacent same-source/revision affine cells can match exact empty,
point or segment traces on their shared face. Other neighbors remain unresolved;
this does not establish grid closure, a transition to voxel faces, GPU precision,
native display or edit authority.

Projection-critical analysis solves the section determinant's bounded integer
quadratic, retaining irrational roots as a defining polynomial and ordered branch.
Signs of linear forms at those roots use integer comparisons, including closed-cell
bounds, exact boundary contacts and the remaining derivative. Within that cell's
trilinear extension, a nonzero derivative identifies a regular projected saddle;
zero identifies a singular point. Boundary points still need the incident cells'
connections checked. Denominator-zero roots are checked separately: a complete
zero section is explicit, and extraneous roots produce no isolated point. An
identically zero determinant remains a distinct degenerate result, which can
represent a singular family, a regular plane or no zero set. No epsilon resolves
these cases. Storage stays fixed at two candidates per axis, without a case table.
Approximate coordinates cannot choose connectivity; exact rational coordinates
remain available where possible. Irrational coordinate forms use their exact
conjugate numerator when direct subtraction would cancel. Contour connections and
procedural triangulation
are still required before this analysis can supply a mesh.

`face_interior_limit` computes the contour connections immediately inside one
cell as parallel sections approach its boundary. Exact leading integer coefficients
of the corner values and section determinant select this one-sided limit, without
sampling at a displaced plane. Adjacent cells can have different connection
pairings at the same shared face cross. These pairings leave the original face
geometry intact; persistent zero edges and saddle families remain explicit
critical outcomes. They do not independently establish the joined surface's
topology or manifoldness.

`critical_graph` derives finite axis-aligned zero lines directly from the two
bilinear equations obtained by separating the line's coordinate. A line can
cross the cell even when its critical intersection is outside it. Exact
fractional-linear coordinates merge finite critical points and boundary
endpoints, then order the segments on each line. Common-radical comparisons use
checked i128 coefficients and fixed 256-bit products; expressions have no
representation-based coordinate equality. No floating distance chooses a join.
The graph uses fixed storage for at most 18 nodes, six lines and 102 segments,
with bounded cancellation points and no per-cell heap allocation. Identically
vanishing eliminations and whole zero sections remain explicit diagnostics.
Strict same-sign fields have no interior zero set; an empty graph does not
establish absent mask membership or resolve finite-grid truncation. Complete
face contours, singular families and triangulation remain separate work. Future
mesh owners must retain source/map/ID/revision and admit aggregate geometry;
unit-cell graphs grant no edit or disclosure authority.

`CriticalGraph::anchor_star` orders retained anchor half-edges at an original
node, carrying their edge/neighbor IDs, signed axes and boundary clipping. Exact
gradient signs select the outward cyclic order at regular nodes. A zero partial
derivative at a zero node proves an entire axial line; every in-cell direction
must be present, with no duplicate ray. Original power coefficients are bounded
by 2040, so the existing bilinear sign predicate evaluates these derivatives.

An isolated singularity with nonzero mixed derivatives retains two separate
three-ray rings at the same node. In translated coordinates its field is
`e*xy+f*xz+g*yz+h*xyz`. The local map `X=x/(1+h*x/g)` reduces it to the quadratic
cone with positive scale and Jacobian near the node, fixing the anchor rays.
Exact mixed-derivative signs then determine the two rings and their outward
orientation without moving the contact. Family diagnostics remain explicitly
unresolved. Fixed storage and bounded cancellation preserve the original graph
on failure. These are anchor orders, with outside rays clipped. Boundary bits
do not determine which intervening surface sectors lie inside the cell; one-sided
face limits and complete cell-face contours remain necessary. Surface patch
connectivity and finite-grid policy are separate requirements.

`RegularTangents` compares signed axial directions with regular face-contour
directions at one regular zero point. With outward normal n, these are `e_i`
(requiring n_i=0) and `n cross e_j` (requiring a nonzero face gradient). Dot and
cross-dot-n signs reduce to exact derivative signs after removing positive norm
factors. An arbitrary valid reference selects a counterclockwise order without
angles, approximate normals or a width increase. Invalid and vanishing directions
are refused. Equal directions remain equal across representations; callers must
prove geometric identity before merging curves or creating distinct half-edges.

Regular anchor stars use this same comparison with the original positive-axis
reference, retaining their previous start and cyclic order. Ascending face-contour
directions account for transverse-axis parity, while cell-boundary directions use
the outward cell-face side. These are constant-size direction operations without
allocation. They do not determine curvature, inside-cell sectors, complete
source-cell contour ownership or a surface patch triangulation.

`MaskField::cell_graph` assembles a source-owned cell contour/anchor graph. It
retains the original field owner, cell samples, phase and factorization. Unavailable
samples, no strict-interior interface, unresolved families and finite graphs have
distinct states. Selected boundary strata and whole-section diagnostics remain
available; these states do not close the finite grid or declare an empty mask.

Finite assembly processes the six canonical source faces one at a time. Exact
coordinate comparisons merge their points with original critical nodes. Shared
face expressions own boundary coordinates, and every boundary point must occur
in each containing face. All union points split their original anchor lines;
coincident axial geometry merges while preserving face/anchor ownership. Bounds
are 552 raw nodes, 2,238 edges and 4,476 half-edges, with checked u16 identities.

Open axial segments use exact affine derivative signs and intersect the boundary
conditions from every containing face. An isolated zero edge therefore remains
a lower-dimensional stratum, with no fabricated surface half-edges. Regular mixed
tangent rotations and ordinary-double rings connect eligible half-edges into
oriented cycles. Isolated points and inactive edges remain in the constraint graph.
These cycles do not yet supply a general patch parameterization or triangle mesh.

`SourceCellGraph::projection_chart` retains a finite source-cell graph and builds
an exact arrangement of its two endpoint fields on the selected projection axis.
Each original graph node is inserted in the chart, while distinct original heights
remain in the retained graph even when their projections coincide. Optional exact
constraint points share the 80-entry insertion capacity with original graph nodes
and let a boundary owner request additional splits; they do not by themselves
coordinate a whole seam. A rational chart point can be lifted as an empty,
unique-height or whole zero fiber; a unique result retains the source-axis
derivative sign. Unrepresentable or irrational lifts fail
explicitly. `CellProjectionChart::project_edges` retains every original graph edge
with its original node IDs. Projection-axis edges retain both distinct heights at
one collapsed chart point; other edges become straight projected intervals or
end-face curves. Each end-face curve is linked in source-node order to the chart
edges that cover it, including intermediate chart splits. Ambiguous branches,
missing links, capacity failures and cancellation publish no directory. Input and
retained typed storage remain separately admitted. Straight projected intervals
are retained as independent constraints. `ProjectedCellEdges::contacts` enumerates
exact contacts among those intervals or collapsed points and with both end-face
zero sets. A contact retains both source-edge IDs or its source-edge/end-face pair;
coincident intervals and whole-zero portions retain both endpoints. Separate
owners are not welded merely because their projections coincide. The two-pass
worker bounds output to 262,144 contacts and 32 MiB of retained typed storage,
admits it before allocation, and publishes no partial directory on cancellation
or unsupported arithmetic. `ProjectedContacts::split` rebuilds the chart with
all contact endpoints and preserves its prior chart nodes, within the same
80-entry insertion bound. It splits each straight source constraint at every chart
node and gives coincident pieces one shared segment ID. Per-source directed paths
retain ownership, collapsed paths retain their original height endpoints, and a
coincident axis-aligned chart edge is linked without merging curved arcs solely
by endpoints. Its input, rebuilt chart and edge directory retain separate charges;
scratch and final typed output are admitted with bounded sizes.

`SplitProjectedConstraints::planar_graph` joins the chart's exact curve edges
with the split straight cuts, using the existing exact chart tangent order and
cardinal source-ray directions to construct a rotation system. Twin and next
half-edges form closed boundary walks, each retaining its original chart region
and field signs; the exterior is explicit. Separate curves remain separate even
when their endpoints coincide. The graph requires a connected edge-bearing
component: disconnected inputs are refused rather than assigning multi-boundary
faces incorrectly. Isolated chart constraints remain in the retained input.
Output is limited to 8,192 edges and 16 MiB of separately admitted typed storage;
scratch is admitted as well. Cancellation or inconsistent connectivity publishes
no partial graph. `ProjectedPlanarGraph::refine` reuses the two-field curve
refiner's coordinated slice partition and inserts the same slices into horizontal
source cuts; vertical cuts retain their original endpoints. For each added slice
point, an unexpected field or square-boundary contact is refused; the earlier
contact and split stages supply the complete intersection constraints. Each original
planar edge has one retained path and each region traverses those paths in its
original half-edge direction, including repeated visits at contacts or bridges.
Curve and additional path storage are admitted separately, and limits never lower
requested detail. `RefinedPlanarGraph::triangulate` groups all retained vertices by
exact shared slice, orders the nonvertical boundary pieces within each slab and
uses their directed face ownership to triangulate each strip. Slice vertices,
including isolated constraints, appear on both incident slab boundaries. The
result retains its source graph and planar region IDs. Additional vertical slice
splits still need propagation to neighboring owners; this planar mesh does not
certify a 3D surface interior or shared seam and has no rendering or source-edit
authority. Output, scratch, comparisons and cancellation are bounded; inconsistent
slice order or region incidence produces no partial mesh.

`CellProjectionChart::fiber_bounds` classifies the source field at exact chart
coordinates, including unrelated quadratic coordinates, by the exact signs on its
two end faces. An empty fiber and a whole zero fiber remain distinct; a unique
fiber is enclosed by inclusive dyadic height bounds at up to 16 bits, or an exact
height when found. The bounded bilinear sign predicate evaluates each midpoint
without floating-point decisions and can refuse unsupported arithmetic. A planar
mesh vertex can query the chart retained through its source graph. These vertex
bounds do not resolve whole fibers, certify triangle interiors or coordinate
neighboring seams. Exact local coordinate endpoints retain the degenerate interval
0 or 1, so source-face vertices are not displaced into a cell by midpoint selection.
`TriangulatedPlanarGraph::enclose_surface` retains that mesh and computes inclusive
3D local-coordinate boxes for its uniquely defined vertices, using exact coordinate
comparisons and fiber signs. Empty and whole fibers stay explicit. Each input
triangle keeps a same-index disposition based on its source region and vertex
fibers; a candidate is not an interior certificate. Output is separately admitted,
bounded and cancellable. Curved boundary error, whole-fiber interpretation,
neighboring seams and rendering remain separate obligations.
`EnclosedSurfaceGraph::facet_certificate` chooses a deterministic dyadic midpoint
for each unique vertex. Exact quadratic Bernstein coefficients of the two source
end faces prove endpoint signs and a strictly signed axial derivative across the
projected triangle. Exact cubic Bernstein coefficients bound the source-field
residual on its 3D planar representative; their ratio gives a conservative axial
error to the unique zero on each projected fiber. Noncandidate, nonpositive
projection, and failed proof cases remain explicit. The certificate is local to
that projected triangle: curved boundary topology, shared seams and adaptive
refinement remain separate obligations. `facet_physical_error_upper_mm` and
`ConformingSurfaceMesh::triangle_physical_error_upper_mm` convert a numerical
certificate using the retained source's affine basis and outward-rounded L1
axis length. This conservative separation bound includes anisotropy and shear;
it excludes world-position rounding, GPU quantization, missing surface regions
and boundary or seam errors. Unproved certificates have no physical bound.
`EnclosedSurfaceGraph::refine_facet` bisects a candidate on its longest projected
edge, samples the original field for each new midpoint and rechecks every child
against the requested axial error tolerance. It keeps unresolved leaves when
depth or triangle limits are reached, projection degenerates, or a midpoint is
empty, whole, or has the opposite axial orientation. Output and fixed scratch
are separately admitted and cancellation publishes no partial result. This is a
single-facet operation; neighboring facets and source cells do not yet share an
adaptive split union, so its output is not a conforming or renderable mesh.
`EnclosedSurfaceGraph::refine_conforming` coordinates candidate facets within
one source-cell chart. Source vertex IDs and explicit midpoint parent IDs identify
edges; rounded coordinates are not welded. Splitting a shared candidate edge
updates both owners before either child is recertified. Limits, work exhaustion,
singular midpoints and incompatible neighbors remain unresolved rather than
loosening tolerance. Output and scratch are admitted separately. Noncandidate
facets remain in the retained source. A proposed split on a projection end-face
edge is allowed only if exact field values at both endpoints and the midpoint
prove its quadratic chord lies on the face zero contour; otherwise it remains
unresolved until source-contour-aware refinement. The separate
`refine_conforming_physical` entry accepts an explicit millimetre target and
marks a candidate within tolerance only when its conservative physical bound
meets that target. Face application and either promotion form preserve each
physical target while axial meshes continue to use the requested axial settings.
The target does not certify unproved or unrepresented geometry. Cross-cell seams,
curved-boundary topology, whole-surface physical accuracy and native rendering
remain separate work.
`ConformingSurfaceMesh::shared_face_partition` compares two neighboring meshes
only when their source, mask key, revision, face and projection axis agree. It
unions exact transverse contour coordinates, verifies unique fibers and compatible
representatives, and reports existing vertices, requested splits and absent points
on each side. Open intervals also retain per-side candidate-edge coverage, so
matching endpoints cannot conceal a gap. Distinct exact coordinates that collapse
or reorder on the output grid are refused. Output and scratch are bounded and
admitted separately. This directory does not apply splits, prove full face coverage,
or establish a cross-cell seam; end-face and cross-projection cases remain open.
`SharedFacePartition::apply` groups requests by original boundary edge, applies
all ordered split points to private copies of both candidate meshes, and
recertifies every resulting triangle against each side's requested axial
error tolerance. Source triangle/facet identities and exact partition point
references remain available; open intervals without candidate coverage remain
absent. Both outputs are admitted and published atomically, with separately
bounded scratch and cancellation. This joins the represented candidate edges
for one compatible transverse face only: singular/noncandidate regions, other
faces, curved end-face topology, physical error and whole-grid seams remain open.
`AppliedFacePair::promote_cells` atomically creates two reusable cell meshes with
exact face-split coordinates, source face keys, original facet identity and the
prior application retained. Later compatible face partitions use this provenance
rather than rounded point equality; a split crossing onto another face must be an
exact corner. The retained dependency depth includes both input histories and is
bounded. Storage and cancellation are separately admitted; composing individual
face exchanges does not itself establish a whole-grid fixed point or surface.
`AppliedFacePair::promote_cells_compacted` instead materializes the same vertex
parents, exact face coordinates, source facets and current certificates without
retaining the prior pair or its audit history. A caller that needs that history
must retain the pair separately. Both results are admitted atomically, and their
subsequent face queries use the retained source graph and exact coordinates.
`CellSeamBatch::coordinate` accepts an explicit bounded set of source cells and
revisits their shared faces whenever either cell generation changes. It retains
uncovered intervals, unsupported projections, exhausted limits and faces that
were not rechecked as distinct results. It uses compacted promotion; only split
sides replace their prior meshes and certificates. Each face operation and the
complete returned batch remain private until successful publication; cancellation
or memory refusal returns no partial batch. Bounded rounds, visits, updates and
other resource limits can stop coordination before convergence. Matching
represented candidate edges does not certify noncandidate strata, unlisted
neighbors or a complete surface.
`MaskField::candidate_grid` connects source-cell graph construction, charting,
planar constraints, triangulation, enclosure, physical refinement and face
coordination over an explicit box of at most 256 cells. Each cell retains its
source origin and either a candidate mesh index, a nonfinite graph disposition,
or the stage and reason that prevented construction. Face coordination reports
its own unresolved results; a coordination limit retains the uncoordinated
candidates. Cancellation or memory refusal publishes no partial grid. The source
and every shared intermediate retain their own memory admission, including the
aligned shared-reference header, so a cloned mesh can outlive the grid. A box
with no produced candidates does not imply an empty or complete surface.
`candidate_grid_reusing` retains each independent cell result before face
coordination. With matching source, mask identity, revision, projection, limits
and memory budget, it reuses overlapping cells and coordinates the new box from
independent inputs. Across revisions it reuses only unavailable, no-interior and
family dispositions when all eight source-cell samples match, including unknown
and invalid samples. Those dispositions are determined before face and edge
neighborhood collection. An independent candidate mesh can be reused across
revisions only when every sample in its clipped one-cell face/edge neighborhood
matches; its retained source chain is rebound to the current revision before
all faces in the requested box are coordinated again. Unresolved stages rebuild.
`CandidateAssembly` collects disjoint boxes with matching source revision and
construction settings, orders their cells deterministically and re-coordinates
their independent candidates together. Its shared-face reports include faces
between boxes; overlap, stale revisions and mismatched inputs are refused. The
assembly is limited to 256 supplied cells and retains explicit uncovered or
unresolved states. It does not establish a complete surface or continuity with
unlisted cells.

`SourceCellGraph::refine_edges` retains that immutable graph and supplies one
canonical path per original edge. All original nodes remain the vertex prefix,
including isolated and inactive constraints. Added exact points carry their
original edge ID. Incident regions reuse the same path in their half-edge
direction; boundary walks retain repeated visits at contacts and cuts. Axial
edges stay straight. Face arcs reuse the bilinear chord certificate with endpoints
ordered by ascending transverse u, then restore the original edge direction.
Canonical shared-face inputs therefore produce identical exact points and
floating approximations from either cell at the same requested precision.

The path bound is max-coordinate Hausdorff error `2^-precision_bits` in the unit
cell, for bits 0..24. It does not bound a surface interior, physical-affine error
or GPU quantization. Complete count/admit/emit passes separately charge bounded
scratch and up to 65,536 vertices / 16 MiB of additional retained typed storage;
the input graph keeps its own reservation. Limits and cancellation never lower
precision or replace a previous result. Unavailable, family and no-interface
inputs are refused rather than converted into empty refined geometry. These
paths are inputs to patch triangulation, not a completed smooth surface or a
whole-grid adaptive seam directory.

Metadata, fixed preparation, bounded heap scratch and completed typed output use
shared volume admission. Output limits can be lowered; the complete retained byte
count excludes shared source/mask buffers, scratch and allocator/process overhead.
Cancellation, incomplete unions, inconsistent incidence and pressure publish no
partial finite graph and preserve previous owners. Source chunks, family patches,
certified 3D approximation, complete adaptive seam unions and physical/GPU/native
publication remain separate requirements.

`ExactCoordinate::compare` also orders coordinates from unrelated quadratic
fields in neighboring cells. Compatible fields retain the common-radical fast
path. Otherwise, rationalization gives a positive denominator and the difference
has two radicals. Sign-aware isolation reduces that comparison to one radical;
fixed checked 1024-bit signed arithmetic preserves the squared products without
truncation or allocation. Floating coordinates never select equality or order.
This comparison is a prerequisite for shared insertion unions, not an assembled
face contour or a seam-free mesh by itself.

Coordinate forms admit coefficients through 2^22 for a checked graph transfer.
Root coefficients remain bounded by 2^20. Rationalization gives magnitudes below
2^89 for the numerator constant and positive denominator and below 2^67 for the
radical coefficient. Cross-field ordering fits 720 bits. Same-field arithmetic
retains checked i128 coefficients and the existing 256-bit squared comparison,
including mixed retained i64/u64 rational coordinates. Polynomial coefficient
limits remain separate from coordinate-form limits.

`Bilinear::zero_slice` fixes u or v and returns an exact zero point, an entire
zero line or no intersection within the closed square. Rational evaluation uses
checked retained ratios; out-of-square zeros need no materialized ratio. An
irrational input must retain forms bounded by 2040. One graph transfer produces
forms of magnitude at most 3121200, within 2^22; an oversized subsequent input is
refused. This supplies shared slice coordinates for future coordinated curve
refinement without rounding intersections or replacing whole lines by points.
It does not itself construct a shared adaptive partition or mesh.

`Bilinear::arrangement` joins its exact boundary zeros and a bounded union of
inserted points. It retains isolated zero corners, entire zero edges, singular
crosses and whole zero faces. Hyperbola branches are separated by their exact
asymptote side and ordered monotonically; factorized crosses split at their
actual centre. Segments follow the zero curve between endpoints, without
claiming that a curved branch equals its straight chord. Canonical numerical
node ordering is independent of insertion order. Directed segments keep F>0 on
the right in ascending face coordinates; cell-face orientation accounts for
the outward side and axis parity.
After exact equality is proved, duplicate points retain a deterministic supplied
expression per coordinate; rational values reduce to one identity expression.
This also keeps their later floating approximations independent of which cell
was visited first. Representation ordering never decides numerical equality.

Every insertion must satisfy the exact bilinear equation. Its sign evaluation
handles products of coordinates from unrelated quadratic fields using fixed
checked arithmetic; the final comparison fits 768 bits. Zero contours admit 80
raw insertions and produce at most 85 nodes and 85 segments, without per-face
heap allocation. The common fixed arena also holds four extra domain corners
for plane subdivision. Whole zero faces remain two-dimensional, with retained
points and no invented contour.
Cancellation or a capacity/input failure returns no partial arrangement.
`CriticalGraph::face_arrangement` validates matching opposite face samples and
collects both incident graphs' points. The owning extractor must still prove
source adjacency and supply all insertions from the four cells around entire
zero lattice edges. This local face construction does not settle cell-interior
families, finite-grid closure, chunk ownership, triangulation or mesh admission.

`Bilinear::intersections` decomposes two zero sets into axis factors, irreducible
curves and whole squares. Common factors and curves remain explicit, clipped to
the closed square. Clipping may leave only a single corner of a common curve. Finite events
include crossings, tangencies and singular joins on shared components. Primitive
corner values make geometry independent of field sign, scale and argument order.
For two different irreducible curves, eliminating one coordinate gives a quadratic
whose coefficients have magnitude at most 1,040,400. Exact roots, nonzero graph
denominators and both field signs validate the retained points. At a regular
intersection the eliminant derivative equals the gradient cross product; repeated
roots mark tangencies, while a zero gradient marks a singular join. Storage is
fixed at four finite points and two common components, with cancellation and no
partial result. This supplies intersection events, not two-field region loops.

`MaskField::edge_partition` reads a global source-lattice edge, its four incident
cells and four normal half-face stencils. For an entire zero edge, a known cell
has the inward expansion `F(s,r,t)=B(s)r+C(s)t+D(s)rt`. The four normal and four
mixed-normal linear coefficients supply at most eight finite roots; endpoints
give at most ten partition points. Normal half-faces are read independently, so
their roots survive unavailable diagonal samples at interior or boundary edges.
Identically zero coefficients remain known zero forms, separate from unavailable
stencils. Source invalidity, unknown mask coverage and outside cells remain
distinct. This finite coefficient partition does not classify 3D singular families.

`MaskField::source_face` reads both incident cells and all four edge partitions
from the same immutable owner and global face key. Two graphs of at most18nodes
and four partitions of at most10points bound the raw union at76insertions. The
face retains the exact source/map/ID/revision, samples, cell states, graphs and
edge partitions; unresolved eliminations and whole sections stay attached. Known
face samples can yield contours even when off-face samples prevent complete
cell graphs. Unknown face samples leave its contour unavailable, and finite
source boundaries add no exterior samples or caps. Results use fixed storage
and bounded cancellation, with no dense source copy or per-neighborhood heap
allocation. Whole-grid chunk admission, triangulation, interior-family resolution
and GPU/native mesh publication still require separate ownership and validation.

`SourceFace::whole_zero_face` analyses a proved zero face while retaining its
source/map/ID/revision, global key and both incident cell states. Each known cell
has the exact inward expansion `F(s,u,v)=s*G(u,v)`; its opposite source face gives
the bilinear field G without resampling. When both sides are known their exact
intersection events remain attached. Outside and unavailable neighbors stay
distinct; an absent intersection result means incomplete incident stencils.

Pointwise classification away from nonconstant zero curves keeps both selected
neighborhoods interior, or points an interface toward the unselected side. Two
negative inward fields retain a selected zero-thickness sheet rather than filled
volume. A whole-zero inward field selects its complete incident cell under F>=0.
Nonconstant zero fields retain a constraint; unknown or outside neighbors remain
unresolved without a closing cap. This analysis grants no edit authority and
does not replace the required two-field region arrangement or finite-grid policy.

`SourceFace::zero_subdivision` builds that arrangement when both incident cells
are known. It retains the proved zero-face owner and every original face point,
and exposes each region's selected-phase classification. Unknown or outside
neighbors leave the result unavailable, without inventing a cap. Region data
does not grant source-edit authority or settle finite-grid closure.

`SourceFace::zero_mesh` retains the original mask/source/revision, global face key,
known incident cells and the completed two-field triangulation. The complete
geometry includes interior regions for constraint inspection. A separate patch
iterator omits those regions, winds interfaces toward their signed lattice-axis
normal and marks zero-thickness sheets for two-sided rendering. Ascending
transverse coordinates require a winding reversal on axis1. This orientation is
in source-lattice space; physical-affine conversion and GPU quantization remain
separate gates. Neither triangle IDs nor phase labels grant source-edit authority.

Provenance metadata, fixed subdivision preparation and both retained geometry
stages use shared volume admission. The completed byte count excludes shared
source/mask buffers and temporary preparation. Patches add no allocated triangle
copy. Refinement and triangulation keep their own limits; failure or cancellation
releases unpublished work and preserves prior owners. Unproved zero faces and
outside/unavailable incident cells return no mesh before geometry admission,
without a closing cap. An admitted face with only interior regions instead has
a complete mesh and zero drawable patches. Whole-grid connectivity and a shared
boundary-path union across neighboring patches are still required.

`Bilinear::subdivision` retains the complete square boundary alongside one
bilinear zero set. Nonzero boundary points are accepted; contour insertions still
require exact zero membership. Coincident zero/boundary edges merge, singular
crosses split at their centre and whole-zero interior constraints remain isolated
points. Canonical numerical nodes and undirected edges supply directed half-edge
loops with exact region signs and an explicit exterior boundary. Tangent signs
give cyclic ordering for this bilinear square graph; zero-gradient crosses use
their proved axis lines. No floating angle, chord or area selects connectivity.
Bounds are 80 combined raw insertions, 89 nodes, 92 edges, 184 half-edges and four
regions. Cancellation or inconsistent inputs return no partial subdivision.

`Bilinear::joint_subdivision` builds closed regions for two bilinear zero sets,
retaining up to 80 arbitrary exact constraints. Its fixed arena has 98 nodes,
106 edges, 212 half-edges and nine regions. Two separate axis crosses can require
all nine regions. Edge identity includes primitive geometry: different curved
arcs with identical endpoints enclose a region and must remain distinct. Common
curves merge while retaining both original fields' ownership and orientation.

Outgoing rays use sign octants, gradient cross products within one octant and an
exact quadratic curvature comparison at equal-tangent contacts. This orders both
sides of a tangency without floating angles or regularization. A separate bounded
polynomial predicate admits derivative-cross coefficients up to 2^20 within the
existing 1024-bit arithmetic; its widened-coordinate comparison fits 808 bits.
The ordinary polynomial limit remains 2040. Closed
face-left loops carry both strict interior signs, with zero reserved for a whole
zero field. An arc between two common zeros uses its eliminant's quadratic term
to determine the other field's open-arc sign, without an approximate midpoint.

Nonzero interior constraints retain a unique containing region, located using
exact upper/lower graph-side signs on their vertical slice. No point is discarded
or moved to a nearby boundary. Geometry, source ownership and cancellation remain
separate from coordinated curve refinement, triangle approximation and GPU
publication; independent chords could otherwise create false crossings.

`JointSubdivision::refine` supplies coordinated edge paths and one exact u-slice
partition. It begins with every original node's u, including isolated interior
constraints and the original intersection/domain events. An open event slab has
at most four nonvertical graphs: two domain edges and one zero graph per field.
The accepted single-field chord certificate and largest-coordinate dyadic split
are shared by both refiners. Any selected rational split cuts every active graph;
vertical edges retain their ordered original constraints. Curves with equal
heights at both endpoints force an interior split, preserving a lens even at
coarse detail. Exact endpoint order then prevents chord crossings or collapsed
regions within each slab. At an isolated constraint's slice, its polygonal
boundaries remain at their exact curve positions.

The completed result retains the original graph/node prefix, one canonical path
per edge and the common slice coordinates. Limits remain 65,536 vertices, 16 MiB
of retained typed storage and precision bits 0..24. Fixed scratch, the bounded cut
directory and completed output are admitted separately before allocation. A
128-level work stack covers the two active curves' selected bisections and a
forced lens split. Checked slice-transfer failures, limits and cancellation
publish no partial paths and never lower precision. Physical/GPU conversion
and general trilinear connectivity remain separate work.
The owning extractor must share these completed boundary paths with incident
patches; this local face result does not establish a conforming whole-grid mesh.

`PlaneSubdivision::refine` converts its edges to certified adaptive polylines.
Original exact nodes remain the vertex prefix; distinct points are never welded
by rounded coordinates. Curved bilinear arcs use exact 40-bit dyadic enclosures
and rational curve points. The chord residual is bounded by `|D*du*dv|/4`;
lower bounds on the transverse derivatives turn it into a coordinate-error
certificate. A small monotone bounding box is a separate sufficient certificate.
Straight edges need no added vertices, and nearly flat arcs can remain sparse.
The bound is max-coordinate Hausdorff error `2^-precision_bits` in the unit
square, for bits 0..24. Physical-affine and GPU quantization error are separate.

The worker counts complete prospective vertices/indices before reserving output
and allocating. Shared volume admission charges fixed scratch and retained typed
storage separately, including a fixed-size subdivision copy. Limits are 65,536
vertices and 16 MiB per result; callers can lower them. Errors or cancellation
discard unpublished work and release reservations without changing the input or
reducing requested precision. Canonical edge paths are deterministic for the
same complete insertion union and detail; owning geometry must share the
completed path across incident patches. Refinement alone supplies no triangles,
GPU mesh, source-edit authority or whole-grid geometry admission policy.

`RefinedSubdivision::triangulate` and `RefinedJointSubdivision::triangulate` retain
an `Arc` to their exact completed input and use one private triangulation kernel.
At each regular u, a bilinear sign restricts v to one interval. Complete events
and coordinated chords preserve each region's u-monotonicity, including two-field
regions and factored axis boundaries. Merging the two lexicographically ordered
boundary chains and a stack triangulates each region without a quadratic ear
search. Collinear boundary vertices remain constraints. Original half-edge/path
directions and vertex bounds are checked before the ring is consumed.

Isolated interior points split one containing triangle, or both triangles sharing
their containing diagonal. Joint points are bound to their original exact
`interior_region`; sign pairs alone cannot identify disconnected regions. A point
cannot be inserted into another region's triangles. The single-field whole-zero
case retains its original region-zero rule. A mismatched assignment or incomplete
diagonal match discards the unpublished output. The concrete result types retain
their own input owners and every triangle's original region ID.

Turns first use cached 40-bit integer interval bounds. Ambiguous turns clear all
six positive coordinate denominators and evaluate the determinant in up to six
quadratic radical fields. Opposite-sign terms are compared by recursive magnitude
squaring; each level removes one radical, so depth is at most six. Dependent
radicals and exact collinearity need no floating tolerance or factorization.
Checked fixed-width signed integers use derived widths up to 65,536 bits; the
coefficient pool is separately admitted and released. Overflow is an error.
With the widened coordinate forms, a determinant's weighted coefficients are
bounded by `541+22*k` bits for k radical fields. Each elimination updates the
bound to `2*b+k+2`. The dispatch uses 16, 32, 64, 128, 256, 512 and 1024 limbs for
zero through six fields, covering the largest 43,519-bit bound.

The worker counts `n-2` triangles per region and two per isolated interior point
before allocating. Output IDs are counterclockwise and retain region identity;
all shared constraint edges use the same completed vertex path. Scratch and
additional retained triangles are admitted separately from the immutable input.
Limits are 131,072 triangles and 16 MiB of additional retained typed storage;
callers may lower them. Failure or cancellation publishes nothing and preserves
prior owners. These are unit-square face meshes. Whole-grid source-owned assembly,
full trilinear surface connectivity, finite-grid closure and GPU conversion remain
separate gates.

`Trilinear::phase` uses the inclusive F>=0 level, matching retained fractional
half-maximum ties. Nonnegative corners select the complete closed cell; strictly
negative corners select none. Nonpositive corners with zeros select only all-zero
boundary strata, since tensor weights are strictly positive in an open cell or
face. `zero_boundary` records these vertices, edges and faces. Opposite strict
corner signs identify crossing cells. These classifications preserve original
categorical membership; an entire zero cell is selected, not an empty mask or
an internal surface by itself.

`factorization` separates primitive integer axis factors from a constant or
irreducible bilinear/trilinear residual; an identically zero field stays whole.
A multi-affine product cannot share a variable between nonconstant factors,
because degrees add. Reducibility therefore supplies a one-axis factor, found
by an exact rank-one 2x4 corner flattening. Primitive proportional rows have
integer quotients, retaining bounded amplitudes. Reconstructing all eight corners
certifies the full polynomial identity before publication. Axis factors retain
out-of-cell zeros as absent planes and classify exact in-cell coordinate signs
without an epsilon. Residual bilinear axes remain explicit; residual trilinear
fields have no identically vanishing section determinant. Source faces retain
each known incident cell's factorization alongside its original critical graph.
Fixed storage and cancellation keep this analysis separate from patch
intersection/orientation, singular contacts, finite-grid closure and triangulation.

`CellFactorization::plane_patch` builds an in-cell factor plane from its exact
rational position and bilinear cofactor. The plane's nonzero cofactor boundary
points remain domain constraints, while cofactor zero curves divide regions and
their normal signs. Normals point out of F>=0 in lattice coordinates and require
the source inverse transpose for physical conversion. Boundary planes stay
candidates requiring both incident cell phases; detached patches grant no source
or edit authority. Curved arcs still need shared refinement and triangulation,
and general trilinear sheets remain separate patch-construction work.

`InterfaceRay` is a separate f64 membership reference for known cell interfaces.
It walks bounded source-cell intervals without allocating or scanning derived
faces. Only eligible positive cells against known valid negatives or marked grid
truncation can yield a hit. Unknown coverage and missing source samples remain
open; clip/range entries inside a cell create no face. The consumed traversal
returns the original signed ray parameter, physical point, positive cell interval
and opaque map/revision identity. Exact quad-edge/corner contacts are ambiguous;
zero-length tangent cells produce no hit. It uses no epsilon nudges. This geometry
reference alone does not establish current scene or foreground visibility.

Extraction counts the complete prospective output before allocating faces.
Limits are 2,097,152 quad descriptors, 65,536 chunks and 128 MiB of referenced
typed storage per generation; a caller can lower them. Shared volume admission
also bounds concurrent retained generations and transient work. Chunks own
32-cubed source cells, using one bounded 34-cubed halo during counting/emission.
Related manual edits and undo rebuild changed tiles and face-adjacent chunks;
unchanged chunks retain their Arc and allocation charge. An identical revision
reuses its whole surface without a scan/allocation. New objects, sources or IDs
need a full extraction. Cancellation/admission refusal discards unpublished work
and preserves prior generations. The backend has no GPU buffers, clipping caps,
smoothing, desktop display, exports or agent access yet.

`GpuCellSurface` retains that complete CPU surface and the exact shared GPU source.
Each immutable chunk buffer holds a 16-byte integer instance descriptor per quad:
source cell coordinates and explicit side/grid-truncation flags. Segment identity
stays on the parent surface as u16; packing does not reinterpret Rust enum padding
or expand a second vertex/index/normal array. Unchanged geometry reuses its GPU
chunk Arc, including equal faces re-emitted by a CPU halo update. A changed
revision still gets a parent carrying its new CPU map identity. Replacement
objects/IDs upload fresh buffers, and another GPU source/device cannot supply a
base. No old buffer is overwritten or copied.

Allocation, write-only mapped packing and bounded fence waits belong on a worker.
CPU admission covers the directory and chunk owners. GPU target admission rounds
each allocation conservatively to 4 KiB; it is not measured driver memory. Queued
upload staging is bounded by 16 MiB plus the largest changed chunk and fences
between batches. Cancelled/failed targets retire after submitted work completes;
prior buffers remain valid. Buffer-limit or allocation refusals do not publish a
partial model. Integer readback and vertex-input probes check the descriptor
contract. These inputs remain distinct from ordered source composition,
smoothing, physical picking, desktop display and disclosure/export paths.

`CandidateAssembly` also retains each listed cell's affine classification and
exact traces for listed adjacent pairs involving an affine plane. Faces involving
non-affine or unavailable neighbors remain unresolved. Unlisted neighbors and
finite-grid closure are not inferred from absent reports.

For a listed mixed affine/general face, `CandidateAssembly::mixed_face_partition`
can inspect the current candidate mesh against the exact affine trace. The bounded,
source-bound directory identifies proposed split positions, candidate-edge intervals
and whether their facets meet the requested tolerance; absent and uncertified
intervals remain explicit. Unsupported face projections and unavailable cells do
not become coverage. The directory does not alter either surface, establish a seam,
or certify the physical error of a proposed affine split.

`MixedFacePartition::apply` rebuilds the represented candidate facets and a
quantized affine polygon together. It recertifies the candidate triangles,
bounds the affine vertices' physical displacement with outward-rounded source
basis norms, and refuses collapsed, inverted or self-intersecting fans. Exact
coordinates on other source-cell faces stay on those faces. The retained spans
preserve missing coverage and post-application candidate certification. The
result is local CPU geometry, not a complete seam, GPU projection certificate,
native surface or editing authority.
`AppliedMixedFace::promote_candidate_compacted` materializes the recertified
candidate, its existing seam points and the exact coordinates of mixed-face
splits for subsequent compatible face exchanges. It preserves the candidate's
applied error criterion while dropping application history; callers that need the
matching affine result or that history must retain the original application.
`AffineFaceMesh::merge` reconstructs one affine cell from checked applications
on distinct polygon edges and maps each face's partition points into the merged
boundary. Shared candidate corners must agree. The triangulation preserves the
original exact edge topology and bounds physical displacement; duplicate edges,
conflicting corners and insufficient resources remain explicit refusals.
The result retains its face applications but does not certify missing coverage,
coordinate a whole grid or provide GPU or native display.
`MixedCoordination::coordinate` starts from one immutable candidate assembly,
recoordinates its general candidate meshes and visits listed mixed faces within
explicit round, visit, update and memory limits. Candidate generations invalidate
their earlier face applications; stale faces remain marked for rechecking.
The result retains per-face missing and uncertified counts, per-cell affine merge
states and current general-candidate reports. Those reports concern candidate
meshes, not proof that an affine replacement or the whole surface is joined.
Unlisted neighbors, unsupported faces, finite-grid closure, GPU projection and
native editing remain outside this result.

`GpuCandidateSurface` uploads within-tolerance triangles of a bounded candidate
assembly as per-cell immutable triangle descriptors. Exact affine polygons use
the same integer grid only when a conservative physical displacement bound fits
the requested tolerance and rounding preserves strict convexity and topology;
otherwise they remain explicitly unresolved. The retained affine error bound
excludes later f32 projection and rasterization. Each descriptor
stores the source-cell origin, three exact integer subcell numerators and the
eight signed source-field samples; floating-point conversion is a separate
rendering step. Unresolved triangles,
noncandidate cells and assembly face states remain inspectable rather than
silently treated as complete coverage. Unchanged integer geometry and source-field
samples can reuse a GPU chunk across mask revisions while the parent retains the current assembly.
`GpuCandidateSurface::upload_coordinated` instead retains the mixed coordination
snapshot, uploads its current candidate meshes and checked affine polygons, and
withholds listed planar cells whose mixed-face geometry could not be assembled.
Its whole-surface reuse requires the same coordination snapshot; equal immutable
chunks may be reused after their geometry and field samples match. Face gaps,
uncertified regions, general-face state, f32 projection and native rendering
remain separate from the uploaded partial input.
Triangle and byte limits, source identity, cancellation, admitted GPU buffers
and bounded upload staging are checked before publishing.

`GpuCandidateProjector` projects represented candidates using source-centred
physical camera coordinates. A guarded screen rectangle identifies candidate
pixels; each fragment intersects the corresponding source-cell triangle before
writing its source-cell ID, physical depth and outward trilinear-field normal.
Shared grid vertices are carried across cell boundaries before f32 conversion;
the intersection projects along the dominant ray axis and evaluates shared
edges in a consistent endpoint order. These measures do not certify raster or
intersection accuracy across devices, or establish complete surface coverage.
The normal uses the inverse-transpose affine; a zero alpha denotes an unavailable
normal. Views whose conservative screen
coordinate error exceeds half the guard are refused. A worker counts conservative
rectangle pixel work before allocation and divides draws into bounded fenced batches;
views exceeding the frame work limit are refused. The reported vertex reconstruction
bound excludes projection, rasterization, intersection, clipping, source-surface
error and world-origin rounding. The image retains the incomplete assembly and
does not itself perform source composition or establish edit authority.

`CandidateOpaqueRenderer` composes represented candidate hits with source
material in physical depth order, using bounded fenced worker rows. It checks the
source, camera, navigation and clip identities and refuses invalid or required
missing normals. The retained assembly describes incomplete coverage, so an
absent hit does not establish that a surface is absent. This output is a partial
preview without a complete fallback, authoritative picking or native display.
Its optional source/material-bound range classification skips only certified
transparent sample runs, and the completed image retains the classification
actually used. A material change requires a new classification; no transparent
tiles uses the ordinary path. This acceleration does not resolve surface coverage.

`GpuSurfaceProjector` produces a separate immutable `SurfaceImage` for one exact
GPU source, mask revision, numeric ID, camera/display state, clip and requested
size. It renders known cell interfaces two-sided, leaving unknown boundaries
open. Physical clipping discards fragments and creates no cut caps. This differs
from the first eligible cell of a closed-volume mask ray; the two image types
cannot substitute for each other. Outputs contain integer cell/ID/side/grid flags,
signed physical depth and outward float16 normals. They include no source opacity
and do not establish current scene visibility or authorize picks, edits or sharing.

Projection works around the source centre in voxel displacement coordinates,
preserving reflected/sheared bases without subtracting large world offsets in
f32. Rays are moved along their direction onto that centre plane in f64 before
upload, preserving the same lines even with a distant camera target plane.
Projection and composition use the same integer-centred pixel offsets.
The camera remains physical and orthographic. A guarded screen rectangle
covers each quad; its fragment shader intersects and bounds the actual cell face.
This avoids fixed-point triangle edge omissions and interpolated depth drift.
Physical and hardware depth come from that ray-plane intersection. Signed depth
is measured from the plane through the source centre along the retained viewing
direction, rather than from the camera's target plane. Coincident edge contacts
remain raster/f32 choices, not an authoritative f64 pick. A conservative ratio
of full depth range to smallest source cell-plane spacing must be at most 2^20;
non-finite, collapsed projection/clip parameters and unsupported limits refuse
the request instead of reducing detail.

Declared camera angles at exact multiples of 90 degrees use exact sine/cosine
axes. Small trigonometric residuals could otherwise move a ray off a valid source
interpolation node beside missing data. Nearby oblique angles retain their actual
trigonometric values; there is no tolerance-based snapping of measured geometry.

The worker renders the exact requested dimensions within 1..2048 per side and
the device limit. It submits at most 131,072 face instances before a cancellable
fence; only the first batch clears attachments. Cancellation or failure never
returns a partial image or changes an older one. Three retained attachments admit
28 bytes per pixel plus an allowance; temporary depth admits four more. Shared
CPU metadata and GPU retirement keep old generations charged while referenced or
submitted. This raster path needs three render targets and a uniform buffer,
without compute or storage buffers. Smooth interpolation, authoritative picking
and native presentation remain separate work.

`OpaqueSurfaceRenderer` combines one completed `SurfaceImage` with the exact shared
scalar source, original camera/navigation/clip, optical material and independent
source/surface lighting. Source integration stops at the known opaque interface;
foreground radiance and attenuation remain in front of it. A real backface visible
through an open unknown boundary is not moved to a fabricated cap. Scalar samples
stay within their centre domain, leaving the outer half-cell space empty. Surface
headlight uses the inverse-transpose face normal with two-sided diffuse response;
it does not need another normal texture binding. Linear float16 output is encoded
once by the shared display pipeline.

The compositor rebases rays onto the retained source-centre depth plane in f64;
it does not subtract a large camera offset from depth in f32 or substitute the
closed-mask image contract. A bounded GPU status check validates IDs/side/grid
flags, cell/depth arithmetic and final radiance before publishing. It is an
arithmetic consistency check, not exact membership or current pick authority.
Pending row sequences retain the complete scene and require identical owners and
view/material/light settings. Two slots, at most 256 batches, explicit cancellation,
CPU identity admission and GPU output/retirement admission bound retained work.
Only complete checked images return. This requires compute and a status buffer;
devices without them can still use the separate surface projection backend.

For sources with missing samples, this compositor splits integration at actual
source interpolation-cell boundaries before applying optical steps. Validity is
constant within each open interpolation cell, so a midpoint cannot assign an
entire coarse step across a missing-data boundary. Traversal advances all tied
axes, is bounded by the sum of source dimensions plus three and refuses unresolved
arithmetic rather than publishing a partial image. Fully valid sources retain the
ordinary material integrator. The existing closed-mask and source-only material
paths retain their original stepped validity sampling. Native surfaces, multiple
regions/transparency and export/sharing remain separate.

The optional accelerated compositor uses the same source-bound material tile
classification as the scalar renderer. It checks the exact GPU source and
material values before allocating a pending frame. Completed accelerated images
retain the classification they actually used, so dropping a renderer cannot
retire data needed by a later probe. Classification is independent of the mask
revision and camera; those remain bound by the complete scene. Zero transparent
tiles select the existing pipeline and retain no acceleration owner in the image.
The missing-source interpolation-cell splitter remains unchanged; only certified
zero optical steps within its intervals may be skipped.

`GpuSurfacePicker` is an optional local worker pipeline for one pixel of a complete
`OpaqueSurfaceImage`. It reuses the same optical pixel function at the original
full viewport coordinates, storing unquantized radiance and foreground
transmittance in a one-pixel float32 target. A 56-byte readback includes cell/ID/
side/grid, depth, validation status and the completed float16 pixel. Explicit
aligned row pitch preserves packed offsets on the tested Metal path. No full-frame
readback or additional per-frame visibility target is required. The probe checks
GPU status and displayed color, then compares the first known f64 membership
interface and source-centre depth. Ambiguous or mismatching raster contacts yield
no usable hit. Foreground transmittance below five percent yields an obscured
result; this is a local display rule. Supplied masks remain read-only.

An accelerated picker must use the exact classification Arc retained by its
completed image. A legacy/accelerated mismatch or a separately prepared
classification is refused before readback, even if its numeric inputs are equal.
The paired frame/probe shader builders share the optical functions and resources.
An unused zero-flag classification falls back to the legacy frame/probe pair.
This adds no per-frame target or readback and does not replace scope, current-view,
membership or visibility checks.

A `PickFrame` captures the originating job's admitted opaque `PickScope`, rather
than relabelling an old front with a newer scope. The caller must renew that scope
across selected source/acquisition, session or tool boundaries and revoke it on
close. Revocation invalidates both in-flight probes and retained results. Desired
`PickState` can describe a new CPU map/revision or view before its GPU image exists;
it must come from live state when preparation is pending. Source pointer, map key/
revision/ID, complete camera/navigation/clip/material/lighting state and size must
match. Every outcome retains its exact front and requires revalidation against
the current state and current pixel before use. An old coherent frame shown under
Updating cannot authorize a new-view hit. These internal APIs provide no editing,
export or agent-sharing route; native brush/workbench integration remains separate.

Manual GPU label uploads retain a separate immutable R8Uint texture bound to the
exact admitted GPU source. A related revision admits a new target, submits a GPU
copy of the previous grid, then queues only changed source tiles. The copy is
submitted before tile staging so it cannot overwrite those edits. Creation, tile
reads, uploads and bounded fence waits belong on a worker. A cancelled/failed
upload discards the unpublished target and leaves earlier textures unchanged.
Identical revisions reuse the existing allocation. Replacement objects get a
full upload; another source/device cannot supply an incremental base.

CPU staging is bounded to one clipped 32-cubed tile. GPU admission includes the
new grid, conservatively padded staging rows and retirement while commands finish.
The current prototype still copies the whole old GPU grid for each changed
revision; it is not sparse GPU residency. Callers retain each revision Arc through
submission/presentation of its texture view. Native 3D uses these immutable inputs
on its region worker.

Supplied GPU inputs retain one selected ID and the complete immutable CPU segment
owner on that exact GPU source. R32Uint texels pack 32 binary memberships or four
original fractional bytes. Only supplied source planes occupy texture depth; a
separate source-z address texture distinguishes known empty planes from omitted
ones. Unused IDs have all-unknown addresses and one zero-initialized dummy texel.
Selection never flattens overlapping regions or narrows their u16 numbers.

Uploads admit compact data, addresses, padded staging and retirement before
allocation. CPU staging holds one raw row, up to 32 packed rows and source-plane
addresses; GPU staging is bounded by 16 MiB plus a final batch/address upload.
Creation and fence waits belong on a cancellable worker. Identical source/object/
revision/ID reuse the existing owner and upload nothing. Failed or cancelled
replacements leave prior objects unchanged. Fractional values and meaning remain
available; categorical first hits apply the same fixed half-maximum view as core.

The mask projection backend traverses nearest source cells independently
of scalar sampling steps. It writes integer (cell x/y/z, ID) texels and signed
physical first-hit depth, skipping missing source samples. It intersects the
half-open categorical domain and optional physical clip after f64 rebasing.
Centred integer pixel offsets preserve exact centre rays before floating-point
division. GPU arithmetic remains f32; an unresolved interval or traversal limit
is an explicit error ID, distinct from an empty ray. Future edits must revalidate
picks against the core f64 ray and the completed scene.
Manual and supplied inputs share that geometry contract. Supplied geometry uses
the original source dimensions, independently of compact packed texture shape.
Its additional address binding requires three sampled textures; a separate manual
pipeline retains the previous two-texture device subset. Manual selection refuses
IDs above 255; supplied selection must match the retained uploaded ID.
Color-source projection reads alpha for eligibility, requiring one more sampled
texture. It cannot use the scalar validity placeholder as proof of a color sample.
The source-material compositor remains scalar-only.

Two pending mask outputs accept contiguous full-width row batches under one exact
request. Only the last encoded row returns an immutable image retaining its label
revision, source, selected ID, camera, view and clip. A published image is never
overwritten. Outputs admit 20 bytes per pixel and bounded per-dispatch uniforms;
each pending image permits at most 256 row batches. Callers submit or drop an
encoder before discarding its pending target and retain completed images through
GPU use. These mask-only first hits include no source opacity: composition must
respect the physical ray order.

A separate worker-only compositor integrates physical source material up to one
opaque selected mask hit. Foreground source attenuates that mask's unlit
linear luminance; source behind it contributes nothing. Scalar interpolation
stops at voxel centres, so the categorical outer half-cell space does not invent
source opacity. Empty masks retain the source and background. The compositor
requires the exact GPU source, completed mask image, camera, navigation and clip;
inversion and MIP are unsupported. Transparent masks need joint ordered events
and are outside this first-hit composition path.

Two bounded pending outputs accept contiguous rows with immutable parameters.
Only a complete image whose GPU arithmetic status has been checked can publish;
cancellation, changed requests, admission refusal and unresolved mask traversal
leave earlier images unchanged. The final four-byte readback uses a bounded,
cancellable worker wait. RGBA16F stores linear radiance and the shared display
pipeline encodes it once at presentation. Output admission includes eight bytes
per pixel, validation state and up to 256 batches of uniforms/bindings. This
backend requires a storage buffer; devices without it retain source-only views.
Native scalar material views can display one selected manual or supplied region
through this path. Ordinary exports and agent snapshots exclude this scene.

The native region view retains the same CPU map owners used by 2D and the upload
worker's shared GPU source. Series selection and exact source Arc identity are
required; numeric geometry equality cannot attach a map. One worker accepts one
running request and one replaceable pending request. Camera changes cancel obsolete
view computation independently of the data-upload lease, so motion cannot repeatedly
restart an unchanged mask upload. A changed map/selected supplied ID cancels that
lease. Manual ID changes share the uploaded all-ID grid. CPU label work suspends the
image while retaining the renderer and prior GPU grid for changed-tile upload when
the edit completes. Removal, disabling the view and source closure drop its owners.

The paint callback only draws an immutable completed image; pipeline construction,
packing, projection, composition and bounded GPU waits stay on the worker. Motion
requests up to 512 pixels per side. After 180 ms without interaction, the worker can
prepare up to 2048 in 16-row batches. Each refined batch fences before another is
submitted, bounding obsolete queued work. A running preview finishes while only
the newest desired camera/style request waits; it can publish a coherent frame
for the same exact source/map/revision/ID while continuous motion continues.
Refinement is cancelled for new input. Partial refinements never replace the
front image. Source/map/ID
changes hide an old region frame immediately. Camera/style changes retain the last
coherent complete frame with an updating caption. Until the first region frame is
ready, the existing source material is shown and labelled as such. Refusals retain
the source and offer an explicit retry. Worker completion and the one refinement
deadline trigger repaints; a completed quiet view schedules no further work.

Native 3D navigation consumes ordered pointer events once per application frame,
including a release's final position. A gesture captures its originating button,
viewport, clip and UI layer only after a finite press inside that interactive
viewport. The click-distance threshold separates jitter from a drag; Space key
transitions preserve pan order even when release arrives in the same frame.
Focus loss, Escape, pointer loss, conflicting buttons, covering controls or changed
viewport ownership cancel capture. Layout retries cannot apply movement twice.
Stationary double clicks retain Fit, and idle frames do not change the camera.

The 3D region selection and brightness are explicit, independent of 2D overlay
visibility/opacity. Manual ID selection is shared with the label editor; supplied
IDs remain u16. Fractional masks use the fixed half-maximum categorical view.
The geometry control chooses mask cells or source-grid cell faces. Switching
geometry retires the previous worker backend; unchanged immutable fronts retain
their own resources. Cell faces use `CellSurface` and `GpuCellSurface`, including
changed-tile/chunk reuse, followed by ordered `OpaqueSurfaceRenderer` composition.
Projection is reused when only material, brightness or lighting changes. Surface
lighting is independent of source lighting. Missing and unknown interfaces remain
open; physical clipping adds no cap. There is no smoothing or decimation.

**Pick surface** enables a read-only primary-click probe for cell faces. Each
requested image captures its originating `PickScope`. A separate bounded worker
checks the actual completed optics against the desired map, revision, ID, camera,
clip, material, both lights, quality, image size and scope. An interactive front
cannot authorize a desired refined view. Foreground transmittance below 5% blocks
a hit regardless of its displayed brightness; exact edge/corner contacts remain
ambiguous. The click is mapped through the actual viewport to the completed raster.
Navigation, tool/view changes, changed viewport bounds or a newer click cancel the
pending probe. Returning to an earlier camera requires a new active originating
scope, rather than reauthorizing the old frame. A newer MPR input generation also
clears pending probes without rerendering or retagging an unchanged 3D scene.
Only a current checked hit returns the source-cell centre to the workbench's MPR
request; the geometric outer face may lie outside its centre-sampling domain.
Standalone 3D shows a local coordinate readout. No label is changed.

This path has no transparency, multiple-ID scene or 3D painting. Enabling it
selects Material; returning to Volume/MIP disables it.
Sharing/export guards apply as soon as the local scene is selected, including
pending or failed preparation, so they cannot substitute a source-only image.

The native labels panel selects either manual labels or read-only supplied masks.
Each keeps its own source association and state; manual undo/dirty guards never
flatten supplied masks into uint8 IDs. A generic overlay cache retains weak map/
completed-image keys, exact source, view kind, visibility/style and device limits.
Unchanged plane textures survive generation changes; changed frames cannot use old
textures. One coalescing worker samples and prepares charged CPU/GPU textures
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
The linked MPR worker retains one immutable image per anatomical plane, keyed by
that plane's physical coordinate, requested sampling side, slab settings and
backend mode. Moving another axis changes the crosshair without resampling or
uploading that image. Both thin and slab interaction retain the requested spatial
sampling and all declared slab intervals. Updates remain provisional for
measurements and exact-view sharing until input is quiet; settling reuses the same
completed pixels. A coherent set completes despite newer pointer input, preventing
starvation, while study closure or a backend-mode change cancels it. A newly
completed preview for an older position is accepted only if its requested side
and slab settings still match the latest request, and its sampling epoch has not
changed. Returning to earlier settings does not revive a queued or late preview
from before an intervening sampling change. Oblique previews follow the same
sampling rule and retain an already-completed compatible preview when a newer
plane position is requested. The idle worker waits on a condition variable.
Linked-plane sliders also recover a primary drag whose press, movement and
release all arrive in one UI pass. The press must be on the visible rail, and
both endpoints must be on its active layer. A release outside the rail is used
only after enough movement to rule out a click; Escape and focus loss cancel the
recovery. Normal held gestures remain with the widget. The fallback maps the
release through the pinned egui slider's handle geometry; recheck that mapping
when the UI library or slider styling changes.
Scalar slabs sample bounded discrete extrema or a mean along an explicit physical
plane normal, excluding missing samples before display mapping. The mean sums at
most 257 finite samples in f64 and divides by the valid count. Slab hashes keep
their original projection bytes and give the mean a new one. CPU workers publish a
requested-detail preview for oblique interaction, then produce a canonical CPU
frame once input is quiet. Scalar previews may use the GPU with a CPU fallback;
the completed preview retains the requested slab sampling.
Linked MPR retains full sampling during movement. Projection parameters
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

A source-bound scalar range grid summarizes powers-of-two tiles of 8–128
interpolation cells. Each tile includes the positive vertex halo, original finite
scalar extrema and separate valid/total vertex counts. At most 262,144 records
(4 MiB) are admitted, with object metadata charged separately. Preparation scans
on a cancellable worker without copying source pixels and retains the exact
immutable source. Missing samples have coverage metadata, not a nonresident page
or a confirmed empty region. Invalid vertices remain in the raw extrema because
zero-weight shader arithmetic can still read them at a valid grid node.

The material interval predicate checks endpoint clamping and every internal knot;
transparent endpoints cannot hide an opaque internal peak. Stored vertex extrema
alone do not certify the legacy rounded sampler's range.

An opt-in scalar material backend certifies the original trilinear sampler using
an outward scalar error bound. Fully expanding its eight weighted vertices gives
at most 27 signed monomials, with absolute sum at most 27 times the largest vertex
magnitude. Fewer than 128 rounded additions/multiplications cover their evaluation
and reassociation. The bound uses gamma-128 with full-ULP directed f32 rounding,
plus a conservative absolute term for subnormal flushes, then rounds outward to
f32. It changes no scalar value or source measurement. A shared integer stencil
binds the classification to the same eight vertices and validity test as the loads.
Lighting retains the existing validity-aware physical source gradient.

An immutable GPU range grid retains the exact source, CPU summaries and material.
Its flags require zero extinction coefficients on every segment touched by that
expanded interval, including ties. An isolated zero knot next to a nonzero
coefficient cannot certify a rounded GPU lerp. Material changes require new
classification; missing coverage remains separate. Source binding uses Arc identity,
not numeric geometry equality. Admission includes the R8Uint flags, side uniform,
bounded padded staging and retained identity; retired GPU work keeps its charge.
Material endpoint clamping precedes division, avoiding overflow for distant source
values and closely spaced knots without changing endpoint semantics.

The integrator keeps its original global midpoint count and spacing. A proven zero
tile can advance over original indices after verifying endpoints one voxel inside
that same integer tile. The affine guard requires at most 4096 samples, source
origin components at most 4096, direction and near magnitudes at most one million,
step in [1e-6, 1e6], and direction times near/total advance at most 4096. Under
those limits the expanded affine expression has absolute term sum below 24,576;
32 directed f32 rounding operations bound its error below 0.125 voxel, including
negligible propagated flushes. Convexity of the exact line and both endpoint errors
keep intervening rounded samples inside the tile. Boundary division proposes an
index only; the original affine expression validates it, with bounded backoff.
Large cancellation, narrow edge tiles or failed tests retain individual-sample
evaluation. No transparent tile means no extra range binding.

The native material renderer and exports retain their existing integration path.
The dense-reference and accelerated material, surface and picker constructors are
backend interfaces; they do not yet enable desktop acceleration or provide
source-brick residency. Native integration requires interaction acceptance.

Source-page inputs address one measured GPU-residency failure: a generated
4096×512×256 f32 volume is admitted on the CPU but exceeds a 2048-voxel 3D
texture side. The legacy layout/extraction prototype remains available for CPU
comparisons. `GpuBrickSource` separately validates fixed 32/64/128-cell layouts
against actual device limits without allocating or reading a dense GPU source.
`GpuBrickResidency` prepares an immutable, source/device/layout-bound generation
of requested page Arcs. It reuses exact prior pages and rejects foreign owners,
duplicate indices and excess admission before uploading. A generation references
at most 256 MiB of pages; the global GPU allowance also counts retained old pages
and pending work. An empty directory means nonresident data, never transparency.

Pages hold original R32Float values, optional R8Uint validity and integer bounds.
A trilinear page needs one positive node beyond its owned cells. Clipped central
secants need one negative and three positive nodes: f32 addition just below a
power-of-two boundary can round a positive secant onto the next integer, whose
zero-weight upper corner is still read by the original eight-value calculation.
A full interior 64-cell secant page therefore stores 68 nodes per axis. Stencils
compute global integer corners and fractions before subtracting the integer page
origin. Ownership, missing samples, outside-source queries and unavailable
stencils have separate statuses. Index gradients require the source inverse
transpose for physical lighting; trilinear-only pages refuse gradient queries.

Worker upload uses one admitted CPU plane and bounded aligned staging, fencing
at 16 MiB and at page completion. Values outside the supported finite GPU range
are refused per page. Cancellation or allocation failure leaves the old complete
generation untouched, and pending GPU commands retain their admission. Actual
GPU tests compare page scalars/secants with independent dense global stencils,
including rounded block boundaries, source ends, missing/extreme values and a
source wider than a constrained device's 3D side. The dense CPU source stays
resident; there is no disk spill.

A bounded GPU atlas packs source pages with an integer directory and the same
sampling/gradient halos. Immutable `GpuBrickAtlas` snapshots can copy retained
pages from an earlier snapshot or upload original CPU values directly. Requested
indices must be distinct and source/pool ownership must match. Atlas storage is
limited to 256 MiB and 128 slots, within actual 3D texture sides and the shared
GPU allowance. Source values and validity are independent of atlas placement.
Direct upload admits one CPU brick of at most 16 MiB, plus aligned GPU staging
of 16 MiB and one maximum padded brick. It flushes at that staging threshold and
at completion, checks cancellation per source row and never copies a whole CPU
volume. Pending GPU work retains its reservation.

`PagedMaterialRenderer` produces complete, checked, immutable linear images.
The global ray interval, midpoint count and sample expression stay unchanged
through page misses. Sixteen-byte float32 continuation records retain radiance,
transmittance and the next original sample index for at most sixteen rows. A
miss records its required page without advancing that index; it cannot act as
transparent data. A bounded request/status readback drives worker uploads. Missing
source samples retain the dense material path's validity rule. Physical gradients
use the source inverse transpose, including reflection and shear. Arithmetic or
stencil failures refuse publication. Batches must be consecutive and retain the
exact source, material and view. At most 4096 waves per batch and 16 GiB of source
uploads per image are allowed. Fully resident required pages use a separate one-dispatch
entry with local ray state and one final status check.

`GpuPagedMaterialRanges` binds the shared zero-extinction classification to the
exact paged source and material, without constructing a dense GPU source. Before
requesting a source page, the shader checks the same bounded, original-index tile
advance used by dense rendering. A proven zero contributes nothing for either a
valid or missing scalar stencil; validity and supplied coverage remain separate.
Classification includes invalid raw vertices and the rounded-interpolation error
bound. Stored-page numeric safety includes the entire gradient halo, so a skip
cannot bypass the original unsupported-value refusal. A page is required if any
of its owned cells lacks that proof. Once every required page is resident, the
complete image needs one dispatch even if transparent pages have never been
uploaded. With no transparent tiles the original bindings and shader are used.
Classified renderers reject even numerically equal replacement material Arcs;
completed images retain their exact classification independently of the cache.
CPU admission includes retained tile flags, required-page flags and bounded safety
scratch. Material changes require new classification and renderer preparation.

The renderer's working cache is private and cannot escape into a completed image.
It fences submitted readers before slot reuse and keeps no encoded, unsubmitted
uses across replacement. Replaced directory entries become nonresident before
source bytes are overwritten. New entries become readable only after their full
values/validity upload completes. Cancellation or refused staging may discard
cache residency, but partially written data remains unmapped and later work can
reload it. Public atlas snapshots remain immutable. Completed image textures and
their exact CPU source/material/view identities are separate from this cache, so
replacement cannot mutate an earlier front. Callers still validate the live
source/session/input epoch before displaying or using a completed result.

Actual GPU tests compare dense and paged scalar/gradient queries and complete
images, force single-slot eviction, verify cancellation/recovery and retain old
fronts. Independent f64 rays check physical attenuation. A constrained device
renders a source wider than its 3D side at a requested width of 2048 pixels.
These backend interfaces do not enable desktop paging, picking, sharing or
exports. Desktop 3D retains its dense source limits until its worker integration,
interaction performance and native visual checks are accepted. Small-cache
streaming remains a separate performance concern from fully resident rendering.

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
is required for background checks. Before starting a selected automatic check,
the worker revalidates that choice while holding the state lock and records the
check as starting under the same lock. An already starting check may finish after
the user turns checks off. Downloads/installations and restart are
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
Image and 3D panes share zoom-gesture ownership while egui's smoothed wheel input
is active. Its remaining delta cannot zoom, pan or browse another pane, and the
ownership clears when the scroll sequence ends, even with the pointer outside.
For wheels or native magnification events without phase information, a new input
after a 150 ms idle gap starts a new sequence. Consecutive magnification events
retain the same owner. An explicitly active wheel touch keeps ownership until
it ends or cancels. Expiry is checked on input without scheduling idle repaints.

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
