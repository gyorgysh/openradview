#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Install/probe/uninstall our exact package on its disposable native CI host."""
import argparse
import json
import os
from pathlib import Path
import platform
import subprocess
import tempfile
import time

import bundle
import installers


def verify_files(root, expected, *, marker=False):
    actual = {p.relative_to(root).as_posix(): bundle.digest(p) for p in root.rglob('*') if p.is_file()}
    if marker:
        if '.openradview-no-self-update' not in actual:
            raise ValueError('Debian installation is missing its package-manager marker')
        actual.pop('.openradview-no-self-update')
    if actual != expected:
        raise ValueError('Installed files differ from the exact native payload')


def probe(root, version, target):
    suffix = '.exe' if target == installers.WINDOWS else ''
    for binary in ('openradview', 'openradview-mcp'):
        executable = root / (binary + suffix)
        subprocess.run([str(executable), '--update-health-check', version], timeout=15, check=True)
        wrong = subprocess.run([str(executable), '--update-health-check', '99999.0.0'], timeout=15)
        if wrong.returncode == 0:
            raise ValueError('Installed executable accepted the wrong update version')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--target', choices=[installers.WINDOWS, installers.LINUX], required=True)
    args = parser.parse_args()
    if os.environ.get('GITHUB_ACTIONS') != 'true':
        raise ValueError('Native installation smoke tests run only on disposable CI hosts')
    expected_os = 'Windows' if args.target == installers.WINDOWS else 'Linux'
    if platform.system() != expected_os:
        raise ValueError('Installer smoke test requires the matching native host')
    version = json.loads(bundle.run(['cargo', 'metadata', '--offline', '--locked', '--no-deps', '--format-version', '1']))
    version = next(p['version'] for p in version['packages'] if p['name'] == 'orv-app')
    package = bundle.ROOT / 'dist' / installers.name(version, args.target)
    record = json.loads(package.with_suffix(package.suffix + '.json').read_text())
    revision = bundle.run(['git', 'rev-parse', 'HEAD'])
    if (record['version'] != version or record['target'] != args.target or record['source_revision'] != revision
            or record['installer_sha256'] != bundle.digest(package)):
        raise ValueError('Installer identity or digest differs from this source')
    with tempfile.TemporaryDirectory(prefix='orv-install-check-') as temporary:
        work = Path(temporary)
        sentinel = work / 'user-data-outside-installation'
        sentinel.write_bytes(b'preserve personal files')
        if args.target == installers.WINDOWS:
            installed = work / 'installation'
            subprocess.run([str(package), '/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART',
                            '/NOICONS', '/DIR=' + str(installed)], timeout=120, check=True)
            try:
                verify_files(installed / 'app', record['payload_files'])
                probe(installed / 'app', version, args.target)
            finally:
                uninstallers = list(installed.glob('unins*.exe'))
                if len(uninstallers) != 1:
                    raise ValueError('Installer did not create exactly one uninstaller')
                subprocess.run([str(uninstallers[0]), '/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART'],
                               timeout=120, check=True)
                deadline = time.monotonic() + 30
                while installed.exists() and any(installed.rglob('*')) and time.monotonic() < deadline:
                    time.sleep(0.1)
                if installed.exists() and any(installed.rglob('*')):
                    raise ValueError('Uninstaller left installed application files')
        else:
            installed = Path('/opt/openradview')
            if installed.exists():
                raise ValueError('Refusing to replace an existing Linux installation')
            subprocess.run(['sudo', 'env', 'DEBIAN_FRONTEND=noninteractive', 'apt-get', 'install', '--yes', str(package)],
                           timeout=180, check=True)
            try:
                verify_files(installed, record['payload_files'], marker=True)
                probe(installed, version, args.target)
                subprocess.run(['/usr/bin/openradview', '--update-health-check', version], timeout=15, check=True)
                subprocess.run(['/usr/bin/openradview-mcp', '--update-health-check', version], timeout=15, check=True)
            finally:
                subprocess.run(['sudo', 'dpkg', '--remove', 'openradview'], timeout=120, check=True)
            if installed.exists() and any(installed.rglob('*')):
                raise ValueError('Package removal left installed application files')
        if sentinel.read_bytes() != b'preserve personal files':
            raise ValueError('Uninstaller changed files outside its installation')
    report = {'schema_version': 1, 'target': args.target, 'version': version, 'source_revision': revision,
              'archive_sha256': record['archive_sha256'], 'installer_sha256': record['installer_sha256'],
              'checks': {'install': True, 'payload_hashes': True, 'executable_version_probes': True,
                         'wrong_version_rejected': True, 'uninstall': True, 'outside_files_preserved': True},
              'scope': 'Native installer and command-line probes. Does not establish interactive UI or a production update roundtrip.'}
    destination = package.parent / f'installer-acceptance-{args.target}.json'
    with destination.open('x') as output:
        json.dump(report, output, indent=2, sort_keys=True)
        output.write('\n')
    print('Native installer install/probe/uninstall checks passed')


if __name__ == '__main__':
    main()
