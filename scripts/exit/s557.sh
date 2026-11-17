#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s557 exit test: the monitor builds without the defaultApprovalPriority isolation warning.
set -uo pipefail
cd "$(dirname "$0")/../../clients/swift"
out=$(swift build 2>&1) || { echo "$out"; exit 1; }
if echo "$out" | grep -q 'nonisolated context'; then echo "$out" | grep 'nonisolated context'; exit 1; fi
swift test 2>&1 | tail -3
