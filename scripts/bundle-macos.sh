#!/bin/sh
# SPDX-License-Identifier: GPL-3.0-or-later
set -eu
exec python3 "$(dirname "$0")/macos/bundle.py" "$@"
