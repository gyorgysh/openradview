# Using the viewer

Open local DICOM/NIfTI files, a folder or a ZIP from **Open files**, **Open folder**,
or drag and drop. Pick a series in the sidebar. Loading runs in the background
and can be cancelled. Keep the source files available while viewing; the app
does not make a patient database or copy the whole study into memory.

The included CT, MRI and X-ray demos are credited public research datasets.
They are available without a download. Use them to learn the controls.

## Move around an image

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
**Oblique** adds explicit plane rotation and normal offset. **3D / MIP** projects
the loaded volume; drag to orbit, and use the named transfer presets/controls.
MPR and projections require consistent geometry and a volume within the budget.
Their interpolated values can differ from source pixels. In compact windows,
**Display and tools** opens a scrollable panel that leaves room for the images.
For scalar MPR/oblique views, choose **Slab MIP** or **Slab MinIP** and a physical
thickness. These show discrete maximum/minimum values along the plane normal,
with the sample count shown. A smaller preview refines after interaction stops.
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
an AI model. Export a map explicitly if you want to keep it; changes remain in
memory until then. Unsaved work is protected when replacing or closing it.

**Study → Export picture…** saves an image-only PNG and reproducible view settings
in a new local folder. It omits UI annotations and label maps. Check for text
burned into pixels before sharing. Original files stay unchanged.

**Agents** contains optional local MCP setup and session controls. Sharing needs
its own approval; entering the viewer does not grant it. See [agent setup](agent-access.md).
**Privacy → Updates** contains the update preference and manual check.

See [supported formats and limits](supported-formats.md), [privacy](../PRIVACY.md)
and the [intended-use notice](../DISCLAIMER.md). Ask a qualified clinician to
interpret your scans; do not use the app to make decisions about care.
