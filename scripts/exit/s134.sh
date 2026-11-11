#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s134 exit test: the macOS start image keeps its 3:2 aspect. Prints the brand image's
# point size from `monitor-tool mascot-check` (macOS only) and fails if it is not 3:2.
set -uo pipefail
cd "$(dirname "$0")/../.."
if [ "$(uname)" != Darwin ]; then echo "s134: skipped (not macOS)"; exit 0; fi
out=$(cd clients/swift && swift run -q monitor-tool mascot-check 2>&1) || { echo "$out"; exit 1; }
echo "$out"
echo "$out" | grep -q 'start_image is 768.0 x 512.0 points' || { echo "s134: start_image is not 768 x 512"; exit 1; }
echo "s134: start_image is 768 x 512 points (3:2)"
