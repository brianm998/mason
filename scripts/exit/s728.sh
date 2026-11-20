#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s728 exit test: the coverage map picks modules inside a directory test target. In a scratch
# workspace with a two-module integration target that spawns a binary, the coverage job maps
# each module to the source files it ran, and a change to a file only one module exercises
# selects that module alone. Also the selection unit tests. Without the llvm-tools component the
# job test says so and checks only that the job reports it (selection stays on the name scan).
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
cargo test -q -p mason-daemon --lib covmap
cargo test -q -p mason-daemon --lib affected
cargo test -q -p mason-daemon --test it covmap_job:: -- --nocapture
echo "s728: ok"
