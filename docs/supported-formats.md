# Supported inputs

OpenRadView is a personal-education viewer, not clinically validated. These are
implemented input/display subsets, not a DICOM conformance statement or evidence
that an image, orientation, reconstruction or measurement is correct for care.

| Input | Supported scope | Limits |
|---|---|---|
| Grayscale DICOM | Integer 8/16-bit MONOCHROME1/2; native implicit/explicit little endian, explicit big endian, JPEG, RLE, JPEG-LS, JPEG 2000 and deflated explicit-VR datasets | Stored precision must fit the allocated sample. Nonlinear modality LUTs, floating pixel data and unsupported transfer syntaxes are refused. Enhanced per-frame geometry is not inferred. |
| RGB/YBR DICOM | Unsigned 8/16-bit RGB and YBR_FULL, native interleaved/planar; native packed YBR_FULL_422 at 8-bit, even width, planar 0; JPEG RGB/YBR_FULL/YBR_FULL_422; JPEG2000 RGB/YBR_RCT/YBR_ICT; RGB/YBR_FULL RLE/JPEG-LS | JPEG/JPEG2000 perform their own color transform; native/RLE/JPEG-LS YBR is converted once. 2D, linked MPR, oblique and experimental RGB volume/maxima views with valid regular geometry and consistent color context. Scalar window/level does not apply. Desktop rectangle statistics report separate RGB code values; MCP roi_stats remains scalar-only. Pan, zoom, browse, invert and snapshots work. |
| Palette DICOM | Signed/unsigned 8/16-bit stored indices, direct 8/16-bit RGB LUTs (including 8-bit entries in words), segmented discrete/linear/indirect tables | Matching descriptors required; indices clip to table endpoints. No alpha/enhanced/supplemental palette semantics. Invalid or cyclic/nested indirect segments are refused. |
| Embedded color profile | DICOM RGB input ICC v2/v4 (scnr, XYZ/Lab PCS), converted to sRGB with relative colorimetric intent | Max 1 MiB profile, bounded CLUT/TRCs; no fetching. Invalid/unsupported profiles fail rather than silently assuming sRGB. Absent profile assumes sRGB. This is profile conversion, not monitor calibration or a calibrated-display guarantee. |
| NIfTI-1/2 | Single-file `.nii`/`.nii.gz` and paired `.hdr`/`.img` (either member may be gzipped), little/big endian; uint8/int8, uint16/int16, uint32/int32, uint64/int64, float32/float64, complex64/complex128. Time, vector/tensor and higher axes become separately labelled scalar subseries; complex views are magnitude, phase in radians, real and imaginary. | Up to 4,096 subseries and one million displayed frames per import. No RGB/RGBA, binary, float128/complex256 or Analyze. Components are viewed individually, without vector/tensor reorientation, tractography or temporal analysis. Values become f32 and may lose integer/f64 precision. Enabled nonfinite scaling or voxel values are refused. Header descriptions/extensions are ignored. |
| Local sources | Files, folders, ZIP and ZIP64 with stored/deflated members; supported NIfTI including gzip and same-directory pairs can be inside ZIP | No extraction, uploads or pixel disk cache. ZIP/gzip integrity and expanded length are checked on a cancellable background worker. Selected files must remain available. |

DICOM modality rescale and signedness are applied before grayscale display;
MONOCHROME1 inversion remains a display operation. Color never substitutes luma
for scalar values. Pre-ICC RGB code values retain 8/16-bit precision separately from
8-bit RGBA presentation, and both participate in source fingerprints. Each color
pixel occupies 10 decoded-cache bytes. Palette/YBR code statistics refer to the
converted RGB codes, not the original indices/chroma or physical modality units. The native view and MCP PNG snapshots share the same renderer.
PNG manifests identify color presentation, source encoding/precision and the
applied profile hash, with display calibration explicitly unverified.

NIfTI spatial units are converted from metres/mm/micrometres into LPS millimetres.
A valid known sform takes precedence over qform, including a negative qfac. The
manifest preserves the chosen method and reference code: scanner, aligned,
Talairach, MNI or another template are not interchangeable. No scanner/patient
reference is assumed for template coordinates. Missing/invalid/unknown geometry
or in-plane shear gets no anatomical labels/physical measurements; the source
pixel grid remains viewable in 2D. MPR/3D require a valid regular stack; scalar and color samples or different color contexts cannot be mixed.
Time/component indices and complex presentation survive source hashes and snapshot metadata. Nonspatial axes are never mixed into a spatial stack. Paired files require a unique same-directory companion; no header-supplied paths are followed. Both files must remain unchanged. The NIfTI source grid is not relabelled as an acquisition plane. Non-CT volumes start with a neutral source intensity range calculated on the worker; CT retains its Bone starting preset. Source range and Reset are available in 3D controls. Transfer functions retain their explicit numeric limits.

The decoded cache is shared and bounded at128MiB. MPR/oblique/3D have a
separate512MiB decoded-volume limit plus GPU limits, including retained RGB code buffers. Maximum32million samples per frame
counts all three color channels; maximum dimension8192, compressed frame payload64MiB,
metadata8MiB/object and128MiB/study,200,000 entries and one million images apply.
Compressed-input expansion is cumulatively capped at64GiB (nested layers also count).
A multi-GB file therefore does not imply a multi-GB resident pixel buffer. The
NIfTI parser additionally requires a bounded integral voxel offset (at least 352/544 bytes for single-file v1/v2; zero is permitted for pairs) and
valid datatype/bitpix/dimensions. Unsupported entries are counted as skipped;
oversized inputs fail explicitly. Large ambiguous encapsulated multiframe and
bit-packed DICOM use a fallback limited to 64 MiB per input object. Larger ambiguous objects are refused.

Gzip and deflated datasets/ZIP members are sequential compressed streams. Forward
navigation can reuse a hot reader; reverse/distant reads can restart decompression.
Opening an already uncompressed source can be faster. Integrity, cancellation and
resource tests are software checks; native codecs/OS allocations are not a hard
process-memory sandbox. No runtime-validated Windows/Linux release or built-in AI model is claimed.

Color resampling and compositing use linear-light sRGB presentation. Pre-ICC RGB
code values are interpolated separately and rounded to their declared precision
for desktop rectangle statistics; these are not physical/modality units. Missing
reformatted pixels remain missing and are excluded from statistics. RGB maximum
projection takes a separate maximum per channel, potentially from different
depths. Color opacity uses linear-light brightness on a0–255 scale, without CT
presets. Presentation uses RGBA8 even when source codes have16-bit precision.
Scalar and color snapshots identify renderer version 5.
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
No registration, label resampling or DICOM SEG/RTSTRUCT import/export is provided.

Export explicitly to a new .nii filename: NIfTI-2 uint8, label intent1002, exact
f64 sform in RAS millimetres, without source pixels/names/identifiers. Existing
files are never overwritten. Maps remain in memory until export; closing, study
replacement and quitting protect unsaved work. Only one source series has a label
map at a time; changing series retains it until explicitly removed. The limits
are32Mi voxels and4Mi pixels/slice, alongside the512MiB source-volume budget.
Workers clone a private edit, publish only successful results and retain two
checkpoints; cancellation leaves the displayed map unchanged. Labels show only
in source2D, and are excluded from renderer/MCP snapshots. No new agent tool or
permission was added. Label maps can themselves contain sensitive information;
export is a user-chosen local save, not an anonymization guarantee.

## Local inspection

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

The series filter searches modality/series-number and NIfTI subvolume labels,
without changing the active series until a row is selected. Import builds a
bounded 32 MiB text catalogue on its worker; one cancellable search worker retains
only the latest query. Only visible rows are laid out. No thumbnail decoding or
extra agent scope is introduced. Clearing a study closes inspection/filter workers.

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

File selection, whole-series fingerprints, frozen-volume source verification,
GPU readback/PNG encoding and writes run on one cancellable worker using the
native GPU device. Retained volume identities are matched independent of source
ordering (including left-handed NIfTI); mismatched source pixels are refused.
Volume verification is bounded to131,072frames. Existing output is never replaced;
new folders/files are created exclusively, with0700/0600 permissions on Unix.
Known partial outputs are removed on failure/cancellation. Forced exit or power
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
count toward the existing decoded cache/CPU volume budget. The combined scalar
volume/mask GPU budget is also 512 MiB; over-budget 3D remains available in 2D/MPR
within their existing limits. Interpolation does not fill through absent samples.
See [validation and representation](validation.md).
