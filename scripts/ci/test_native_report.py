# SPDX-License-Identifier: GPL-3.0-or-later
"""A green CI step must not hide missing, failed or skipped native coverage."""
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
import native_report


class NativeReports(unittest.TestCase):
    def test_every_feature_configuration_must_have_complete_coverage(self):
        with patch.object(native_report, 'tests', side_effect=[100, 101, 125]) as read:
            self.assertEqual(native_report.test_counts(),
                             {'native-default': 100, 'native-shipping': 101, 'native-full': 125})
            self.assertEqual([call.kwargs['gpu'] for call in read.call_args_list], [False, False, True])
            self.assertEqual([call.args[0].parent.name for call in read.call_args_list],
                             ['native-default', 'native-shipping', 'native-full'])
        for counts in ([100, 99, 125], [100, 101, 100]):
            with self.subTest(counts=counts), patch.object(native_report, 'tests', side_effect=counts):
                with self.assertRaises(ValueError):
                    native_report.test_counts()
        with patch.object(native_report, 'tests', side_effect=ValueError('missing shipping report')):
            with self.assertRaises(ValueError):
                native_report.test_counts()

    def test_actual_cases_and_both_gpu_binaries_are_required(self):
        body = ('<testsuites><testsuite name="orv-updates">'
                '<testcase name="tests::native_process::running_executable_survives_failed_then_successful_swap_and_backup_cleanup"/>'
                '</testsuite><testsuite name="orv-render::gpu_golden"><testcase name="slice"/></testsuite>'
                '<testsuite name="orv-render::volume_gpu"><testcase name="volume"/></testsuite></testsuites>')
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / 'report.xml'
            path.write_text(body)
            self.assertEqual(native_report.tests(path, gpu=True), 3)
            for bad in (body.replace('volume_gpu', 'other'), body.replace('<testcase name="slice"/>', '<testcase name="slice"><skipped/></testcase>'),
                        body.replace('<testcase name="slice"/>', '<testcase name="slice"><failure/></testcase>'),
                        body.replace('name="orv-updates"', 'name="orv-updates" failures="1"'),
                        '<testsuites/>'):
                path.write_text(bad)
                with self.subTest(report=bad), self.assertRaises(ValueError):
                    native_report.tests(path, gpu=True)
