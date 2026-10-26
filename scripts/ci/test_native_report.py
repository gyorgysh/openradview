# SPDX-License-Identifier: GPL-3.0-or-later
"""A green CI step must not hide missing, failed or skipped native coverage."""
from pathlib import Path
import tempfile
import unittest
import native_report


class NativeReports(unittest.TestCase):
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
