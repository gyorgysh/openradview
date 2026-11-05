# Changelog

All notable changes to OpenRadView are listed here, newest first.
Versions follow [Semantic Versioning](https://semver.org).

## Unreleased

### Added

- **Surface erase stamps.** In MPR + 3D, choose a physical radius and click a
  visible face of an applied manual label. Review a separate preview before
  Apply or Cancel; each applied change adds one Undo step. Only the original
  visible cell layer inside the radius can change. Covering planes, foreground
  opacity and ambiguous hits can block editing. Hidden IDs and supplied masks
  cannot be edited with this tool.
- **Source planes in 3D.** Show the completed thin axial, coronal and sagittal
  images inside the linked 3D material view, with individual visibility controls
  and the same MPR contrast and inversion. Source planes and one region's cell
  faces share depth ordering and clipping. This view retains its requested
  resolution and selected quality during movement, showing complete images as
  they become ready. Slab projections show an unavailable reason. Region picks
  respect covering source planes and foreground opacity. Volume or MIP hides
  the planes.
- **Linked 3D cuts.** In MPR + 3D, keep either side of each completed MPR plane
  through **3D settings → Cut at linked MPR planes**. Cuts follow the point and
  intersect the separate manual clipping bounds. Resetting linked cuts preserves
  those bounds. Source images, labels and brush scope are unchanged; cut surfaces
  stay open. Conflicting bounds show an explicit empty view.
- **Physical label edits.** In MPR + 3D, preview a sphere or a disk with a chosen
  radius and thickness in millimetres at the linked point, or draw a continuous
  physical sweep on one thin plane. The proposal appears
  on the planes and manual 3D region before Apply; Undo and Redo keep the same
  map revisions. Hidden IDs, other labels and missing source samples are
  preserved. Moving the point or leaving the layout discards an unapplied edit.
- **Cell faces and linked picking.** Display a region's source-grid faces with
  optional lighting. With **Pick surface** enabled, click a visible face to
  inspect its source cell; in MPR + 3D, the three planes follow that cell's centre.
  Picking waits for the current completed view and refuses obscured or ambiguous
  hits, including faces covered by a source plane. Missing and unknown
  boundaries stay open. Labels are unchanged.
- **MPR + 3D.** View the three linked planes beside a local 3D projection of the
  same volume. The projected cursor follows their physical position; 3D keeps
  separate camera and appearance controls. Manual and supplied region selection
  uses the same labels panel. The guides have no depth test or editing action;
  region display still shows one opaque ID. Combined views stay local.

- **Regions in 3D.** Display one manual label or supplied mask ID over a scalar
  material view. GPU preparation runs on a background worker and shows complete
  frames. Fractional masks use half their declared maximum. This view stays local
  and has no picture export or agent sharing yet.
- **3D materials and lighting.** For scalar volumes, map intensity points to
  brightness and opacity per millimetre, with optional lighting for depth cues.
  The material view uses a larger interaction image and refines in bounded tiles.
  It stays local; picture export and agent sharing use **Volume** or **MIP**.

### Changed

- Linked MPR uses GPU resampling where supported and keeps its requested image
  detail and slab samples during movement. Unchanged planes reuse their completed
  images. Unsupported sources/devices use the same sampling on the CPU.
- Fractional segmentation imports retain their original values and probability
  or occupancy meaning in memory. Counts and overlays still use values at or
  above half the declared maximum.
- Manual label editing shares unchanged tiles between revisions and keeps up to
  16 undo/redo steps. Strokes that change no cells preserve the history; returning
  to the last exported revision clears the unsaved indicator.

### Fixed

- 3D camera drags retain movement delivered in the same frame as button release,
  including short gestures. Pan keeps the originating button and Space state.
- Volume-camera turns at exact multiples of 90° avoid tiny off-axis errors that
  could incorrectly hide valid samples beside missing data.
- Manual label controls recognize acquired planes whose stored decimal positions
  differ slightly from the fitted regular volume grid.
- Thin-plane linked MPR keeps its requested image detail while moving the
  crosshair or a plane slider. Planes whose position did not change reuse their
  images instead of being resampled and uploaded again.

## 1.3.0 - 2026-11-03

Labels and supplied masks on every plane, more DICOM image types including
mammography and calibrated ultrasound, four-pane comparison, seven built-in
examples and better keyboard use in small windows.

### Added

- **DICOMDIR media sets.** Open a DICOMDIR, or a ZIP that contains one, and see
  which referenced images are missing or conflict. If the index is broken,
  **Open folder** still reads the images directly.
- **More DICOM image types.** Modality lookup tables, 32-bit float pixel data
  and a single linear real-world value mapping, with declared units where the
  file has them. Variants that aren't supported are refused with a reason.
- **Mammography and X-ray labels.** Mammograms are recognized as MG. When a
  2D image states its side, view (such as MLO) and patient orientation, Image
  info shows them and the image edges show the orientation letters.
- **Ultrasound distances.** Distances and angles on ultrasound images use the
  calibrated regions stored in the file, when all points lie in one region.
- **Labels on every plane.** See labels on MPR and oblique planes, with
  per-label visibility, fill, outline or hatching, and voxel counts.
- **Region tools.** Grow a region from a seed point, keep one connected part of
  a label, or remove small islands. Each edit is previewed before you apply it,
  and Undo works. No AI is involved.
- **Supplied segmentation masks.** Load binary, fractional or label-map DICOM
  segmentation masks that belong to the open series. A fractional mask counts a
  voxel when its value is at least half the stated maximum. Overlapping
  segments stay separate, and the masks are read-only.
- **Ellipse and polygon regions.** Statistics report the sampled area and the
  outline's area separately.
- **Reviewed picture exports.** Choose which measurements and masks to include.
  The saved file is exactly the preview you reviewed.
- **Contact sheets.** Save up to 16 images, or the three MPR planes as a grid,
  up to 2048 pixels per side.
- **Four-pane comparison.** Compare up to four series in a grid and choose
  which pair links together.
- **Reference lines.** Show where the other pane's plane crosses this one.
- **Fused viewing.** Show the comparison pair together with opacity or a
  checkerboard when both images share a spatial reference. Each image keeps
  its own contrast, readouts and exports.
- **Time and echo choices.** Pick NIfTI time points and components, and DICOM
  temporal positions and echo times. Declared values (NIfTI time in seconds,
  echo times in milliseconds) are shown apart from the position numbers.
- **Recorded playback timing.** Play a series at the timing stored in the file
  instead of a fixed frame rate. Playback waits for each image and stops at the
  end of the recorded range.
- **Mean slab.** Average the samples across a slab, next to MIP and MinIP.
  Missing samples are left out of the average.
- **Overlays and shutters.** Show the overlay planes and display shutters
  stored with an image. Both start off and never change measurements.
- **Presentation states.** Apply a file's window, inversion, rotation and flip.
  A file that also contains annotations, shutters or other settings is refused
  instead of partly applied. **As acquired** returns to the original view.
- **Custom shortcuts.** Search and remap tool keys. Help lists the active keys
  and includes an offline controls guide and glossary.
- **Seven built-in examples.** New: a four-view mammogram, a liver ultrasound,
  a whole-body PET, and a prostate MRI with an AI-made mask you can load from
  Labels… → Supplied masks. The picker groups the examples by kind of image.

### Changed

- See why files were skipped: an unsupported encoding or object type, a missing
  NIfTI companion file, or damaged metadata.
- Image info shows a series' geometry, size and whether it forms a regular
  volume, without decoding every image.
- Drag a measurement's points or type exact coordinates. Undo and Redo cover the
  last 16 changes.
- Link pan, zoom and contrast separately in comparison.
- Tab and Shift+Tab move through controls and scroll them into view, and the
  focused control shows an outline. Escape closes Help, Privacy, Image
  information, credits and licenses, and focus returns to the control that
  opened the dialog.
- Small windows keep room for the image. Display controls fold under **Display
  and tools**, and two comparison panes sit side by side unless the window is
  taller than wide.
- Overlays, shutters, reference lines and fused views are never included in
  exports or shared with an agent. Entering comparison, or changing the
  acquisition or playback mode, ends any active sharing approval.

### Fixed

- Big-endian 8-bit DICOM images and some packed lookup tables showed pixels in
  the wrong order.
- Float images with a very wide contrast range showed as plain gray.
- Very large window and level values now use short notation.
- A failed export no longer removes a file that another program created.

## 1.2.0 - 2026-10-28

Open a second study in comparison and save comparison views.

### Added

- **Second source in comparison.** Open a separate local study in the right
  pane, even when the first study has only one image. Each pane chooses its own
  series and controls, and comparison stays on your computer.
- **Saved comparisons.** Save comparison views and measurements to a local file
  without image copies or file paths. Reopening checks both original series
  first. Unsaved measurements and changes to a saved comparison are protected.

### Changed

- Entering comparison keeps your measurements and current view. Cancelling a
  source replacement leaves existing measurements in place.

## 1.1.1 - 2026-10-27

Fixes for installing updates.

### Fixed

- **macOS updates** failed while checking the signed app and its companion.
  Installations of 1.0.0 and 1.1.0 need one manual DMG update to get this fix.
- Updates now check the app bundle and package contents before replacing
  anything, and keep the backup if something doesn't match.
- Windows keeps the complete backup while an older app or companion is still
  running.

## 1.1.0 - 2026-10-27

Slabs, clipping, two-series comparison and support for larger studies. Windows
and Linux packages remain previews; see [platform status](docs/platforms.md)
and [validation](docs/validation.md).

### Added

- **Slabs.** Choose a physical slab thickness with maximum or minimum
  projection in linked MPR and oblique views. A quick preview sharpens when you
  stop moving, and missing samples stay out. Measurements turn off until you
  return to a thin plane.
- **3D clipping.** Limit 3D and MIP views to physical bounds. Source data is
  not changed.
- **Two-series comparison**, with independent controls and optional physical
  slice linking when the geometry matches. Picture export captures the active
  pane. Entering comparison ends sharing.
- **Several measurements at once.** Keep distance, angle and rectangle marks
  while you browse the series or plane, and select, delete or clear them from
  Measurements. Statistics are calculated in the background.
- **Series previews** that load as you scroll.
- **Rotate and flip** 2D and reformatted views. Cursor values, orientation
  letters, measurements, labels and picture exports follow the display, and
  source coordinates stay the same.
- **Source display settings.** Use the window functions, alternative windows
  and lookup tables stored in the file. Image info shows declared units and
  source precision.
- **Enhanced CT, MR and PET** frame geometry, for a supported subset. Time,
  echo, diffusion and component acquisitions are separated before building a
  volume; incomplete geometry stays in 2D.
- `--help` and `--version` print to the terminal without opening the viewer.

### Changed

- Memory for decoded slices and volumes now adapts to your computer, up to
  8 GiB each, with lower limits under pressure. Large studies keep more slices
  ready for browsing back and forth.
- 3D checks the selected graphics adapter first, and 2D and MPR stay available
  when it can't fit a volume.
- Browsing stays responsive while other images decode, and slow reads can still
  be cancelled.
- Linux packages declare the desktop libraries and Vulkan driver they need.
- Linux can fall back to OpenGL, `WGPU_BACKEND` chooses the graphics backend,
  and graphics startup errors explain what to do. Drivers without compute keep
  2D and MPR, and 3D explains what it needs.
- Debian installs hide automatic GitHub checks; install newer `.deb` files with
  your package manager.
- Bigger Help and Privacy dialogs with visible scrollbars, and a note on
  negative coordinates in linked MPR.

### Fixed

- Cmd+Q and the Quit menu on macOS now respect your choice to keep or discard
  unsaved labels, and the close guard while an update installs.
- A 3D texture error on OpenGL.
- Source lookup tables on OpenGL, including multiple tables.
- Contrast for float images when chosen automatically.
- Measurement hints no longer move the image, block the mouse or draw outside
  the pane, and choosing a tool redraws straight away.

## 1.0.0 - 2026-10-26

First release of the local image viewer. macOS on Apple Silicon has native
interface and graphics checks; Windows and Linux are previews. See [formats and limits](docs/supported-formats.md),
[platform status](docs/platforms.md) and [validation](docs/validation.md).

### Added

- **Open your images.** DICOM and supported NIfTI-1/2 files, folders and
  ZIP/ZIP64 archives, loaded in the background and cancellable.
- **View them.** 2D browsing, linked and oblique MPR, experimental 3D and MIP,
  measurements, image information, histograms, local labels and picture export.
- **Optional agents.** Connect a local MCP agent, with your explicit approval
  for each session, and get experimental commentary from your own agent.
- **Updates.** Signed update feeds, with downloads and restarts only when you
  choose. Checks are on by default; turn them off on first launch or in
  Privacy → Updates.
- **Packages.** A macOS app and DMG, a Windows per-user installer, and Linux
  Debian and portable packages.
- **Licenses in Help**, available offline.
