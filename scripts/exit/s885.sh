#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s885 exit test: stage note present and the test world carries the workflow (grep only).
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
grep -q '^Summary: ' docs/stages/s885.md
grep -q '.github/workflows/mason-run.yml' crates/daemon/tests/it/landing_floor.rs
grep -q 'fn a_floor_set_to_github_goes_there' crates/daemon/tests/it/landing_floor.rs
echo ok
