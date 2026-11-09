# SPDX-License-Identifier: GPL-3.0-or-later
import os
import subprocess
import unittest
from unittest.mock import patch

import published_updates


class PublishedUpdateTests(unittest.TestCase):
    def test_each_target_uses_both_signed_releases_and_no_token_for_the_updater(self):
        for target in published_updates.TARGETS:
            calls = []
            def run(args, **kwargs):
                calls.append((args, kwargs))
                return subprocess.CompletedProcess(args, 0, '', '')
            with self.subTest(target=target), patch.dict(os.environ, {
                    'GITHUB_ACTIONS': 'true', 'GH_TOKEN': 'synthetic', 'GITHUB_TOKEN': 'synthetic'
                }), patch('sys.argv', ['published_updates.py', '--target', target]), \
                    patch.object(published_updates.subprocess, 'run', side_effect=run):
                published_updates.main()
            self.assertEqual(len(calls), 4)
            self.assertIn('v1.0.0', calls[0][0])
            self.assertIn('v1.1.0', calls[1][0])
            self.assertEqual(calls[2][0][0], 'cargo')
            self.assertNotIn('--live', calls[2][0])
            self.assertIn('--live-only', calls[3][0])
            for _, options in calls[2:]:
                self.assertNotIn('GH_TOKEN', options['env'])
                self.assertNotIn('GITHUB_TOKEN', options['env'])

    def test_live_transport_failure_retries_then_requires_success(self):
        failure = subprocess.CompletedProcess(['probe'], 1, '', next(iter(published_updates.TRANSIENT_ERRORS)))
        success = subprocess.CompletedProcess(['probe'], 0, 'verified\n', '')
        with patch.object(published_updates.subprocess, 'run', side_effect=[failure, success]) as run, \
                patch.object(published_updates.time, 'sleep') as sleep:
            published_updates.verify_live(['probe'], {})
        self.assertEqual(run.call_count, 2)
        sleep.assert_called_once_with(5)

    def test_integrity_failure_is_never_retried(self):
        failure = subprocess.CompletedProcess(['probe'], 1, '', 'Error: "Invalid update signature."')
        with patch.object(published_updates.subprocess, 'run', return_value=failure) as run, \
                patch.object(published_updates.time, 'sleep') as sleep:
            with self.assertRaises(subprocess.CalledProcessError):
                published_updates.verify_live(['probe'], {})
        self.assertEqual(run.call_count, 1)
        sleep.assert_not_called()

    def test_persistent_transport_failure_still_fails_after_three_attempts(self):
        failure = subprocess.CompletedProcess(['probe'], 1, '', next(iter(published_updates.TRANSIENT_ERRORS)))
        with patch.object(published_updates.subprocess, 'run', return_value=failure) as run, \
                patch.object(published_updates.time, 'sleep') as sleep:
            with self.assertRaises(subprocess.CalledProcessError):
                published_updates.verify_live(['probe'], {})
        self.assertEqual(run.call_count, 3)
        self.assertEqual(sleep.call_count, 2)

    def test_non_ci_use_is_refused(self):
        with patch.dict(os.environ, {'GITHUB_ACTIONS': 'false'}), \
                patch('sys.argv', ['published_updates.py', '--target', published_updates.TARGETS[0]]):
            with self.assertRaises(ValueError):
                published_updates.main()
