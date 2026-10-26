#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Local Clang ASan/UBSan checks of the pinned native decoders, using synthetic data."""
import os
from pathlib import Path
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts/macos"))
import vendor_sources


def main():
    vendor_sources.verify(ROOT)
    if sys.platform != "darwin":
        raise SystemExit("This driver is validated on macOS. Native Windows/Linux sanitizer acceptance is separate.")
    output = ROOT / "target/codec-sanitizers"
    subprocess.run(["cmake", "-S", str(ROOT / "scripts/codecs"), "-B", str(output),
                    "-DCMAKE_C_COMPILER=/usr/bin/clang", "-DCMAKE_CXX_COMPILER=/usr/bin/clang++",
                    "-DCMAKE_BUILD_TYPE=RelWithDebInfo"], check=True)
    subprocess.run(["cmake", "--build", str(output), "--parallel", "4"], check=True)
    environment = os.environ.copy()
    # LeakSanitizer is unsupported by the Apple runtime. Address/UB failures stop
    # the child; a hard deadline also catches a stuck native decoder.
    environment["ASAN_OPTIONS"] = "detect_leaks=0:halt_on_error=1"
    environment["UBSAN_OPTIONS"] = "halt_on_error=1:print_stacktrace=1"
    subprocess.run([str(output / "codec-checks"), str(ROOT / "crates/orv-core/tests/fixtures")],
                   env=environment, timeout=15, check=True)


if __name__ == "__main__": main()
