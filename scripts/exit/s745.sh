#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s745 exit test: a suite's build is sent in RunSpec.build, not joined with &&.
set -euo pipefail
cd "$(dirname "$0")/../.."

grep -q '^Summary: ' docs/stages/s745.md || { echo "s745: stage note has no Summary line" >&2; exit 1; }
[ -f docs/decisions/0738-build-sent-separately.md ] || { echo "s745: decision missing" >&2; exit 1; }
! grep -q '{b} && {' crates/daemon/src/suites.rs crates/daemon/src/hostedcheck.rs || { echo "s745: build still joined" >&2; exit 1; }
grep -q 'fn a_build_goes_in_its_own_field' crates/daemon/src/remote.rs || { echo "s745: test missing" >&2; exit 1; }
echo "s745: ok"
