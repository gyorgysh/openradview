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
  macOS bundle and a release workflow that creates reviewed drafts.

Apple Silicon/macOS is the tested platform; Windows/Linux remain build previews.
See [formats and limits](docs/supported-formats.md), [platform status](docs/platforms.md)
and [validation](docs/validation.md).
