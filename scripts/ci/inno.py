#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Install the exact official compiler into this ephemeral Windows runner."""
import hashlib
import json
import os
from pathlib import Path
import platform
import subprocess
import tempfile
import urllib.request

ROOT = Path(__file__).resolve().parents[2]


def main():
    if platform.system() != 'Windows' or os.environ.get('GITHUB_ACTIONS') != 'true':
        raise ValueError('This tool setup is for the Windows CI runner only')
    config = json.loads((ROOT / 'packaging/windows/inno-tool.json').read_text())
    compiler = ROOT / 'target/inno/ISCC.exe'
    compiler.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='orv-inno-', dir=os.environ['RUNNER_TEMP']) as temporary:
        installer = Path(temporary) / 'inno.exe'
        request = urllib.request.Request(config['url'], headers={'User-Agent': 'OpenRadView-build'})
        with urllib.request.urlopen(request, timeout=60) as response, installer.open('xb') as output:
            size = 0
            while chunk := response.read(512 * 1024):
                size += len(chunk)
                if size > config['bytes']:
                    raise ValueError('Compiler download exceeded its pinned size')
                output.write(chunk)
        if size != config['bytes'] or hashlib.sha256(installer.read_bytes()).hexdigest() != config['sha256']:
            raise ValueError('Compiler download does not match its pinned checksum')
        subprocess.run([str(installer), '/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART',
                        '/NOICONS', '/DIR=' + str(compiler.parent)], check=True)
    if not compiler.is_file():
        raise ValueError('The pinned compiler was not installed')
    with open(os.environ['GITHUB_ENV'], 'a', encoding='utf-8') as output:
        output.write('ORV_INNO_COMPILER=' + str(compiler) + '\n')
    print('Installed hash-pinned Inno Setup', config['version'])


if __name__ == '__main__':
    main()
