# Desktop platforms and local archives

| Package | Checks and limits |
| --- | --- |
| Apple Silicon macOS 13+ · DMG | Native UI and Metal checks; release app/DMG use Developer ID signing, notarization and Gatekeeper verification |
| Windows x64 MSVC · Setup EXE / ZIP | Native default/full tests, GPU rendering and installer probes; interactive desktop checks remain separate. No Authenticode signature |
| Linux x64 GNU · TAR.GZ / DEB | Native default/full tests, Vulkan rendering and installer probes; interactive desktop checks remain separate. ELF library requirements are in build.json |

Windows/Linux packages are previews. Native CI evidence is included in releases;
it does not establish consumer GPU performance, accessibility or a production
upgrade from an older published version. Other architectures require separate
packages and checks. Development CI stays manual and cached; tag releases use
[the protected release workflow](github-release.md).

## Native Linux build

Use Rust from rust-toolchain.toml, a C/C++ compiler, CMake, pkg-config, Python3.11+
and binutils/readelf. On Ubuntu22.04 the native build/runtime packages include
build-essential, cmake, pkg-config, libxkbcommon-dev, libwayland-dev, libvulkan1 and
mesa-vulkan-drivers. Runtime X11/Wayland dependencies also include libx11-xcb1,
libxkbcommon-x11-0, libxcb1 and libdbus-1-3. File dialogs need a working
xdg-desktop-portal and the provider for the desktop (GNOME, KDE or GTK). No X11/Wayland server or GPU is provided by
these archives.

A Vulkan loader alone does not provide a graphics driver. Install a Vulkan ICD
(Mesa: `mesa-vulkan-drivers`) or use an EGL/OpenGL driver. On Debian/Ubuntu, Mesa
OpenGL uses `libegl-mesa0` and `libgl1-mesa-dri`. To select OpenGL explicitly:

```sh
WGPU_BACKEND=gl openradview
```

OpenGL drivers without compute support can display 2D/MPR but cannot render
volume/MIP. Software graphics can be slow at large window sizes. Startup prints
the selected backend, adapter and driver locally to stderr; no diagnostics are
sent anywhere. `openradview --help` prints usage without opening a window.
Native OpenGL runtime and affected-machine performance checks remain pending.

Fetch locked dependencies once, then package offline on the intended oldest host:

```sh
cargo fetch --locked --target x86_64-unknown-linux-gnu
python3 scripts/portable/bundle.py
```

The resulting tar.gz contains openradview, openradview-mcp, license/privacy/
disclaimer, font/dependency/dataset notices, provenance/hashes, exact tracked
source (clean builds), icon and a desktop-entry template. Run ./openradview from a
user-owned folder. The desktop template assumes binaries/icons have been placed
on the user's application path; packaging performs no system installation.
Required ELF libraries and maximum referenced GLIBC version are in build.json.
Build on the intended oldest supported distribution; do not infer compatibility
with older glibc from a newer host's build.

## Native Windows build

Use the pinned Rust toolchain, Visual Studio C++ Build Tools/Windows SDK, CMake and
Python3.11+ from a native developer shell. Target x86_64-pc-windows-msvc (ARM64
requires its own tools and validation). Fetch locked dependencies, then:

```powershell
cargo fetch --locked --target x86_64-pc-windows-msvc
python scripts/portable/bundle.py
```

The ZIP keeps the application and stdio companion together. The packager selects
the static MSVC runtime, inspects PE imports and refuses unknown non-system DLLs.
It does not silently produce an archive missing a compiler runtime. This archive
step does not install files or add an Authenticode signature. MinGW builds
can also be packaged with a statically linked C++ runtime, subject to the same
import audit. MSVC remains the preferred native release target; compilation alone does not establish runtime acceptance.

## Native installers

After creating a clean native archive, `python scripts/portable/installers.py
--target TARGET` produces the matching installer. On Windows, supply
`--compiler PATH_TO_ISCC_EXE` for the pinned Inno Setup compiler. The CI-only
`scripts/ci/inno.py` downloads it from the official release and verifies its exact
size and SHA-256. Installer names and update archives are defined in
[the target contract](../packaging/release-targets.json).

The Windows x64 Setup EXE installs for the current user under
`%LOCALAPPDATA%\Programs\OpenRadView`. The managed application is in its `app`
subfolder; uninstall metadata and shortcuts stay outside the folder replaced by
the updater. A desktop shortcut is optional. The installer does not launch the
viewer, add an account or modify the system PATH. Windows Authenticode signing
requires a separate certificate; Apple credentials do not sign Windows programs.

The Linux amd64 `.deb` targets Debian/Ubuntu systems with compatible runtime
libraries. Its dependencies are derived from the actual ELF programs, with the
desktop libraries and a Vulkan ICD declared explicitly; Mesa EGL/OpenGL drivers
are recommended. It installs the app under `/opt/openradview`, commands under `/usr/bin`, and the desktop entry and icon in
their standard locations. Its marker disables automatic GitHub checks and folder
replacement by the in-app updater. Manual checks remain available; download and
install later `.deb` versions with your package manager. The app does not add
an apt repository. The portable TAR.GZ keeps the writable-folder update path.

**Manual Windows and Linux packages** runs the native default/full test suites,
GPU tests, archive and installer builds, then a real unattended install/version
probe/uninstall check on each host. It runs only when dispatched, serializes the
two hosts, saves reusable caches on main and retains packages for one day.
These checks establish their recorded scope; interactive UI and production update
acceptance are separate from command-line probes. No checklist is filled by
assuming that compilation proves runtime behavior.

## Preview and release boundaries

--development creates a debug preview, permits an explicitly selected cross-target
and records dirty source. Optimized archives require clean committed source on a
matching host. Existing archives are never overwritten. Packaging rejects staged
symlinks, writes deterministic archive metadata and emits SHA256 checksums.
There is no upload, release creation, repository change or CI dispatch.

Normal packages include the shared Rust GitHub updater. Automatic checks are enabled by default, run about once a day and can be
turned off on first launch or in Privacy → Updates. A manual check is always available. Download/install and
restart require explicit actions. Signed manifests bind each exact platform
archive; only managed writable app folders can be replaced. Recognized external
package locations/receipts, read-only and modified app folders use external or
manual installation. Unknown package layouts are not universally detected; see
[the ownership checks](architecture.md#updates-and-release-integrity). macOS additionally checks
the same Developer ID team and Gatekeeper. Development/ad-hoc Mac builds cannot
use in-app installation. Native filesystem and installer tests do not establish an older-version
production update/relaunch roundtrip. See
[updater acceptance](validation.md) and
[the release guide](release-macos.md). The stdio connector has the same explicit session approval as the Mac app.
Native window controls remain enabled on Linux/Windows; macOS alone uses the full
black titlebar with traffic lights. Cmd/Ctrl hints match the platform modifier.

Before any platform release, run default/full tests, actual GPU rendering, native
file/folder/ZIP import, resize, crosshair/scroll/pan/zoom, label editing/export,
accessibility, consent/revoke, closing with unsaved labels and an update-install
roundtrip on that system. Missing native evidence stays explicit in release copy.

## Frozen release inputs

Build optimized portable archives on their native hosts from the exact same clean
source revision/version as the Mac candidate. Copy each archive and its unchanged
`.sha256` sidecar into the release working directory. Prepare and independently
sign its `update-TARGET.json` envelope with the owner-managed existing release
key, as described in the [release guide](release-macos.md). No key operation is
part of the packager or candidate checker.

Add each target explicitly when freezing, for example:

```sh
target/dmgbuild-venv/bin/python scripts/macos/release.py \
  --portable-target x86_64-pc-windows-msvc \
  --portable-target x86_64-unknown-linux-gnu
```

Preparation verifies exact source, per-file hashes, managed inventory, native
build metadata and signed envelope. Development/cross-build previews are refused.
No archive is extracted or executed by this checker. Missing native acceptance
is recorded as pending; publication stops before any GitHub mutation.

The native workflow writes `acceptance-TARGET.json` with actual suite counts,
installer hashes, the run URL and explicit limitations. Freezing checks the
record against the exact archive, installer and source; publication checks that
the corresponding native GitHub job succeeded. It publishes these packages as
previews without asserting unperformed interactive checks.

For full manual acceptance, use this [record template](../packaging/native-acceptance-template.json).
Mark each check true only after performing it and link the reviewed records.
Include no personal study data, paths, machine profile or credentials. A
production update/relaunch roundtrip must be tested separately on each OS.
Changed artifacts require fresh evidence; existing frozen candidates stay unchanged.
