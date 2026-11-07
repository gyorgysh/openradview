# Using the viewer

Open local DICOM/NIfTI files, a folder or a ZIP from **Open files**, **Open folder**,
or drag and drop. Pick a series in the sidebar. Loading runs in the background
and can be cancelled. Keep the source files available while viewing; the app
does not make a patient database or copy the whole study into memory.

For supplied media, use **Open files** to select `DICOMDIR` alone, or a ZIP with
one media index. The viewer opens its supported referenced images and groups
declared studies with temporary numbers. Expand **File-set import** for missing
files, conflicting identities and unsupported objects. Source descriptions and
patient names from the index are not catalogue labels. Burned-in text can still
appear in images.

If a media index cannot be used, choose **Open folder** on its containing folder.
This scans supported files directly, including unlisted images, and ignores the
index. For an unusable index inside a ZIP, unpack the ZIP with your archive tool
first. The app does not repair the index, modify originals or extract archives.

If some inputs are skipped, expand the sidebar's skipped-input summary to see
reason counts. It distinguishes known unsupported encodings/object types, missing
NIfTI companions and malformed DICOM metadata; ambiguous failures remain
unsupported or invalid. An import with no supported images includes these counts
in its error. Indexed files still need successful pixel decoding when browsed.

The included CT, MRI and X-ray demos are credited public research datasets.
They are available without a download. Use them to learn the controls.

## Move around an image

In source 2D, **Image info… → Series geometry and source** checks regular volume
geometry, affine grid steps and label-grid size without decoding the whole series.
It also shows an estimated decoded size and declared lossy/derived image counts.
Unknown declarations remain unknown. Compatible geometry alone does not establish
pixel compatibility, anatomical correspondence or current RAM/GPU availability;
those view requirements are checked when opening the view.

| Action | Control |
| --- | --- |
| Browse images or move a plane | Scroll, ↑/↓, or Images **S** and drag |
| Move the image | Pan **P**, or hold Space while dragging |
| Zoom | Pinch, Cmd/Ctrl + scroll, or Zoom **Z** and drag |
| Change grayscale display | Contrast **W**, or edit Width/Level |
| Fit the image | **R** or Fit |
| Invert the display | **I** or Invert |
| Restore initial display settings | Reset display |

Select a tool before dragging with the primary button. Pan/zoom moves the displayed
image; Images changes which image or plane is shown. Color images use their
original color presentation instead of scalar window/level.

## Views and tools

**2D image** shows one source image. **Linked MPR** shows axial, coronal and
sagittal planes through one linked point; choose Crosshair to move that point.
Thin planes keep their requested sampling during navigation, and planes whose
position did not change reuse their images.
**Oblique** adds explicit plane rotation and normal offset. **3D / MIP** projects
the loaded volume; drag to orbit, and use the named transfer presets/controls.
In Linked MPR, choose **MPR + 3D** from the layout menu to place the linked
planes beside a 3D view of the same volume.
MPR and projections require consistent geometry and a volume within the budget.
Their interpolated values can differ from source pixels. In compact windows,
2D, MPR, oblique and 3D/MIP place their display rows under **Display and tools**,
a scrollable panel that leaves room for the images. Tab and Shift+Tab reach every
control and scroll it into view; Escape closes informational dialogs.

In **3D / MIP**, **Material** offers an editable scalar-to-brightness and opacity
mapping, with optional **Lighting** for depth cues. The selected **Material…** button lets
you add, remove and edit up to 64 points. Values must stay in increasing order;
an invalid edit leaves the last valid material visible. **Reset material** restores
the initial mapping. These controls change appearance, not source values or masks.
The last complete interaction image stays visible while a refined image is built.
Material view ends any active sharing approval and has no picture export or agent
sharing. Choose **Volume** or **MIP** for the original display controls and those
features; sharing still requires a fresh Allow review.

For scalar MPR/oblique views, choose **Slab MIP**, **Slab MinIP** or **Slab mean** and a
physical thickness. These show the discrete maximum, minimum or mean of valid
samples along the plane normal, with the sample count shown. Navigation retains
the requested plane detail and slab sampling.
When you change slab settings, the last completed image remains visible while
the new one is calculated. A status line identifies the displayed sampling
until it matches the selected settings.
At most 257 samples span a slab, so features between samples can be missed.
Slabs disable physical measurements; choose **Thin plane** to measure again. **Clipping…** in 3D/MIP
limits the projection to explicit LPS minimum/maximum coordinates in millimeters.
Restore full source bounds or disable clipping to see the full volume again.

**Compare series** opens two source-image panes, even for a single-image source.
When entering from **2D image**, its view and source-image measurements move into
the left pane.
Choose **Open in right pane…**, then **Files or ZIP…** or **Folder…**, to load
another local source. Choose **Left pane** or **Right pane**, then select
that pane's series in the sidebar. Leaving comparison keeps the active pane's
source. If the original source has unsaved labels, select its pane before leaving.
Tools and picture export apply to the active pane, marked by a brighter border.
Navigation and contrast stay independent. **Link physical slices** is available
for compatible declared geometry; it follows physical image centers. It does not
register anatomy. Entering comparison ends sharing; sharing after leaving needs
a fresh Allow. Different positioning or projections can affect the appearance
of images taken at different times.

In **Links and reference**, **Fuse pane 2 over pane 1** (for the chosen pair)
shows the second pane's current image over the first, with **Opacity** or a
**Checkerboard**. It needs the same conditions as slice linking, and the two
images must be on the nearest matching plane. Each pane keeps its own contrast.
The fused picture is for viewing only: probes, statistics and exports keep each
image separate, and nothing aligns differently positioned data.

To keep the comparison, choose **Comparison… → Save comparison…** and choose a
new local file name. The file contains view settings, measurement points and
source fingerprints, but no image pixels, source paths or patient tags. It may
still contain sensitive information; a cloud-synced folder may copy it elsewhere.
Nothing is saved automatically, and the original images are not modified.

To reopen it, first open the original sources and select the same series in the
same left/right order, then choose **Comparison… → Open saved comparison…**.
Both series are checked against their saved fingerprints before either pane is
restored. If a source is missing, changed or does not match, reopen the correct
source and try again. Checking large series can take time. The comparison file
does not contain the scans, so keep the originals separately. The app prompts
before discarding unsaved comparison measurements or changes to a saved view.

**Measure** offers distance, angle and rectangle statistics when the displayed
geometry supports them. **Image info…** shows dimensions, spacing, image context
and histograms. Readouts and measurements are for exploration and can be wrong.
No projection measurement is treated as a source-plane measurement.
Marks stay in memory while browsing the same series. **Measurements…** selects,
deletes or clears them; outside comparison they are cleared when the series/view
closes. They are never saved automatically. Sidebar previews are generated for
visible series in memory.

**Labels…** paints a local source-grid label map. It is not organ recognition or
an AI model. Export a map explicitly as an uncompressed `.nii` file if you want
to keep it; changes remain in memory until then. Unsaved work is protected when
replacing or closing it.
The same map appears on matching thin MPR and oblique planes. Open **Labels…**
there to choose fill, outline or hatching and hide individual numeric IDs.
Counts use original source voxels and declared cell volume, with valid and missing
samples kept separate; zoom and display resolution do not change them. Painting
and threshold edits stay on source images. Slab projections omit labels.

For a supported scalar volume, enable **Region in 3D** in the 3D controls and
choose a manual label or supplied segment. The material view shows one opaque ID
at a time. Fractional segments use half their declared maximum. Open **Mask
cells…**, choose **Cell faces**, then enable **Pick surface** to inspect a
visible face. When source planes are shown, the menu is already called **Cell
faces…**. In **MPR + 3D**, a successful pick moves the linked point to the
source-cell centre. A visible source plane or foreground material can obscure a
region or block a pick. Missing or unknown boundaries stay open, and clipping
does not add a cap.

In **MPR + 3D**, **Cut at linked MPR planes** keeps a chosen side of the linked
point in 3D. These cuts combine with any separate clipping bounds; neither changes
the source images, label map or physical editing extent. **Reset linked cuts**
clears the linked cuts without resetting the separate clipping bounds.

Choose **Physical edit…** to preview a sphere or disk at the linked point, or
draw a path on one thin plane. Create blank manual labels first if needed; supplied
masks stay read-only. Close the preview window to inspect the proposal, then
reopen it to apply or discard. The preview can reach hidden cells and is not
limited by 3D clipping. Counts and export continue to use the applied map.
The 3D view says **PREVIEW NOT IN 3D** when it is showing a different region or
only the source while the region loads. With a linked cut it says **PREVIEW
HIDDEN · CUT**. Enable **Region in 3D**, select **Manual** and wait for it to
finish to inspect the proposal there.

**Surface stamp…** is a separate erase tool for an applied manual label. Finish
drawing, apply or discard any physical preview, and close Physical edit first. Select
**Choose a face**, then click a face in the completed 3D view. Inspect the
resulting preview, then apply or cancel. It removes one visible cell layer within
the chosen radius; covered or ambiguous faces cannot anchor a stamp. Neither tool
saves the label map automatically. In a compact window, both editing tools are
inside the 3D settings.

In **Labels… → Connected regions**, choose **Grow from seed**, an inclusive
scalar interval and either the current source image (four neighbors) or the whole
volume (six neighbors). **Pick seed on source image** closes the panel; click once
and it reopens. Rotation/flips affect the display, while the seed stays on the
source grid. You can also enter a 1-based column/row and use that typed seed.
**Preview connected edit** shows the proposed map. Close the panel to inspect it,
then reopen Labels and **Apply connected edit** or **Discard preview**. Counts and
exports use the applied map. Changing the source/map, interval, scope, label or
seed invalidates the proposal. Changing view mode also discards the preview.

**Keep seeded component** removes other valid components of that ID within scope.
**Remove small islands** removes components strictly smaller than the chosen cell
count; equal-size components remain. Diagonal contacts do not connect. Missing
samples and other IDs are barriers; cleanup preserves labels stored on missing
samples. These tools use values and connectivity, with no AI model or anatomical
recognition. Applying is one undoable edit; export explicitly to keep it.

In **Labels… → Include / exclude seeds**, choose a rectangle on the current
scalar source image. Add at least one include and one exclude hint by entering
1-based coordinates or choosing **Draw include hint** or **Draw exclude hint**
and clicking or dragging on the image. Squares mark include points and crosses
mark exclude points. The list shows the first six hints; **Remove last hint**
removes a whole stroke or one typed point, and **Clear seeds** removes all hints.
Hints are limited to 8,192 distinct class/position pairs. Drawing a hint on
another image replaces the earlier set. All seeds must be inside
the rectangle and on valid samples that are not occupied by other label IDs.

**Preview seeded region** uses scalar values and four-neighbor propagation. Inspect
the overlay, then choose **Apply seeded region** or **Discard seeded preview**.
The rectangle is limited to 1,048,576 cells and work is bounded; if the calculation
does not converge, reduce the rectangle or adjust the hints and try again. Other
IDs and missing samples are barriers. Exclude results can remove cells from the
selected label inside the rectangle. This tool has no anatomical recognition or
trained model and can cross weak image boundaries. Counts and exports use the
applied map; Apply creates one undoable edit.

**Study → Export picture…** saves an image-only PNG and reproducible view settings
in a new local folder. It omits UI annotations and label maps. Check for text
burned into pixels before sharing. Original files stay unchanged.

**Agents** contains optional local MCP setup and session controls. Sharing needs
its own approval; entering the viewer does not grant it. See [agent setup](agent-access.md).
**Privacy → Updates** contains the update preference and manual check.

See [supported formats and limits](supported-formats.md), [privacy](../PRIVACY.md)
and the [intended-use notice](../DISCLAIMER.md). Ask a qualified clinician to
interpret your scans; do not use the app to make decisions about care.
