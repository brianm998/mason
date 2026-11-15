#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s417 exit test: an agent's mason run wait is credited as it goes and survives a daemon restart.
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo test -q -p mason-adapter-api --lib -- deadline_extra_tests
cargo test -q -p mason-daemon --lib -- sched::tests::an_open_mason_run_wait_is_added_to_the_deadline_before_it_is_granted sched::tests::an_agents_mason_run_wait_is_added_to_its_deadline_when_granted
echo "s417 OK"
