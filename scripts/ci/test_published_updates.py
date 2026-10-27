# SPDX-License-Identifier: GPL-3.0-or-later
import os
import unittest
from unittest.mock import patch

import published_updates


class PublishedUpdateTests(unittest.TestCase):
    def test_each_target_uses_both_signed_releases_and_no_token_for_the_updater(self):
        for target in published_updates.TARGETS:
            calls = []
            def run(args, **kwargs):
                calls.append((args, kwargs))
            with self.subTest(target=target), patch.dict(os.environ, {
                    'GITHUB_ACTIONS': 'true', 'GH_TOKEN': 'synthetic', 'GITHUB_TOKEN': 'synthetic'
                }), patch('sys.argv', ['published_updates.py', '--target', target]), \
                    patch.object(published_updates.subprocess, 'run', side_effect=run):
                published_updates.main()
            self.assertEqual(len(calls), 3)
            self.assertIn('v1.0.0', calls[0][0])
            self.assertIn('v1.1.0', calls[1][0])
            self.assertEqual(calls[2][0][0], 'cargo')
            self.assertIn('--live', calls[2][0])
            self.assertNotIn('GH_TOKEN', calls[2][1]['env'])
            self.assertNotIn('GITHUB_TOKEN', calls[2][1]['env'])

    def test_non_ci_use_is_refused(self):
        with patch.dict(os.environ, {'GITHUB_ACTIONS': 'false'}), \
                patch('sys.argv', ['published_updates.py', '--target', published_updates.TARGETS[0]]):
            with self.assertRaises(ValueError):
                published_updates.main()
