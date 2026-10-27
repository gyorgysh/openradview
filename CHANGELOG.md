# Changelog

## 1.1.0 — Unreleased

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
- `--help` and `--version` print to the terminal without opening the viewer.
- Debian installations hide automatic GitHub checks and keep manual checking
  available. Install newer `.deb` files through your package manager.
- Measurement guidance stays inside the image area without moving it, blocking
  mouse controls or drawing beyond the pane. Tool selection requests a repaint.
- Larger Help/Privacy dialogs, visible scrollbars, bottom padding and an
  explanation of negative coordinates in linked MPR.

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
