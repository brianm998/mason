#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s589 exit test: a command that runs under a step host keeps its slots across a daemon
# restart. With test-suite at 1, a new daemon's broker counts the running host's slot, a
# second request waits, and is granted only once the host's command has ended.
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo test -q -p mason-daemon --lib -- a_command_still_running_under_its_host an_ended_host_or_an_unknown_class
cargo test -q -p mason-daemon --test it -- a_running_check_records_the_slots
echo "s589: ok"
