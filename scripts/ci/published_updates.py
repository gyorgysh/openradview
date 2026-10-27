#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Check published signed updates on the matching disposable native host."""
import argparse
import os
from pathlib import Path
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / 'scripts'))
import update_protocol

TARGETS = ('x86_64-pc-windows-msvc', 'x86_64-unknown-linux-gnu')


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
        subprocess.run(['cargo', 'run', '--locked', '--offline', '--target', args.target,
                        '-p', 'orv-updates', '--features', 'release-validation',
                        '--example', 'verify_release_update', '--', *paths, '--live'],
                       cwd=ROOT, env=environment, check=True, timeout=1200)


if __name__ == '__main__':
    main()
