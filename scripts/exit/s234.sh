#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s234 exit test: with 20 tasks waiting for a broker slot, the views build from the snapshot.
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo test -q -p mason-daemon --test waits_snapshot
cargo test -q -p mason-daemon --lib explain_task_reads
