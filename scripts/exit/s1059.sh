#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s1059 exit test: a real masond against a local channel updates itself by priority.
#  - a docs-only commit: no takeover, the status says "none"
#  - a daemon change while a scripted agent's check (no call id) is in flight: it waits,
#    then takes over when the check ends
#  - an urgent-marked change: takes over without waiting for the check
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo test -q -p mason-daemon --lib -- update_plan::
cargo test -q -p mason-daemon --test it -- \
  update::a_docs_only_commit_causes_no_takeover \
  update::a_daemon_change_waits_for_a_check \
  update::a_normal_update_goes_when_the_check_ends \
  update::auto_disabled_starts_no_update
echo "s1059: ok"
