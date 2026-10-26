# SPDX-License-Identifier: GPL-3.0-or-later
"""Validate artifact-bound native installer and CI evidence without claiming UI acceptance."""
import json
from pathlib import Path
import re

ROOT = Path(__file__).resolve().parents[2]
SMOKE_CHECKS = {'install', 'payload_hashes', 'executable_version_probes', 'wrong_version_rejected',
                'uninstall', 'outside_files_preserved'}
CI_CHECKS = {'format_clippy', 'default_tests', 'full_tests', 'native_gpu',
             'loaded_executable_swap', 'installer_roundtrip'}
LIMITATIONS = [
    'Interactive desktop, accessibility and GPU performance on consumer hardware require separate checks.',
    'A production upgrade from an older published version is not established by these installer and filesystem tests.',
]


def installer_name(version, target):
    catalog = json.loads((ROOT / 'packaging/release-targets.json').read_text(encoding='utf-8'))
    return next((p['installer'].format(version=version) for p in catalog['targets']
                 if p['target'] == target and p['platform'] in ('windows', 'linux')), None)


def installer_names(version, target):
    name = installer_name(version, target)
    return [name, name + '.json', name + '.sha256', f'installer-acceptance-{target}.json'] if name else []


def read(path):
    if path.is_symlink() or not path.is_file() or path.stat().st_size > 128 * 1024:
        raise ValueError('Missing or oversized native evidence')
    return json.loads(path.read_text(encoding='utf-8'))


def installer(folder, target, version, revision, archive, hashes, *, digest, local):
    names = installer_names(version, target)
    if not names:
        raise ValueError('No installer is configured for the release target')
    binary = local(folder, names[0])
    if not 0 < binary.stat().st_size <= 512 * 1024**2:
        raise ValueError('Installer size is outside the release limit')
    installer_hash, archive_hash = digest(binary), digest(archive)
    record = read(local(folder, names[1]))
    expected = {'schema_version': 1, 'version': version, 'source_revision': revision, 'target': target,
                'installer': binary.name, 'installer_sha256': installer_hash,
                'archive': archive.name, 'archive_sha256': archive_hash, 'payload_files': hashes}
    if record != expected:
        raise ValueError('Installer record differs from the exact native archive and source')
    if local(folder, names[2]).read_text(encoding='utf-8') != installer_hash + '  ' + binary.name + '\n':
        raise ValueError('Installer checksum mismatch')
    smoke = read(local(folder, names[3]))
    for key in ('target', 'version', 'source_revision', 'archive_sha256', 'installer_sha256'):
        if smoke.get(key) != expected[key]:
            raise ValueError('Installer acceptance belongs to different artifacts')
    if set(smoke.get('checks', {})) != SMOKE_CHECKS or any(v is not True for v in smoke['checks'].values()):
        raise ValueError('Installer acceptance did not pass every required check')
    return names, installer_hash


def ci_acceptance(record, *, target, version, revision, archive_hash, installer_hash):
    expected = {'schema_version': 2, 'kind': 'ci-native-preview', 'target': target,
                'version': version, 'source_revision': revision,
                'archive_sha256': archive_hash, 'installer_sha256': installer_hash}
    if any(record.get(k) != v for k, v in expected.items()):
        raise ValueError('Native CI evidence belongs to different source or packages')
    if set(record.get('checks', {})) != CI_CHECKS or any(v is not True for v in record['checks'].values()):
        raise ValueError('Native CI evidence is incomplete')
    counts = record.get('test_counts', {})
    if (set(counts) != {'native-default', 'native-full'}
            or any(type(n) is not int or n < 1 for n in counts.values())
            or counts['native-full'] <= counts['native-default']):
        raise ValueError('Native CI test counts are invalid')
    run_id = record.get('run_id')
    if (type(run_id) is not int or run_id < 1
            or record.get('evidence') != f'https://github.com/gyorgysh/openradview/actions/runs/{run_id}'
            or record.get('limitations') != LIMITATIONS):
        raise ValueError('Native CI evidence must retain its actual run and limitations')


def verify_run(record, github):
    """Check the successful target job, even while its parent release is in progress."""
    run_id, target, revision = record['run_id'], record['target'], record['source_revision']
    run = github(f'actions/runs/{run_id}')
    if (run.get('head_sha') != revision or run.get('run_attempt') != 1
            or run.get('event') not in ('push', 'workflow_dispatch')
            or run.get('path') not in ('.github/workflows/portable.yml', '.github/workflows/release.yml')
            or run.get('head_repository', {}).get('full_name', '').lower() != 'gyorgysh/openradview'):
        raise ValueError('Native evidence does not identify the expected source workflow')
    jobs = github(f'actions/runs/{run_id}/jobs?per_page=100')
    selected = [job for job in jobs.get('jobs', []) if re.search(r'package \([^()]+, ' + re.escape(target) + r'\)$', job.get('name', ''))]
    if jobs.get('total_count', 101) > 100 or len(selected) != 1 or selected[0].get('conclusion') != 'success':
        raise ValueError('Native target CI job did not complete successfully')
