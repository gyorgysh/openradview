#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Check published signed updates on the matching disposable native host."""
import argparse
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / 'scripts'))
import update_protocol

TARGETS = ('x86_64-pc-windows-msvc', 'x86_64-unknown-linux-gnu')
TRANSIENT_ERRORS = {
    'Error: "GitHub could not provide the update. Try again later."',
    'Error: "Cannot reach GitHub for updates."',
    'Error: "Cannot download the update from GitHub."',
}


def verify_live(command, environment):
    for attempt in range(3):
        result = subprocess.run(command, cwd=ROOT, env=environment, text=True,
                                capture_output=True, timeout=1200)
        if result.stdout:
            print(result.stdout, end='')
        if result.stderr:
            print(result.stderr, end='', file=sys.stderr)
        if result.returncode == 0:
            return
        if attempt == 2 or not TRANSIENT_ERRORS.intersection((result.stderr or '').splitlines()):
            raise subprocess.CalledProcessError(result.returncode, command,
                                                output=result.stdout, stderr=result.stderr)
        print('Transient GitHub transport failure; retrying the live check.', file=sys.stderr)
        time.sleep(5 * (attempt + 1))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--target', choices=TARGETS, required=True)
    args = parser.parse_args()
    if os.environ.get('GITHUB_ACTIONS') != 'true':
        raise ValueError('Run published native checks only on a disposable CI host')
    with tempfile.TemporaryDirectory(prefix='orv-published-update-') as temporary:
        root = Path(temporary)
        paths = []
        for version in ('1.0.0', '1.1.0'):
            folder = root / version
            archive, _ = update_protocol.names(version, args.target)
            envelope = f'update-{args.target}.json'
            subprocess.run(['gh', 'release', 'download', f'v{version}', '--repo',
                            'gyorgysh/openradview', '--pattern', archive,
                            '--pattern', envelope, '--dir', str(folder)],
                           cwd=ROOT, check=True, timeout=900)
            paths.extend([str(folder / archive), str(folder / envelope)])
        environment = os.environ.copy()
        environment.pop('GH_TOKEN', None)
        environment.pop('GITHUB_TOKEN', None)
        command = ['cargo', 'run', '--locked', '--offline', '--target', args.target,
                   '-p', 'orv-updates', '--features', 'release-validation',
                   '--example', 'verify_release_update', '--']
        # Local integrity/swap failures fail once. Only the separate, unauthenticated
        # live transport probe can retry; it still verifies the signed archive.
        subprocess.run([*command, *paths],
                       cwd=ROOT, env=environment, check=True, timeout=1200)
        verify_live([*command, '--live-only', paths[1], paths[3]], environment)


if __name__ == '__main__':
    main()
