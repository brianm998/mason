#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s1015 exit test: unit narrowing adds no note.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
f=crates/daemon/src/affected.rs
if grep -q 'lib: {} of {} modules, by name scan' "$f"; then echo "unit note still present"; exit 1; fi
grep -q 'g.unit_modules.extend' "$f"
echo "s1015 passed"
