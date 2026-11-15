#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s359 exit test: a diagnosis alert lasts only while its task is still in the failure it diagnosed.
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo test -q -p mason-daemon --lib -- a_diagnosis_needs_you_only_while_its_task_is_still_in_the_failure_it_diagnosed
echo "s359 OK"
