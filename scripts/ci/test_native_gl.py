# SPDX-License-Identifier: GPL-3.0-or-later
import copy
import unittest
from native_gl import validate


class NativeGlTests(unittest.TestCase):
    def report(self):
        return {"schema_version": 1, "version": "1.1.0",
                "build": "developer qa feature; excluded from distribution",
                "first_ui_cpu_complete_ms": 250., "ui_passes": 5, "elapsed_seconds": 4.5,
                "native_window_smoke": {"quiet_ui_passes": 1, "quiet_after_seconds": 1, "close_after_seconds": 4}}

    def test_only_verified_aggregate_fields_are_returned(self):
        report = self.report()
        report["unexpected"] = "must not be copied"
        result = validate(report)
        self.assertNotIn("unexpected", result)
        self.assertEqual(result["quiet_ui_passes"], 1)

    def test_continuous_repaint_slow_startup_missing_smoke_and_incomplete_exit_fail(self):
        for key, value in [("first_ui_cpu_complete_ms", 10_001), ("first_ui_cpu_complete_ms", float("nan")),
                           ("native_window_smoke", None), ("ui_passes", 1), ("elapsed_seconds", 1), ("elapsed_seconds", float("nan")), ("ui_passes", "5"), ("first_ui_cpu_complete_ms", True), ("version", "unexpected text")]:
            report = self.report()
            report[key] = value
            with self.assertRaises(ValueError):
                validate(report)
        for quiet in [0, 9, 300, True]:
            report = copy.deepcopy(self.report())
            report["native_window_smoke"]["quiet_ui_passes"] = quiet
            with self.assertRaises(ValueError):
                validate(report)
