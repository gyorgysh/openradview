# SPDX-License-Identifier: GPL-3.0-or-later
import copy
import unittest

import native_evidence as evidence


class NativeEvidenceTests(unittest.TestCase):
    def test_ci_record_keeps_actual_scope_hashes_and_successful_native_job(self):
        expected = {'target': 'x86_64-pc-windows-msvc', 'version': '1.2.3', 'revision': 'a' * 40,
                    'archive_hash': 'b' * 64, 'installer_hash': 'c' * 64}
        record = {'schema_version': 2, 'kind': 'ci-native-preview', 'target': expected['target'],
                  'version': '1.2.3', 'source_revision': expected['revision'],
                  'archive_sha256': expected['archive_hash'], 'installer_sha256': expected['installer_hash'],
                  'test_counts': {'native-default': 178, 'native-full': 212},
                  'checks': dict.fromkeys(evidence.CI_CHECKS, True), 'run_id': 123,
                  'evidence': 'https://github.com/gyorgysh/openradview/actions/runs/123',
                  'limitations': evidence.LIMITATIONS}
        evidence.ci_acceptance(record, **expected)
        for field, value in [('archive_sha256', 'd' * 64), ('installer_sha256', 'd' * 64),
                             ('source_revision', 'd' * 40), ('limitations', []),
                             ('test_counts', {'native-default': 1, 'native-full': 1}),
                             ('checks', {**record['checks'], 'native_gpu': False}),
                             ('evidence', 'https://example.com'), ('run_id', True)]:
            bad = copy.deepcopy(record); bad[field] = value
            with self.subTest(field=field), self.assertRaises(ValueError):
                evidence.ci_acceptance(bad, **expected)
        run = {'head_sha': expected['revision'], 'run_attempt': 1, 'event': 'push',
               'path': '.github/workflows/release.yml', 'head_repository': {'full_name': 'gyorgysh/openradview'}}
        job = {'name': 'native / package (windows-2025, x86_64-pc-windows-msvc)', 'conclusion': 'success'}
        def github(path):
            return {'total_count': 4, 'jobs': [job]} if '/jobs?' in path else run
        evidence.verify_run(record, github)
        for result in ('failure', 'skipped', None):
            job['conclusion'] = result
            with self.assertRaises(ValueError): evidence.verify_run(record, github)
        job['conclusion'] = 'success'; run['head_sha'] = 'f' * 40
        with self.assertRaises(ValueError): evidence.verify_run(record, github)


if __name__ == '__main__':
    unittest.main()
