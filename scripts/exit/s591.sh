#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s591 exit test: the Swift monitors' Disk section. No script drives the Swift client against a
# daemon, so the tests are the exit test: the disk core tests and the offscreen section tests
# (checkboxes for two volumes, none for one, side by side wide, stacked narrow). Skipped without swift.
set -euo pipefail
cd "$(dirname "$0")/../.."
grep -q '^swift = "done"' <(sed -n '/^\[feature.disk-history\]/,/^\[feature\.[^d]/p' clients/features.toml)
if ! command -v swift >/dev/null 2>&1; then echo "s591: swift missing, skipped"; exit 0; fi
cd clients/swift
swift test --filter 'DiskViewTests|DiskSectionTests'
echo "s591: ok"
