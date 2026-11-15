#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s303 exit test: a dropped device lease frees the device even if its fd was duplicated.
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo test -q -p mason-devices
cargo test -q -p mason-daemon --lib -- a_name_glob_from_the_definition_never_leases_pipeline_slot_1
echo "s303 OK"
