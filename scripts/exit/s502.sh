#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s502 exit test: further compiles wait while syspolicyd is overloaded; the first never does.
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo test -q -p mason-scheduler --test properties -- a_busy_syspolicyd_holds_only_a_further_compile
cargo test -q -p mason-daemon --test limits -- a_busy_syspolicyd_holds_further_compiles_but_never_the_first
cargo test -q -p mason-daemon --lib -- policyd::tests
grep -q 'syspolicyd_max_pct' crates/daemon/src/sched.rs
echo "s502 OK"
