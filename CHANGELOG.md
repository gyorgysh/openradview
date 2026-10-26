# Changelog

## 1.1.0 — Unreleased

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
