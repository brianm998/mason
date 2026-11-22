#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s950 exit test: the stage note exists.
set -euo pipefail
cd "$(dirname "$0")/../.."
grep -q "^Summary: " docs/stages/s950.md
