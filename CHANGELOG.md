# Changelog

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
