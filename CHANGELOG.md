# Changelog

All notable changes to OpenRadView are listed here, newest first.
Versions follow [Semantic Versioning](https://semver.org).

## Unreleased

### Added

- **MPR + 3D.** View three linked planes beside a 3D view of the same volume.
  Show thin source planes in 3D, or cut the 3D view at the linked point.
- **Regions in 3D.** Show one manual label or supplied segment at a time over a
  scalar material view. Cell faces can be lit and selected to move the linked
  MPR point. Missing boundaries remain open, and foreground material or source
  planes can block a selection.
- **Physical label editing.** Preview a sphere, disk or drawn path in MPR + 3D,
  then apply or discard it. A separate surface stamp erases one visible cell
  layer. Applied edits can be undone and redone; supplied masks remain read-only.
- **3D appearance.** Adjust the brightness and opacity of scalar values and add
  lighting for depth cues. These controls do not change the source images.
- **Include and exclude hints.** Draw strokes or mark points on one scalar source
  image, then preview a proposed region before applying it. Remove a whole stroke
  when needed. Missing samples and other labels act as barriers. This tool uses
  image values; it does not identify anatomy.

### Changed

- Linked MPR and oblique views retain their requested image detail and slab
  sampling while moving. Supported scalar views use GPU resampling, with a CPU
  fallback. Unchanged linked planes reuse their completed images.
- Fractional segmentation imports retain their original values and whether they
  represent probability or occupancy. Overlays and counts use half the declared
  maximum as the membership threshold.
- Label editing keeps up to 16 undo and redo steps. Returning to the last exported
  revision clears the unsaved indicator.

### Fixed

- The 3D view now indicates when a physical preview is not the region shown.
- Opening physical editing no longer hides a supplied mask when there is no
  manual label map yet.
- Physical-edit indicators are easier to read over bright images.
- Drawing a physical path now ends drawing mode when its preview opens.
- Linked MPR and oblique views no longer accept an older moving preview with a
  different slab setting or image resolution after those settings change.
- A 3D region could fail to appear at certain window sizes when a ray crossed a
  cell boundary. The source image remained visible when this happened.
- An oblique setup error could disappear after another request, leaving the view
  waiting. Errors from older MPR or oblique requests no longer interrupt a newer
  view.
- Short 3D camera drags could lose the last movement on release.
- Exact quarter-turn camera angles could incorrectly hide valid samples next to
  missing data. Label controls also handle small rounding differences in acquired
  plane positions.

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
