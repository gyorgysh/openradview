# SPDX-License-Identifier: GPL-3.0-or-later
"""Select unchanged native packages from this run's Actions artifacts."""
from pathlib import Path
import shutil
import sys

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / 'scripts/portable'))
import native_evidence
import release_checks

TARGETS = ('x86_64-pc-windows-msvc', 'x86_64-unknown-linux-gnu')


def stage(inputs, destination, version, revision):
    selected = []
    for target in TARGETS:
        folder = inputs / f'native-{target}-{revision}'
        archive_name, bundle = release_checks.update_protocol.names(version, target)
        archive = release_checks.local(folder, archive_name)
        if not 0 < archive.stat().st_size <= 512 * 1024**2:
            raise ValueError('Native input archive exceeds the release limit')
        hashes, metadata = release_checks.inventory(archive, bundle)
        build = metadata.get('build.json', {})
        if (build.get('source_commit') != revision or build.get('target') != target
                or build.get('build_host') != target or build.get('version') != version
                or build.get('source_dirty') is not False or build.get('profile') != 'release'
                or build.get('features') != ['mcp', 'updates']
                or build.get('updater') != release_checks.update_protocol.configuration()
                or build.get('files') != {n: h for n, h in hashes.items() if n != 'build.json'}
                or hashes.get('source.tar.gz') != release_checks.source_digest(revision)):
            raise ValueError('Native artifact source or host differs from the release')
        checksum = release_checks.local(folder, archive_name + '.sha256')
        archive_hash = release_checks.digest(archive)
        if checksum.read_text(encoding='utf-8') != archive_hash + '  ' + archive_name + '\n':
            raise ValueError('Native artifact checksum mismatch')
        names, installer_hash = native_evidence.installer(folder, target, version, revision, archive, hashes,
                                                         digest=release_checks.digest, local=release_checks.local)
        report = release_checks.local(folder, f'acceptance-{target}.json')
        native_evidence.ci_acceptance(native_evidence.read(report), target=target, version=version,
                                      revision=revision, archive_hash=archive_hash, installer_hash=installer_hash)
        for name in [archive_name, checksum.name, *names, report.name]:
            source = release_checks.local(folder, name)
            if (destination / name).exists() or (destination / name).is_symlink():
                raise ValueError('Refusing to replace an existing release input')
            selected.append((source, destination / name))
    for source, output in selected:
        shutil.copyfile(source, output)
