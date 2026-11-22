#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s806 exit test (#783): the `suites` schedule kind: parsing and the full-test runs.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
grep -q '^Summary: ' docs/stages/s806.md
cargo test -q -p mason-daemon --lib -- schedule::tests::a_suites_schedule
cargo test -q -p mason-daemon --test it -- suites_schedule::
