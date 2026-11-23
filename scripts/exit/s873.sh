#!/bin/sh
# SPDX-License-Identifier: GPL-3.0-or-later
# Exit test for s873 (#776): a run cut off at its limit keeps its output.
set -e
cd "$(dirname "$0")/../.."
cargo test -p mason-daemon --lib remote::tests::a_run_past_its_limit
cargo test -p mason-daemon --lib remote::tests::the_run_json_carries
