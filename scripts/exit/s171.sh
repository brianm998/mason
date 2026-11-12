#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s171 exit test: the digest groups waits by what was waited for.
# A daemon with a `compile` limit of 1 and two tasks whose test steps need it (the slot is held
# from outside, so both queue); then the digest has one `compile` row with a count of at least 1
# and no per-position lines ("in line", "held by"). The daemon runs in the test process, as in
# scheduling.rs, and the digest is read from it directly, not through the `mason digest` command.
set -uo pipefail
cd "$(dirname "$0")/../.."
cargo test -q -p mason-daemon --test wait_classes
