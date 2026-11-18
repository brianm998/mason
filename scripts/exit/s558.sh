#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s558 exit test: a check feeds the develop run's counts and history; the phase wording is right.
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo test -q -p mason-core checks::tests
cargo test -q -p mason-daemon --test it hosted_check::
echo "s558: ok"
