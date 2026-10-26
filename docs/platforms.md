# Desktop platforms and local archives

| Target | Current evidence | Distribution status |
| --- | --- | --- |
| Apple Silicon macOS13+ | Native UI, Metal, tests and app-bundle checks | Local app builds; actual release signing/notarization/publication are separate |
| x86_64 Linux GNU | Full desktop/stdio companion cross-check and link on macOS with Zig0.17; ELF dependency inspection | Build preview; native UI, portal, Vulkan, accessibility and install checks pending |
| x86_64 Windows GNU | Full desktop/stdio companion cross-check and link with MinGW-w64/GCC16.2 | Compiler preview; native runtime and MSVC package checks pending |
| Windows MSVC / Linux aarch64 | Source/build tooling prepared | Not built or run on a matching host here |

A successful cross-build is not a native runtime test. macOS remains the tested
release platform. No Windows/Linux signing, installation or updater roundtrip is
claimed. ARM builds need separate checks. Development CI stays manual: optional cross-platform jobs are serialized and cached.
The separate [release workflow](github-release.md) uses owner dispatch while private
and reviewer approval after becoming public.

## Native Linux build

Use Rust from rust-toolchain.toml, a C/C++ compiler, CMake, pkg-config, Python3.11+
and binutils/readelf. On Ubuntu24.04 the native build/runtime packages include
build-essential, cmake, pkg-config, libxkbcommon-dev, libwayland-dev, libvulkan1 and
mesa-vulkan-drivers. File dialogs need a working xdg-desktop-portal and the provider
for the desktop (GNOME, KDE or GTK). No X11/Wayland server or GPU is provided by
these archives.

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
It does not silently produce an archive missing a compiler runtime. No installer,
registry change, elevation or Authenticode signature is performed. MinGW builds
can also be packaged with a statically linked C++ runtime, subject to the same
import audit. MSVC remains the preferred native release target; neither compiler
preview establishes runtime acceptance.

## Preview and release boundaries

--development creates a debug preview, permits an explicitly selected cross-target
and records dirty source. Optimized archives require clean committed source on a
matching host. Existing archives are never overwritten. Packaging rejects staged
symlinks, writes deterministic archive metadata and emits SHA256 checksums.
There is no upload, release creation, repository change or CI dispatch.

Normal packages include the shared Rust GitHub updater. A saved first-run choice
controls daily checks, with a manual check always available. Download/install and
restart require explicit actions. Signed manifests bind each exact platform
archive; only managed writable app folders can be replaced. Recognized external
package locations/receipts, read-only and modified app folders use external or
manual installation. Unknown package layouts are not universally detected; see
[the ownership checks](architecture.md#updates-and-release-integrity). macOS additionally checks
the same Developer ID team and Gatekeeper. Development/ad-hoc Mac builds cannot
use in-app installation. This source and cross-build evidence does not establish
a native Windows/Linux update roundtrip or a public signed release. See
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
is recorded as pending; draft/publication stops before any GitHub mutation.

After performing the native checks, place `acceptance-TARGET.json` beside the
archive using this [record template](../packaging/native-acceptance-template.json).
Fill the exact target, version, full source revision and final archive SHA-256;
mark a check true only after it passed and link the reviewed test records in
`evidence`. Include no personal study data, paths, machine profile or credentials.
This is a reviewer-maintained evidence record, not automated proof of UI/GPU
behavior. Keep update acceptance pending until a real installation/relaunch/
backup-cleanup roundtrip has been completed on that OS. The production feed is
unavailable anonymously while this repository is private; this checker adds no
alternate feed or credential bypass. Changing an archive requires fresh
acceptance and a new frozen candidate; existing candidates stay unchanged.
