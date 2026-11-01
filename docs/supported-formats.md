# Supported inputs

OpenRadView is a personal-education viewer, not clinically validated. These are
implemented input/display subsets, not a DICOM conformance statement or evidence
that an image, orientation, reconstruction or measurement is correct for care.

| Input | Supported scope | Limits |
|---|---|---|
| Grayscale DICOM | Integer 8/16-bit MONOCHROME1/2; native implicit/explicit little endian, explicit big endian, JPEG, RLE, JPEG-LS, JPEG 2000 and deflated explicit-VR datasets | Stored precision must fit the allocated sample. Nonlinear modality LUTs, real-world value mappings, floating pixel data and unsupported transfer syntaxes are refused. Enhanced spatial support has the explicit subset below. |
| RGB/YBR DICOM | Unsigned 8/16-bit RGB and YBR_FULL, native interleaved/planar; native packed YBR_FULL_422 at 8-bit, even width, planar 0; JPEG RGB/YBR_FULL/YBR_FULL_422; JPEG2000 RGB/YBR_RCT/YBR_ICT; RGB/YBR_FULL RLE/JPEG-LS | JPEG/JPEG2000 perform their own color transform; native/RLE/JPEG-LS YBR is converted once. 2D, linked MPR, oblique and experimental RGB volume/maxima views with valid regular geometry and consistent color context. Scalar window/level does not apply. Desktop rectangle statistics report separate RGB code values; MCP roi_stats remains scalar-only. Pan, zoom, browse, invert and snapshots work. |
| Palette DICOM | Signed/unsigned 8/16-bit stored indices, direct 8/16-bit RGB LUTs (including 8-bit entries in words), segmented discrete/linear/indirect tables | Matching descriptors required; indices clip to table endpoints. No alpha/enhanced/supplemental palette semantics. Invalid or cyclic/nested indirect segments are refused. |
| Embedded color profile | DICOM RGB input ICC v2/v4 (scnr, XYZ/Lab PCS), converted to sRGB with relative colorimetric intent | Max 1 MiB profile, bounded CLUT/TRCs; no fetching. Invalid/unsupported profiles fail rather than silently assuming sRGB. Absent profile assumes sRGB. This is profile conversion, not monitor calibration or a calibrated-display guarantee. |
| NIfTI-1/2 | Single-file `.nii`/`.nii.gz` and paired `.hdr`/`.img` (either member may be gzipped), little/big endian; uint8/int8, uint16/int16, uint32/int32, uint64/int64, float32/float64, complex64/complex128. Time, vector/tensor and higher axes become separately labelled scalar subseries; complex views are magnitude, phase in radians, real and imaginary. | Up to 4,096 subseries and one million displayed frames per import. No RGB/RGBA, binary, float128/complex256 or Analyze. Components are viewed individually, without vector/tensor reorientation, tractography or temporal analysis. Values become f32 and may lose integer/f64 precision. Enabled nonfinite scaling or voxel values are refused. Header descriptions/extensions are ignored. |
| Local sources | Files, folders, ZIP and ZIP64 with stored/deflated members; supported NIfTI including gzip and same-directory pairs can be inside ZIP | No extraction, uploads or pixel disk cache. ZIP/gzip integrity and expanded length are checked on a cancellable background worker. Selected files must remain available. |
| DICOMDIR | Select a local `DICOMDIR` alone, or a ZIP containing one file-set index. Explicit-VR little-endian Basic Directory; direct IMAGE references under PATIENT/STUDY/SERIES records. Defined/undefined sequence and item lengths, file-relative record offsets, extensionless files and unique case-insensitive names are supported. | Read-only subset: no indirect/MRDR references or directory writing. Non-image records are counted as unsupported. Cycles, orphan active records, contradictory hierarchy, duplicate files/instances, unsafe paths, links/reparse points and ambiguous names are refused. At most 16 MiB, 50,000 records and 16 hierarchy levels. |
| Supplied DICOM masks | Explicit local selection through Labels… → Supplied masks. Read-only BINARY SEG, native implicit/explicit little endian, one-bit MONOCHROME2, matching loaded DICOM source grid and explicit source references. Overlap and sparse coverage are preserved on source/thin MPR/oblique views. | Up to 64 numeric segment IDs, 4,096 source/mask frames, 32 million source voxels and four million pixels per source/output plane; 128 MiB packed masks and 128 MiB pixel payload, subject to shared memory admission. No resampling, editing, supplied-mask export, slabs, 3D or ordinary agent snapshots. |

A media index selects only its referenced images. Each accepted image's own SOP
class/instance, transfer syntax, study and series must match its record. Missing
files and conflicting identities are counted separately; directory descriptions
and patient/study free text are not shown. Study labels use temporary generated
numbers. Missing study identifiers are not inferred or combined into a study.
Local File IDs have one to eight components of one to eight ASCII letters,
digits or underscores. Resolution is confined to the selected directory handle,
including later pixel reads; changed files are refused. ZIP references stay
inside the index's archive subdirectory and never resolve to local files. A ZIP
with multiple indexes is refused rather than choosing a file-set.

**Open folder** explicitly scans supported image files independently of any
DICOMDIR, including ZIP members encountered during that scan. Use it on the
containing folder when the media index is incomplete or unusable. For a ZIP with
an unusable index, unpack it with your own archive tool and select that folder;
the viewer does not extract files. Folder scanning can include supported images
that were not listed by an index.

Recognized segmentation, surface segmentation, label map and height map objects
are excluded from ordinary image imports, even when they carry pixel data. They
are reported as unsupported object types. Supplied masks require the explicit
selection above; they never attach automatically. The initial reader requires one
derivation/source reference per mask frame, matching frame of reference, rows,
columns, spacing, orientation and position. Multiframe sources need one explicit
original frame number. Missing/wrong/ambiguous references and duplicate segment/
source-plane pairs are refused. Metadata is bounded to 8 MiB, 100,000 tokens and
64 nesting levels; repeated attributes in one dataset are refused. The supported
file ends at its exactly sized, even-padded native pixel payload. Compressed or
deflated masks, fractional/label-map/surface/height-map objects, concatenations and
different grids are unsupported.

Each supplied segment retains its own positive bits and source-plane coverage.
An omitted plane is unknown for that segment; a supplied all-zero plane is known
empty. Counts separate stored positives from positives on valid image samples.
Segment volumes can overlap and must not be added. Display uses numeric IDs,
visibility, fill/outline/hatching and overlap crosshatching; supplied names are not
retained. The manual NIfTI label map has independent editing, undo and export.

Import keeps reason counts for skipped inputs in memory. Known unsupported
encodings and object families, missing paired files, malformed DICOM metadata
and unrecognized inputs have separate categories. Other unsupported/invalid
representations remain an explicit combined category when a more precise reason
is unknown. No supported images yields an error with the collected counts.
Cancellation, changed sources and hard limits still fail the operation. A
successfully indexed file can still fail later pixel decoding; the summary is
not a full-study pixel validation result.

DICOM modality rescale and signedness are applied before grayscale display;
MONOCHROME1 inversion remains a display operation. Source LINEAR, LINEAR_EXACT
and SIGMOID functions are explicit, with up to 16 alternative window pairs and
16 VOI lookup tables. Unknown functions and malformed pairs/tables are refused.
Lookup descriptors support 8/16-bit outputs, up to 65,536 entries, signed/unsigned
first inputs, endpoint clipping and packed/word-padded 8-bit data. The supported
source subset requires at most 16 stored bits and integral rescale whose possible
outputs fit exact f32 integers (±2²⁴); fractional/inexact source LUT mappings are
refused. Fractional resampled LUT indices round down after scalar interpolation.
A table and a window function are alternatives; they are never applied together.
GPU display widths below 10⁻²⁰ are refused because subnormal arithmetic may be
flushed to zero; choose a wider contrast range.
Spatial volumes require matching lookup tables across frames; differing tables
remain available in source 2D.

Scalar context records source storage/precision, the conversion to f32, numeric
scaling and recognized declared units. Unknown units remain unknown; a CT/PET
modality name does not establish HU/SUV. Integer/f64 source precision can be lost
when values become f32. Windowing and lookup tables do not change histogram/probe/
ROI sample values. Native views and snapshots share the same grayscale mapping. Color never substitutes luma
for scalar values. Pre-ICC RGB code values retain 8/16-bit precision separately from
8-bit RGBA presentation, and both participate in source fingerprints. Each color
pixel occupies 10 decoded-cache bytes. Palette/YBR code statistics refer to the
converted RGB codes, not the original indices/chroma or physical modality units. The native view and MCP PNG snapshots share the same renderer.
PNG manifests identify color presentation, source encoding/precision and the
applied profile hash, with display calibration explicitly unverified.

Enhanced CT, MR/MR Color and PET can use explicit shared/per-frame patient
position, orientation and pixel spacing. Spatial reconstruction requires a complete
dimension-index table, declared dimension organizations, a positive in-stack
position, valid regular geometry and consistent reference. Ordinals are scoped
to their organization; they are not assumed to equal the indexed values. Stack
identities stay separate, including across source objects. Time, echo, diffusion, complex components and other declared
nonspatial indices form separate acquisitions. Classical echo/time context is
also separated. Acquisition labels use generated numbers, not patient tags.
Missing/incomplete geometry, unresolved mixed components and sampled/distorted
frames remain source-only 2D. No positions are inferred from slice thickness.
Conflicting shared/per-frame macros, mismatched frame counts and unsupported value
mappings are refused. Functional-group tables are limited to 65,536 frames and
16 dimension indices, within the existing metadata/token bounds; an import is
limited to 4,096 series/acquisitions. This is a supported subset, not general
enhanced-DICOM or quantitative PET support; no HU/SUV unit is inferred.

NIfTI spatial units are converted from metres/mm/micrometres into LPS millimetres.
A valid known sform takes precedence over qform, including a negative qfac. The
manifest preserves the chosen method and reference code: scanner, aligned,
Talairach, MNI or another template are not interchangeable. No scanner/patient
reference is assumed for template coordinates. Missing/invalid/unknown geometry
or in-plane shear gets no anatomical labels/physical measurements; the source
pixel grid remains viewable in 2D. MPR/3D require a valid regular stack; scalar and color samples or different color contexts cannot be mixed.
Time/component indices and complex presentation survive source hashes and snapshot metadata. Nonspatial axes are never mixed into a spatial stack. Paired files require a unique same-directory companion; no header-supplied paths are followed. Both files must remain unchanged. The NIfTI source grid is not relabelled as an acquisition plane. Non-CT volumes start with a neutral source intensity range calculated on the worker; CT retains its Bone starting preset. Source range and Reset are available in 3D controls. Transfer functions retain their explicit numeric limits.

The decoded 2D cache and prepared-volume pools grow with total/available physical
RAM, up to 8 GiB each. They are shared across imports/views and allocate only as
images are requested. A 500 MiB study is not restricted to 128 MiB: file size is
separate from decoded memory, which also depends on compression, scaling and color.
The cache evicts older slices when its allowance is full; eviction changes speed,
not which source images can be opened. Prepared MPR/oblique/3D sources retain their
pixels while needed, including RGB code buffers and validity masks.

For example, the policy chooses these initial ceilings when the stated physical
memory is available. Actual pressure/device limits can lower them:

| RAM / available RAM | Decoded slice cache | Prepared CPU volumes | Host-side GPU ceiling |
|---|---:|---:|---:|
| 8 / 4 GiB | 512 MiB | 1 GiB | 1 GiB |
| 16 / 8 GiB | 1 GiB | 2 GiB | 2 GiB |
| 32 / 20 GiB | 2 GiB | 4 GiB | 4 GiB |
| 64 / 48 GiB | 4 GiB | 8 GiB | 8 GiB |
| 128 / 100 GiB | 8 GiB | 8 GiB | 8 GiB |

CPU sources, cache and GPU copies share the host headroom calculation, so Apple
Silicon memory is not counted twice as independent RAM and VRAM. GPU admission is
also limited by the selected adapter, texture/compute support and source/output
costs. Metal uses its recommended working-set size; Windows uses matching dedicated
adapter capacity (not a free-VRAM claim); Linux uses matching DRM memory counters
when exposed by the driver. Unknown/ambiguous adapter capacity keeps a conservative
limit. Allocation failures retain a 2D/MPR fallback within their own limits.

RAM counters refresh on worker activity, without an idle timer or process listing.
Optional cached pixels are evicted on use after the ceiling falls. Active volumes
stay valid while pressure restricts new preparation. Swap is not counted as fast
working memory, and the app creates no study-data spill files, persistent cache or
memory/session log. Preparation scratch, displayed slices, labels and driver memory
remain separate; these allowances are not a total process-memory guarantee.
Maximum32million samples per frame
counts all three color channels; maximum dimension8192, compressed frame payload64MiB,
encoded metadata8MiB/object, parsed grayscale choices8MiB/object and total
metadata128MiB/study,200,000 entries and one million images apply.
Compressed-input expansion is cumulatively capped at64GiB (nested layers also count).
A multi-GB file therefore does not imply a multi-GB resident pixel buffer. The
NIfTI parser additionally requires a bounded integral voxel offset (at least 352/544 bytes for single-file v1/v2; zero is permitted for pairs) and
valid datatype/bitpix/dimensions. Unsupported entries are counted as skipped;
oversized inputs fail explicitly. Large ambiguous encapsulated multiframe and
bit-packed DICOM use a fallback limited to 64 MiB per input object. Larger ambiguous objects are refused.

Gzip and deflated datasets/ZIP members are sequential compressed streams. Forward
navigation can reuse one of four bounded hot readers; reverse/distant reads can
restart decompression. Slow I/O/decoding does not hold the decoded-entry lock, so
already-cached slices remain retrievable by other workers.
Opening an already uncompressed source can be faster. Integrity, cancellation and
resource tests are software checks; native codecs/OS allocations are not a hard
process-memory sandbox. Windows/Linux runtime coverage is limited to the checks
listed in [platform status](platforms.md). No built-in AI model is included.

Color resampling and compositing use linear-light sRGB presentation. Pre-ICC RGB
code values are interpolated separately and rounded to their declared precision
for desktop rectangle statistics; these are not physical/modality units. Missing
reformatted pixels remain missing and are excluded from statistics. RGB maximum
projection takes a separate maximum per channel, potentially from different
depths. Color opacity uses linear-light brightness on a0–255 scale, without CT
presets. Presentation uses RGBA8 even when source codes have16-bit precision.
Scalar MPR/oblique slabs support minimum/maximum of trilinear modality samples,
0.001–2000 mm thickness and 3–257 evenly spaced samples including both boundaries
and the center. The displayed sample count is explicit; bounded sampling can miss
features between samples. Padding/outside samples are excluded, and an entirely
missing ray stays missing. RGB slabs are not supported. Slab snapshots record the
sampling grid separately from physical measurement geometry.

Scalar and color snapshots identify renderer version 8. Scalar manifests include
typed source precision/units/scaling; resampled outputs do not claim the first
source frame's linear transform as their own. Source choices participate in both
whole-source fingerprints and per-frame verification. The geometry-only agent
query keeps its existing scope.
The final native display preserves RGB and interpolates the sRGB view in linear
light. Rectangle statistics run on cancellable workers; clearing/changing the
image or closing the study discards pending results.

## Local labels

The desktop can create a uint8 label volume for one selected regular source series.
Paint/erase on its acquired 2D grid, fill a scalar range on one/all slices, change
visibility/opacity and undo the last two edits. Labels have no anatomical names or
inferred meaning. Thresholds follow the selected scalar representation; no color
threshold, automatic organ segmentation or model inference is provided.

Load a scalar NIfTI label volume with integer values0–255 (zero background), exact
dimensions/affine and the same NIfTI spatial-reference code. DICOM sources accept
scanner-reference NIfTI labels with matching numeric geometry. Numeric equality
does not establish anatomical correspondence: the user must verify the overlay.
Import does not register or resample a map to another grid. Supplied BINARY DICOM
SEG has the separate read-only import described above. DICOM SEG export and
RTSTRUCT import/export are unsupported.

Export explicitly to a new .nii filename: NIfTI-2 uint8, label intent1002, exact
f64 sform in RAS millimetres, without source pixels/names/identifiers. Existing
files are never overwritten. Maps remain in memory until export; closing, study
replacement and quitting protect unsaved work. Only one source series has a label
map at a time; changing series retains it until explicitly removed. The limits
are32Mi voxels and4Mi pixels/slice, alongside the adaptive shared source-volume allowance.
Workers clone a private edit, publish only successful results and retain two
checkpoints; cancellation leaves the displayed map unchanged. Labels appear on
source 2D and matching thin MPR/oblique planes, with per-ID visibility and
fill/outline/hatching. Nearest-voxel display never interpolates IDs; half ties
choose the larger index, with source cells covering [-0.5, dimension-0.5).
Stored IDs on missing samples are preserved but not displayed. Output validity
also masks the overlay. Slabs and 3D omit labels. Current map, source, completed
plane generation/preview and display settings must match before compositing;
unmatched old layers are hidden while the worker prepares replacements.
Overlay buffers are limited to four Mi-pixels and the device texture-side limit;
failure leaves the underlying image available. Source counts report stored and
valid voxels separately. Label cell volume includes all stored labeled cells and
uses the absolute source affine determinant, including shear; it is not the
product of displayed grid-step lengths. Visibility/zoom/resampling do not change
these counts or the original map. Labels are excluded from renderer/MCP snapshots. No new agent tool or
permission was added. Label maps can themselves contain sensitive information;
export is a user-chosen local save, not an anonymization guarantee.

Connected selection uses an inclusive scalar interval with explicit four-neighbor
source-image or six-neighbor whole-volume connectivity. A seed must be in scope,
on a valid source sample, in range and on background or the selected ID. Growing
adds connected background cells and preserves existing labels. Cleanup keeps a
seeded selected-ID component or removes components strictly smaller than an
explicit cell cutoff. Missing samples are barriers and their stored labels remain
unchanged. Other IDs are never overwritten. No diagonal connectivity, anatomical
inference, registration or model is provided.

Selection and its proposed map are prepared privately on workers. Preview is
bound to exact map/source identity, source seed, interval, scope, label and edit
kind. A changed input or view mode invalidates it. Apply swaps one immutable
checkpoint into the current map with the existing two-step undo/unsaved guard.
Cancel/admission failure cannot partly change the current map. Source counts and
exports continue using applied labels. The preview does not enter agent snapshots.
Grid limits are checked from metadata before dense preparation; every retained
map/checkpoint is charged to the CPU volume pool. Traversal admits up to one byte
of mask and four bytes of queue per cell in scope before allocation. The queue is
released after computation; the mask is private until a proposed map is prepared.

## Local inspection

The selected series' header summary checks regular volume geometry with the same
affine rules as materialized volumes, including regular tilt. It reports grid
steps, label-grid size limits and a decoded-data estimate separately from current
memory/device admission and pixel-context validation. It decodes no additional
frames. Declared lossy-compression and original/derived flags are counted per
image; absent, malformed or unavailable declarations remain unknown. These flags
are not an independent verification of source quality. Affine steps are not
acquisition slice thickness. No raw tags, identifiers or source paths are exposed.

Source 2D **Image info…** captures one displayed frame, its display settings and
image number. It shows dimensions, declared spacing, NIfTI component/complex
context and spatial-reference type without raw patient tags or paths. Close the
window to resume manual navigation. Inspection itself creates no image history or saved export.

The histogram counts every valid pixel on a cancellable worker, using 256
equal-width bins from each channel's minimum to maximum. The upper endpoint is
included in the final bin; a constant channel uses the first bin. Missing scalar
samples/transparent reformat pixels are excluded and counted separately. Scalar
values follow stored scaling; no physical intensity unit is inferred from a
modality name. Color counts preserve RGB code precision before ICC conversion;
without retained codes, they use 8-bit sRGB presentation. Display window, invert,
pan and zoom do not change the distribution. Log count scale changes the graph,
not its values. Channel bins can cover different value ranges.

Hover readouts use the nearest displayed pixel centre, without extra interpolation
or display window/inversion. Column/row numbers start at one. A source view reads
source values; MPR/oblique readouts describe their resampled images and may differ.
No readout or histogram establishes a medical finding.

Edge letters follow validated image-column/row directions in the LPS reference,
including oblique combinations. Unknown geometry and current quadruped inputs
remain unlabeled. The image-space scale follows declared spacing and current fit/
zoom. It is not monitor calibration or a promise of true object size in projection
images. NIfTI scanner/aligned/Talairach/MNI/template references remain distinct.
These overlays are desktop context and stay outside existing image-only snapshots.

**Rotate / flip** changes source 2D, linked MPR or oblique display orientation by
90° steps and horizontal/vertical flips. It changes no source pixels, acquisition
geometry or physical coordinates. Probes, measurement marks, direction letters,
scale bars and local label overlays follow the displayed view. In linked MPR the
brighter pane border marks the active pane. Fit resets pan/zoom; Reset orientation
in the menu restores the source display axes. Volume projections use camera roll.
Picture exports retain the display transform in their scene metadata. Nothing is
saved automatically. Each transformed exact view needs its own preview approval.

The series filter searches modality/series-number and NIfTI subvolume labels,
without changing the active series until a row is selected. Import builds a
bounded 32 MiB text catalogue on its worker; one cancellable search worker retains
only the latest query. Only visible rows are laid out. One cancellable worker
prepares previews for at most 32 visible rows, sampling the middle source image
with its original presentation. At most 32 64×64 previews are retained in RAM;
the shared source cache and RAM/GPU reservations apply. No thumbnail files or
extra agent scope are introduced. Clearing a study closes inspection/filter/preview workers.

Distance, angle, rectangle, ellipse and simple polygon ROI marks remain in memory across navigation in
the current series. Source marks bind to the exact frame ordinal and geometry;
reformat marks bind to the exact plane and sampling grid. Each collection holds
at most 128 marks and 4096 points; polygons have 3–256 vertices, without holes or
crossing/touching edges. **Measure** chooses the tool. Ellipses use two opposite
corners; polygons finish with Enter or **Measurements… → Finish polygon**.
Backspace removes the last draft point and Escape cancels. Changing image, plane,
view mode or tool cancels a draft. **Measurements…** lists marks on the current image/plane and
offers selection, deletion and clearing. ROI statistics run in the background,
one calculation per collection. Changing series or closing its view clears marks;
restart restores none automatically. Comparison marks can be explicitly saved in
a local comparison file and restored only against matching source series. They
are excluded from image-only exports and agent snapshots.

**Measure → Edit measurements** selects or drags a vertex. **Measurements…** also
offers a vertex selector and 1-based source column/row with **Apply vertex
coordinates**. The original stays intact until the completed proposal validates;
Escape, navigation override, context changes or invalid geometry cancel it.
Undo/redo retains up to 16 geometry edits in memory, including the matching
validated reports. Deleting or clearing marks clears this edit history. Unapplied
coordinate changes must be applied or cancelled before saving a comparison.

ROI statistics include valid pixel centres inside the region boundary. Sampled
area counts valid cells; shape area describes the continuous outline. They can
differ, especially for small regions or missing samples. Physical areas require
valid spacing and plane geometry. Color statistics retain their declared RGB code
domain. These tools do not recognize anatomy or identify disease.

## Local series comparison

**Compare series** shows two independently navigable source 2D series. The right
pane can open a separate local file selection, folder or archive, including a
single-image source. Each pane has its own source cache; process-wide RAM/GPU
ceilings are shared, without preparing two dense volumes. Select a pane with its
button or by interacting with its image/slider, then choose a series from that
pane's source in the sidebar. Tools, contrast and explicit picture export apply
to the active pane. Narrow layouts stack the panes vertically.

Optional physical linking requires complete, regular parallel planes and the same
declared spatial reference. Dimensions, spacing, source order and in-plane display
orientation can differ. Each navigation table is bounded to 32 MiB and admitted
against the shared prepared-source RAM pool. Checking uses headers on workers,
without decoding a whole series. A source image's physical center chooses the
nearest target plane within its field of view and half-slice end bounds. A point
outside that field leaves the other image unchanged with a message. This is no
registration, resampling or proof of anatomical correspondence. Missing, irregular
or unrelated geometry keeps navigation independent. NIfTI linking also requires
a matching loaded-source reference and transform context; a template name alone
does not establish correspondence.

Entering comparison revokes live/frozen sharing and cancels shared reviews/work.
Agents cannot access the dual-pane layout. Return to a single series and review a
fresh Allow before sharing. An explicit comparison file can retain views and marks;
it has fingerprints but no source paths or pixels and may still be sensitive.
Reopening requires the same sources in the same pane order and fresh verification.
Files with ellipse/polygon marks use version 2 (up to 2 MiB). Earlier version 1 files
retain their256KiB limit; files containing only the earlier mark kinds still use
version 1. Invalid geometry or source mismatches leave the current comparison intact.
No study history or thumbnail files are created. Picture export captures
only the active completed source pane through the existing explicit export path.

## Local playback and picture export

Source 2D series have explicit Play/Pause, requested1–30fps (default8) and optional
Loop. The default stops at the end. A ready frame receives its full interval before the next request;
late decoding lowers speed instead of skipping images or queuing catch-up work.
This is paced browsing, not acquisition timing or a multiaxis time-series player.
Manual view changes, Escape, source/mode changes, label editing, dialogs and loss
of focus pause it. Paused playback schedules no timer; decoding wakes pending work.

**Study → Export picture…** captures one completed pane from source2D, linked MPR,
oblique or3D/MIP. It writes image.png plus the existing snapshot view.json schema
inside a new user-chosen folder. The displayed aspect, window/inversion, pan/zoom,
physical plane/camera and sampling settings are retained. Output is bounded to
1024pixels per side for slices and512 for volumes; it is a rendered picture,
not an original-resolution dataset export. No app overlays, measurements or label
map appear. A whole MPR grid, DICOM secondary capture and batch/video export are
not implemented. Larger outputs would need a separately designed resource policy.

**Study → Export illustration…** separately captures one source image, settled
thin MPR/oblique plane or active comparison pane, up to 1024 pixels per side. Added
layers start unchecked. Completed current-plane outlines/numbers and the selected
applied manual/supplied mask group can be included. Mask visibility, fill/outline/
hatch style, opacity, overlap pattern and nearest categorical sampling follow the
native view. Unapplied connected-region proposals are unavailable; finish/cancel
measurement drafts/edits first. Comparison panes have no applied mask layer.
Slabs and 3D are unsupported. Results, selection/edit handles, crosshairs, scales
and app controls are excluded. Outlines use fixed output-pixel strokes/fonts.

Prepare preview checks the fresh source and renders immutable selected layers on
a worker. Changing layers clears the preview and disables saving until a new
matching preview finishes. The displayed preview may be scaled to fit; Save uses
its exact PNG bytes, after fresh source verification. It writes image.png and a
separate version 1 illustration.json in a new OpenRadView-illustration folder.
That manifest records the scene, geometry/source/PNG hashes, selected outline
coordinates/kinds/numbers and mask settings/raster hash. It is not the image-only
snapshot schema and does not contain an editable mask volume. Reproduction needs
the original source and masks. No files are written until explicit Save. A late
cancellation after both files finish leaves the completed export in place.

File selection, whole-series fingerprints, frozen-volume source verification,
GPU readback/PNG encoding and writes run on one cancellable worker using the
native GPU device. Retained volume identities are matched independent of source
ordering (including left-handed NIfTI); mismatched source pixels are refused.
Volume verification is bounded to131,072frames. Existing output is never replaced;
new folders/files are created exclusively, with0700/0600 permissions on Unix.
Only files successfully created by that operation are removed on failure/cancellation;
failed exclusive creation never transfers ownership of an existing file. Forced exit or power
loss can still leave an incomplete export; neither is claimed transaction-safe.

No network or agent permission is involved. Exported settings exclude paths,
patient tags and reference UIDs; they contain geometry and content fingerprints.
Identifying text already burned into pixels remains possible. Review both files
before sharing. Nothing is saved automatically, and no study history is created.

## Native decoder policy

JPEG-LS RGB/YBR presentation follows each frame's encoded interleave mode.
Component-mode buffers are reordered; CharLS already returns pixel order for
line/sample modes. DICOM Planar Configuration does not override the compressed
stream. Known-value fixtures cover 8/12/16-bit precision and split/mixed-layout
multiframe sources.

The pinned build uses CharLS 2.4.5 and OpenJPEG 2.5.4 with reviewed downstream
bounds/progress fixes. Exact sources and licenses are included in the repository;
packages record their inventory hashes. JPEG 2000 uses strict complete-codestream
decoding, rather than progressive-download tolerance. Truncated/partial inputs
return an error. Complete supported lossless, lossy and RGB16 fixtures remain
covered. Input/volume budgets are unchanged; these checks are not a guarantee
against every malformed file or an OS/process memory sandbox. See
[validation and native source scope](validation.md).

## Declared padding

Supported integer monochrome and palette DICOM use Pixel Padding Value and its
optional inclusive range in stored code values, before rescale or palette/ICC
conversion. These pixels are absent from automatic contrast, probes, histograms,
ROI statistics, threshold labels, MPR/oblique and volume sampling. No arbitrary
background or anatomy is inferred. Invalid padding metadata is refused rather
than guessed; float-pixel padding remains outside the supported input formats.
All-padding pictures have no valid statistics. Explicit source window settings
are retained. Scalar masks use one byte per source pixel only when needed and
count toward the decoded cache/CPU volume allowance. GPU admission also counts
the scalar/mask textures and reserves both output buffers. Over-budget 3D remains
available in 2D/MPR within their limits; allocation errors return a fallback
message. Interpolation does not fill through absent samples.
See [validation and representation](validation.md).
