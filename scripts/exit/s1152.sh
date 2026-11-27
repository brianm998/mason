#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s1152 exit test: write-time rules on every Edit/Write (decision 1141).
#  1. A scripted develop session writes a test with a timing assertion: the Write is denied, the
#     agent receives the rule's message, writes a hang guard instead, and that Write is allowed.
#     The task's detail (what `mason task show` prints) lists exactly one rule hit.
#  2. The rules: MASON's seed rules on sample text, rules read from the base commit and not the
#     worktree, a malformed rules file allowing the edit with one logged line.
#  3. The adapter registering `mason hook edit` with `--settings`.
set -euo pipefail
cd "$(dirname "$0")/../.."

echo "== a scripted develop session"
cargo test -q -p mason-daemon --test it write_rules::

echo "== the rules"
cargo test -q -p mason-daemon --lib rules::tests::

echo "== the adapter's settings"
cargo test -q -p mason-adapters --lib every_session_is_given_the_command_guard
echo "OK s1152"
