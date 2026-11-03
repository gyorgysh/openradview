# Changelog

## Unreleased

- Open supported integer DICOM modality tables, with declared units, stored-code
  padding and endpoint clipping. Apply them before interpolation and VOI display;
  preserve uniform mapping context in reformats and picture metadata.
- Apply a single linear DICOM real-world value mapping to stored integer or float
  values, with its declared units. Alternative mappings are still refused.
- Open native 32-bit DICOM Float Pixel Data with its stored values, float padding
  and source windows. NaN and infinite samples count as missing.
- Keep contrast for very wide windows over extreme float values, which could
  render as uniform gray, and show such window values in compact notation.
- Correct native big-endian eight-bit OW image ordering across frame boundaries
  and packed eight-bit grayscale table word ordering.
- Include retained scalar metadata in resident import memory admission.
- Opt into supported DICOM source intervals for local playback, with declared trim,
  fixed/variable timing and visible ready-image pace. Wait for each requested image
  without skipping; stop at the trim end. Manual fps/Loop remain the default.
  Changing playback mode ends existing sharing approval.
- Select supported declared DICOM temporal positions and echo times, with actual
  values shown separately from dimension ordinals. Preserve organization and
  unselected acquisition context; explain missing or ambiguous choices.
- Inspect supported source frame intervals, trim and delay. Timing requires the
  original object frame order; partitioned, reordered or mixed sources remain
  independently viewable; source intervals are an explicit playback choice.

- Select NIfTI time points, components, higher axes and complex views directly
  within their original input. Show declared time coordinates and steps in seconds,
  with frequency, unknown and invalid timing kept distinct. Index navigation
  stays available without valid timing; changing acquisition uses normal source
  selection and ends any existing sharing approval.

- Search and remap local tool shortcuts, with conflict checks and saved bindings
  that contain no study data. Tool labels and Help show the active keys.
- Enter measurement points by image coordinates and select an active MPR plane
  or physical point through controls. Keep numeric Tab navigation and geometry
  undo/redo focus when controls change state.
- Add a manually advanced offline controls guide and technical glossary in Help.
- Optionally show finite reference-plane intersections in local comparison.
  Lines require matching completed thin images and known spatial references,
  follow display transforms, and stay outside picture exports and snapshots.
- Link physical pan, physical zoom and compatible scalar windows separately in
  comparison. Pending actions wait for their requested images and cancel when
  view ownership or context changes; inversion remains per pane.
- Compare four local source panes in a two-by-two grid. Choose one pair for links
  and reference lines; each pane keeps its own tools, marks and image export.
  Extra panes stay in memory and require confirmation before closing. Return to
  two panes to open another study or save/open a comparison file.

- Review an explicit current-pane illustration before saving, with selected completed
  measurement outlines and applied manual/supplied masks on source/thin reformat
  views. Layer changes invalidate the preview; saves reuse its exact PNG bytes.
  Image-only exports and agent snapshots retain their existing scope.
- Save a reviewed MPR grid or up to 16 selected source images in a contact sheet,
  with explicit range, columns, layer choices and generic image/plane captions.
  Sheets fit within 2048 pixels per side and render one bounded tile at a time.
- Preserve files created by another writer when an export's exclusive output
  creation fails.

- Draw ellipse and simple polygon regions on source and thin reformat planes.
  Report scalar/RGB statistics with sampled area separate from continuous shape
  area. Explicitly saved comparisons preserve the new shapes; earlier comparison
  files remain readable. Drafts cancel when their image or view changes.
- Adjust measurement vertices by dragging handles or entering source coordinates.
  Validate a change before replacing the mark, cancel without losing the original,
  and undo or redo up to 16 geometry edits in memory.

- Explicitly load matching local BINARY DICOM segmentation masks on source,
  thin MPR and oblique planes. Preserve overlapping segments and sparse coverage,
  with numeric visibility, styles and source counts. Supplied masks stay read-only
  and separate from manual labels; ordinary image imports exclude segmentation
  object types. No model is used or downloaded.

- Preview and explicitly apply local seeded region growing, keep a selected label
  component or remove small islands. Use four neighbors on one source image or
  six across the volume, with missing samples and other IDs as barriers. No AI
  model or anatomical recognition is used; edits remain undoable and unsaved.

- Inspect local labels on thin MPR and oblique planes, with per-ID visibility,
  fill/outline/hatching and source voxel counts. Keep missing samples separate
  and derive label cell volume from the source affine. Painting stays on source
  images; slabs, 3D and shared snapshots omit labels.
- Open a local DICOMDIR or a ZIP with one media index, validate referenced image
  identities, and show missing/conflicting references with generated study groups.
  Constrain file-set reads to their selected folder or archive. Open folder scans
  supported files directly when an index is unusable.
- Show selected-series regular geometry, label-grid size, decoded-size estimate
  and declared lossy/derived image counts in Image info without decoding the
  whole series. Keep geometry readiness separate from pixel and device admission.
- Explain skipped local inputs with bounded reason counts, including when no
  supported images load. Distinguish known unsupported encodings and DICOM object
  types, missing NIfTI companions, malformed metadata and uncertain input failures.

## 1.2.0 — 2026-10-28

- Open a separate local source in the right comparison pane, including when the
  first source has only one image. Choose series and controls independently;
  comparison stays local.
- Explicitly save comparison views and measurements in a path-free local file,
  and restore them after both original series have been verified. No scan copies
  or study history are created; unsaved measurements and changes to a saved
  comparison are protected.
- Keep source-image measurements and the current view when entering comparison;
  cancelling a source replacement leaves existing measurements intact.

## 1.1.1 — 2026-10-27

- Fix macOS update installation when verifying the signed application and
  companion. Version 1.0.0 and 1.1.0 installations need a manual DMG update
  to reach a version containing this fix.
- Check sealed Mac bundle contents and portable package inventories before
  replacement and backup cleanup, preserving backups when a mismatch is found.
- Preserve the complete Windows backup while an older app or companion is
  still running.

## 1.1.0 — 2026-10-27

- Choose physical slab thickness with maximum/minimum projections in scalar
  linked MPR and oblique views. Bounded previews refine after interaction; missing
  samples stay excluded. Slab readouts/metadata identify projections, and physical
  measurements stay disabled until a thin plane is selected.

- Clip experimental 3D/MIP views with explicit physical LPS bounds. Clipping
  participates in exact-view approval and picture metadata; source data stays intact.

- Compare two source series locally with independent pane controls and optional
  physical slice linking for compatible geometry. Memory limits remain shared;
  picture export captures the active pane. Entering comparison ends sharing, and
  returning to a single series requires fresh approval before sharing again.
- Keep multiple temporary distance, angle and rectangular ROI marks while
  navigating the current series or exact reformat plane. Select/delete/clear
  them from Measurements; statistics run in the background. Visible series gain
  lazy, bounded previews. Marks and previews stay in memory and clear on closure.
- macOS Cmd+Q and menu Quit now honor the existing choice to keep or discard
  unsaved labels and the close guard during update installation.
- Rotate 2D/source and reformatted views by 90° and flip them horizontally or
  vertically. Cursor values, orientation letters, measurements, label overlays
  and explicit picture exports follow the display while source coordinates stay
  unchanged. Linked MPR highlights the active pane.
- Honor source grayscale window functions and bounded VOI tables, including
  alternative source windows. Keep scaling, display mapping and inversion
  separate. Show declared intensity units/source precision, and bind presentation
  into snapshots and source verification. Correct automatic float-image contrast.
- Support explicit enhanced CT/MR/PET frame geometry within a bounded subset.
  Separate time, echo, diffusion and component acquisitions before reconstruction;
  incomplete geometry stays in 2D. Refuse conflicting functional groups and
  unsupported value mappings instead of assuming spatial or intensity meaning.
- Adapt decoded-slice caching and prepared-volume capacity to physical memory,
  with shared ceilings up to 8 GiB each and lower limits under pressure. Large
  studies can retain more decoded slices for back-and-forth browsing. GPU
  admission also checks the selected adapter; limits and allocation failures
  retain a 2D/MPR fallback. Active views reuse prepared sources, and CPU MPR avoids
  repeated coordinate transforms while preserving geometry and missing samples.
- Cached slices remain retrievable while other workers decode slow input. Hot
  decompression streams stay bounded and waiting reads remain cancellable.
- Linux packages declare the missing desktop libraries and a Vulkan driver.
- Native Linux OpenGL fallback, explicit `WGPU_BACKEND` selection, actionable
  graphics startup errors and local adapter/driver diagnostics. Drivers without
  compute support keep 2D/MPR available; 3D/MIP explains the requirement.
- Fix an OpenGL 3D texture-view validation error, preserving linear-light display
  interpolation without alternate texture views.
- Fix source grayscale lookup-table display on OpenGL, including single tables
  and multiple-table arrays.
- `--help` and `--version` print to the terminal without opening the viewer.
- Debian installations hide automatic GitHub checks and keep manual checking
  available. Install newer `.deb` files through your package manager.
- Measurement guidance stays inside the image area without moving it, blocking
  mouse controls or drawing beyond the pane. Tool selection requests a repaint.
- Larger Help/Privacy dialogs, visible scrollbars, bottom padding and an
  explanation of negative coordinates in linked MPR.

Windows/Linux packages remain previews. Native CI covers tests, GPU rendering
and installer probes; broader interactive desktop/accessibility, consumer GPU
performance and older-version production update acceptance remain pending.
See [platform status](docs/platforms.md) and [validation](docs/validation.md).

## 1.0.0

Initial release of the local image viewer:

- DICOM and supported NIfTI-1/2 files, folders and ZIP/ZIP64 archives, with bounded
  background loading and cancellation.
- 2D navigation, linked and oblique MPR, experimental GPU volume rendering/MIP,
  measurements, image information, histograms, local labels and picture export.
- Scoped local MCP connections with explicit session approval and instance
  discovery; optional experimental commentary from the user's own agents.
- Shared signed-manifest updater, user-controlled downloads/restart, native
  macOS app/DMG, Windows per-user installer and Linux Debian/portable packages.
- Offline software, font and dataset license viewer in Help.
- Automatic update checks enabled by default, with opt-out on first launch or in
  Privacy → Updates; downloads and installation require user action.

Apple Silicon/macOS has native UI and Metal checks. Windows/Linux previews have
native test and installer checks; interactive desktop and older-version update
acceptance remain separate.
See [formats and limits](docs/supported-formats.md), [platform status](docs/platforms.md)
and [validation](docs/validation.md).
