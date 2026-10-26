#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Wrap a verified native archive in a per-user Windows or Debian installer."""
import argparse
import json
from pathlib import Path
import platform
import shutil
import subprocess
import tarfile
import tempfile
import zipfile

import bundle
import release_checks

ROOT = Path(__file__).resolve().parents[2]
WINDOWS = 'x86_64-pc-windows-msvc'
LINUX = 'x86_64-unknown-linux-gnu'


def name(version, target):
    catalog = json.loads((ROOT / 'packaging/release-targets.json').read_text())
    for entry in catalog['targets']:
        if entry['target'] == target and target in (WINDOWS, LINUX):
            return entry['installer'].format(version=version)
    raise ValueError('No installer is configured for this target')


def unpack(archive, destination, version, target, revision):
    expected, stem = bundle.update_protocol.names(version, target)
    if archive.name != expected or archive.is_symlink() or not 0 < archive.stat().st_size <= 512 * 1024**2:
        raise ValueError('Installer input must be the exact bounded native update archive')
    hashes, metadata = release_checks.inventory(archive, stem)
    build = metadata.get('build.json', {})
    if (build.get('target') != target or build.get('build_host') != target
            or build.get('source_commit') != revision or build.get('version') != version
            or build.get('source_dirty') is not False or build.get('profile') != 'release'
            or build.get('features') != ['mcp', 'updates']
            or build.get('files') != {n: h for n, h in hashes.items() if n != 'build.json'}):
        raise ValueError('Installer requires an exact clean optimized native-host payload')
    suffix = '.exe' if target == WINDOWS else ''
    required = {'openradview' + suffix, 'openradview-mcp' + suffix, 'LICENSE',
                'PRIVACY.md', 'DISCLAIMER.md', 'THIRD-PARTY-NOTICES.txt', 'source.tar.gz',
                'build.json', 'update-layout.json'}
    if not required <= set(hashes) or hashes['source.tar.gz'] != release_checks.source_digest(revision):
        raise ValueError('Installer payload source or notices differ from the release')
    destination.mkdir()
    transferred = 0

    def save(relative, source):
        nonlocal transferred
        if relative not in hashes:
            raise ValueError('Installer input paths changed during extraction')
        path = destination / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        with path.open('xb') as output:
            while chunk := source.read(512 * 1024):
                transferred += len(chunk)
                if transferred > 2 * 1024**3:
                    raise ValueError('Installer payload exceeds its extraction limit')
                output.write(chunk)
        if bundle.digest(path) != hashes[relative]:
            raise ValueError('Installer input changed during extraction')
        path.chmod(0o755 if relative in ('openradview' + suffix, 'openradview-mcp' + suffix) else 0o644)

    if target == WINDOWS:
        with zipfile.ZipFile(archive) as source:
            for item in source.infolist():
                relative = item.filename.removeprefix(stem + '/')
                with source.open(item) as content:
                    save(relative, content)
    else:
        with tarfile.open(archive, 'r:gz') as source:
            for item in source:
                if not item.isreg():
                    raise ValueError('Installer input must contain regular files only')
                relative = item.name.removeprefix(stem + '/')
                with source.extractfile(item) as content:
                    save(relative, content)
    return hashes


def windows(payload, output, version, compiler):
    if platform.system() != 'Windows' or not compiler:
        raise ValueError('Use the native Windows host and pinned Inno Setup compiler')
    subprocess.run([str(compiler), '/Qp', f'/DAppVersion={version}',
                    f'/DPayloadDir={payload}', f'/DOutputDir={output.parent}',
                    f'/DOutputBase={output.stem}',
                    f'/DBrandIcon={ROOT / "packaging/windows/OpenRadView.ico"}',
                    f'/DBrandImage={ROOT / "assets/brand/app-icon.png"}',
                    f'/DLicenseFile={ROOT / "LICENSE"}',
                    str(ROOT / 'packaging/windows/openradview.iss')], check=True)


def debian(payload, output, version, work):
    if platform.system() != 'Linux':
        raise ValueError('Build Debian installers on the native Linux host')
    package = work / 'debian-package'
    app = package / 'opt/openradview'
    app.parent.mkdir(parents=True)
    shutil.copytree(payload, app)
    (app / '.openradview-no-self-update').write_text('Updates are installed with the Debian package manager.\n')
    desktop = package / 'usr/share/applications/openradview.desktop'
    desktop.parent.mkdir(parents=True)
    shutil.copyfile(ROOT / 'packaging/linux/openradview.desktop', desktop)
    icon = package / 'usr/share/icons/hicolor/512x512/apps/openradview.png'
    icon.parent.mkdir(parents=True)
    shutil.copyfile(ROOT / 'assets/brand/app-icon.png', icon)
    documentation = package / 'usr/share/doc/openradview'
    documentation.mkdir(parents=True)
    shutil.copyfile(ROOT / 'LICENSE', documentation / 'copyright')
    commands = package / 'usr/bin'
    commands.mkdir(parents=True)
    for binary in ('openradview', 'openradview-mcp'):
        launcher = commands / binary
        launcher.write_text('#!/bin/sh\n# SPDX-License-Identifier: GPL-3.0-or-later\n'
                            f'exec /opt/openradview/{binary} "$@"\n')
        launcher.chmod(0o755)
    controls = work / 'debian'
    controls.mkdir()
    (controls / 'control').write_text('Source: openradview\nSection: science\nPriority: optional\n'
                                    'Maintainer: gyorgysh <gyorgy@pueev.com>\n\n'
                                    'Package: openradview\nArchitecture: amd64\nDescription: Personal image exploration\n')
    needed = subprocess.check_output(['dpkg-shlibdeps', '-O', '-e' + str(app / 'openradview'),
                                      '-e' + str(app / 'openradview-mcp')], cwd=work, text=True)
    dependencies = next((s.removeprefix('shlibs:Depends=') for s in needed.splitlines()
                         if s.startswith('shlibs:Depends=')), '')
    if not dependencies or '\n' in dependencies:
        raise ValueError('Could not determine Debian runtime dependencies')
    dependencies += (', libxkbcommon0, libwayland-client0, libvulkan1, libegl1, libx11-6, '
                     'libxcursor1, libxi6, libxrandr2, libx11-xcb1, libxkbcommon-x11-0, '
                     'libxcb1, libdbus-1-3, mesa-vulkan-drivers | vulkan-icd')
    control = package / 'DEBIAN'
    control.mkdir()
    size = sum(p.stat().st_size for p in package.rglob('*') if p.is_file())
    (control / 'control').write_text(
        f'Package: openradview\nVersion: {version}\nArchitecture: amd64\nSection: science\nPriority: optional\n'
        f'Maintainer: gyorgysh <gyorgy@pueev.com>\nInstalled-Size: {(size + 1023) // 1024}\n'
        f'Depends: {dependencies}\nRecommends: xdg-desktop-portal, libegl-mesa0, libgl1-mesa-dri\n'
        'Homepage: https://openradview.com\n'
        'Description: Local viewer for your own medical images\n'
        ' Open DICOM and NIfTI images for personal education and research.\n'
        ' Not for diagnosis, treatment or decisions about care.\n')
    subprocess.run(['desktop-file-validate', str(desktop)], check=True)
    subprocess.run(['dpkg-deb', '--root-owner-group', '--build', str(package), str(output)], check=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('archive', type=Path, nargs='?')
    parser.add_argument('--target', choices=[WINDOWS, LINUX], required=True)
    parser.add_argument('--compiler', type=Path)
    args = parser.parse_args()
    version = bundle.run(['cargo', 'metadata', '--locked', '--offline', '--no-deps', '--format-version', '1'])
    version = next(p['version'] for p in json.loads(version)['packages'] if p['name'] == 'orv-app')
    archive = (args.archive or ROOT / 'dist' / bundle.update_protocol.names(version, args.target)[0]).resolve(strict=True)
    revision = bundle.run(['git', 'rev-parse', 'HEAD'])
    if bundle.run(['git', 'status', '--porcelain', '--untracked-files=normal']):
        raise ValueError('Installer source must be clean and committed')
    host = next(s.removeprefix('host: ') for s in bundle.run(['rustc', '-vV']).splitlines() if s.startswith('host: '))
    if host != args.target:
        raise ValueError('Installer must be built on its matching native host')
    output = archive.parent / name(version, args.target)
    if output.exists() or output.with_suffix(output.suffix + '.json').exists():
        raise ValueError('Refusing to overwrite a versioned installer')
    with tempfile.TemporaryDirectory(prefix='.orv-installer-', dir=archive.parent) as temporary:
        work = Path(temporary)
        hashes = unpack(archive, work / 'payload', version, args.target, revision)
        staged = work / output.name
        if args.target == WINDOWS:
            windows(work / 'payload', staged, version, args.compiler)
        else:
            debian(work / 'payload', staged, version, work)
        if not staged.is_file() or not 0 < staged.stat().st_size <= 512 * 1024**2:
            raise ValueError('Installer was not produced within its size limit')
        staged.rename(output)
    record = {'schema_version': 1, 'version': version, 'target': args.target,
              'source_revision': revision, 'installer': output.name, 'installer_sha256': bundle.digest(output),
              'archive': archive.name, 'archive_sha256': bundle.digest(archive), 'payload_files': hashes}
    output.with_suffix(output.suffix + '.json').write_text(json.dumps(record, sort_keys=True, indent=2) + '\n')
    output.with_suffix(output.suffix + '.sha256').write_text(record['installer_sha256'] + '  ' + output.name + '\n')
    print('Built native installer:', output)


if __name__ == '__main__':
    main()
