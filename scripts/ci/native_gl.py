#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Explicit Linux QA startup/idle probe; no study input, screenshots or session data."""
import json
import math
import os
from pathlib import Path
import platform
import re
import subprocess
import sys
import tempfile
import time


def validate(report):
    smoke = report.get("native_window_smoke")
    first = report.get("first_ui_cpu_complete_ms")
    quiet = smoke.get("quiet_ui_passes") if isinstance(smoke, dict) else None
    passes = report.get("ui_passes")
    elapsed = report.get("elapsed_seconds")
    version = report.get("version")
    if (report.get("schema_version") != 1 or
            report.get("build") != "developer qa feature; excluded from distribution" or
            not isinstance(version, str) or not re.fullmatch(r"[0-9]{1,5}\.[0-9]{1,5}\.[0-9]{1,5}", version) or
            type(first) not in (float, int) or not math.isfinite(first) or not 0 <= first <= 10_000 or
            type(quiet) is not int or not 1 <= quiet <= 8 or
            smoke.get("quiet_after_seconds") != 1 or smoke.get("close_after_seconds") != 4 or
            type(passes) is not int or passes < 2 or
            type(elapsed) not in (float, int) or not math.isfinite(elapsed) or not 3.5 <= elapsed <= 30):
        raise ValueError("Native QA did not demonstrate finite startup and quiet-window repainting")
    return {"schema_version": 1, "version": report["version"], "backend": "Gl",
            "first_ui_cpu_complete_ms": first, "quiet_ui_passes": quiet,
            "scope": "Linux software OpenGL native startup/idle; no input dialogs, studies or consumer GPU performance tested"}


def main():
    if platform.system() != "Linux" or len(sys.argv) != 2:
        raise ValueError("Run on Linux with the explicit QA executable")
    if os.environ.get("WGPU_BACKEND") != "gl" or os.environ.get("ORV_QA_NATIVE_SMOKE") != "1":
        raise ValueError("Explicit OpenGL and QA smoke settings are required")
    binary = Path(sys.argv[1]).resolve(strict=True)
    started = time.time()
    process = subprocess.Popen([str(binary)], stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
    report_path = Path(tempfile.gettempdir()) / f"OpenRadView-qa-{process.pid}.json"
    try:
        try:
            _, errors = process.communicate(timeout=30)
        except subprocess.TimeoutExpired:
            process.kill()
            process.communicate()
            raise ValueError("Native QA window did not close within its bound") from None
        if process.returncode != 0 or "backend=Gl;" not in errors:
            raise ValueError("Native OpenGL startup failed: " + errors[-6000:])
        if (report_path.is_symlink() or report_path.stat().st_mtime < started or
                report_path.stat().st_size > 32_768):
            raise ValueError("Missing or invalid current QA aggregate")
        result = validate(json.loads(report_path.read_text()))
        print(json.dumps(result, sort_keys=True))
    finally:
        if report_path.is_file() and not report_path.is_symlink() and report_path.stat().st_mtime >= started:
            report_path.unlink()


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, KeyError) as error:
        print(str(error), file=sys.stderr)
        sys.exit(1)
