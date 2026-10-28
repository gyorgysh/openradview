# Changelog

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
