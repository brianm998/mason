#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s681 exit test: a converted all-compile request (an agent's --exclusive) has s552's bound and
# backfill, scoped to compile: with one compile holder that never finishes, the waiters behind
# it are granted after the bound, short work passes before it, and the request still goes first
# once compile drains. A person's exclusive request is unchanged.
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo test -q -p mason-scheduler --test properties -- converted_request a_person_s_exclusive an_exclusive_request_gives_up
cargo test -q -p mason-daemon --test origin_rank
echo "s681: ok"
