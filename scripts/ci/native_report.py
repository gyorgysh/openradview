#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Record actual native CI checks without asserting interactive acceptance."""
import argparse
import json
import os
from pathlib import Path
import sys
import xml.etree.ElementTree as ET

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / 'scripts/portable'))
import bundle
import installers


def tests(path, *, gpu):
    if not path.is_file() or path.stat().st_size > 8 * 1024**2:
        raise ValueError('Missing or oversized native test report')
    report = ET.fromstring(path.read_bytes())
    cases = report.findall('.//testcase')
    if not cases or any(report.findall('.//' + kind) for kind in ('failure', 'error', 'skipped', 'rerunFailure', 'flakyFailure')):
        raise ValueError('Native test report includes failures, retries or skipped tests')
    for suite in [report, *report.iter('testsuite')]:
        if any(int(suite.get(key, '0')) for key in ('failures', 'errors', 'skipped')):
            raise ValueError('Native suite did not pass completely')
    names = [case.get('name', '') for case in cases]
    if not any('running_executable_survives_failed_then_successful_swap' in name for name in names):
        raise ValueError('The loaded-executable update test did not run')
    if gpu:
        for binary in ('gpu_golden', 'volume_gpu'):
            if not any(binary in suite.get('name', '') and suite.findall('testcase') for suite in report.iter('testsuite')):
                raise ValueError('A required native GPU test binary did not run')
    return len(cases)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--target', choices=[installers.WINDOWS, installers.LINUX], required=True)
    args = parser.parse_args()
    if os.environ.get('GITHUB_ACTIONS') != 'true' or os.environ.get('GITHUB_REPOSITORY') != 'gyorgysh/openradview':
        raise ValueError('Expected the native OpenRadView CI context')
    outcomes = json.loads(os.environ.get('ORV_NATIVE_OUTCOMES', '{}'))
    required = {'format_clippy', 'default_tests', 'full_tests', 'installer_tests'}
    if set(outcomes) != required or set(outcomes.values()) != {'success'}:
        raise ValueError('Every native CI check must have succeeded')
    revision = bundle.run(['git', 'rev-parse', 'HEAD'])
    if revision != os.environ.get('GITHUB_SHA') or bundle.run(['git', 'status', '--porcelain', '--untracked-files=normal']):
        raise ValueError('Native CI source identity changed')
    counts = {profile: tests(ROOT / 'target/nextest' / profile / 'junit.xml', gpu=profile == 'native-full')
              for profile in ('native-default', 'native-full')}
    if counts['native-full'] < counts['native-default']:
        raise ValueError('Full native tests unexpectedly omitted default coverage')
    version = json.loads(bundle.run(['cargo', 'metadata', '--offline', '--locked', '--no-deps', '--format-version', '1']))
    version = next(p['version'] for p in version['packages'] if p['name'] == 'orv-app')
    archive, _ = bundle.update_protocol.names(version, args.target)
    installer = installers.name(version, args.target)
    smoke = json.loads((ROOT / 'dist' / f'installer-acceptance-{args.target}.json').read_text())
    for key, expected in {'source_revision': revision, 'target': args.target, 'version': version,
                          'archive_sha256': bundle.digest(ROOT / 'dist' / archive),
                          'installer_sha256': bundle.digest(ROOT / 'dist' / installer)}.items():
        if smoke.get(key) != expected:
            raise ValueError('Native installer evidence differs from the final package')
    smoke_checks = {'install', 'payload_hashes', 'executable_version_probes', 'wrong_version_rejected',
                    'uninstall', 'outside_files_preserved'}
    if set(smoke.get('checks', {})) != smoke_checks or any(v is not True for v in smoke['checks'].values()):
        raise ValueError('Native installation did not pass its recorded checks')
    run_id = os.environ['GITHUB_RUN_ID']
    if not run_id.isdigit():
        raise ValueError('Invalid native workflow identity')
    report = {'schema_version': 2, 'kind': 'ci-native-preview', 'target': args.target,
              'version': version, 'source_revision': revision, 'archive_sha256': smoke['archive_sha256'],
              'installer_sha256': smoke['installer_sha256'], 'test_counts': counts,
              'checks': {'format_clippy': True, 'default_tests': True, 'full_tests': True,
                         'native_gpu': True, 'loaded_executable_swap': True, 'installer_roundtrip': True},
              'run_id': int(run_id), 'evidence': f'https://github.com/gyorgysh/openradview/actions/runs/{run_id}',
              'limitations': ['Interactive desktop, accessibility and GPU performance on consumer hardware require separate checks.',
                              'A production upgrade from an older published version is not established by these installer and filesystem tests.']}
    destination = ROOT / 'dist' / f'acceptance-{args.target}.json'
    with destination.open('x') as output:
        json.dump(report, output, sort_keys=True, indent=2)
        output.write('\n')
    print('Recorded native test and installer evidence with explicit scope')


if __name__ == '__main__':
    main()
