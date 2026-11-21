#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s784 exit test: the two fixed base reds pass; the third is a known open one (docs/stages/s784.md).
set -uo pipefail
cd "$(dirname "$0")/../.."
cargo test -q -p mason-pipeline --bin mason no_tracked_file_but_the_definition_is_unmapped || exit 1
cargo test -q -p mason-pipeline --test remote_dispatch || exit 1
