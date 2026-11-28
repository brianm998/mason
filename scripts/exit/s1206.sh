#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s1206 exit test: every failed or stuck task reaches the recovery entry point and ends in a fix
# (repair), a card or a hand-off brief (decision 1196). The integration tests start temporary
# daemons with the fake adapter: a job panic gets a recovery row and a session, a repairing agent
# puts a task back in the test step with its commit, and a session with no verdict leaves a brief
# that `mason task brief` prints plus a recovery-handoff alert.
set -euo pipefail
cd "$(dirname "$0")/../.."

grep -q '^Summary: ' docs/stages/s1206.md
grep -q 'self.recover_failed(failing);' crates/daemon/src/daemon.rs
grep -q 'recovery::alerts' crates/daemon/src/diagnose.rs
cargo test -q -p mason-daemon --test it recovery::
cargo test -q -p mason-daemon --lib -- recovery::tests diagnose::tests
cargo test -q -p mason-adapter-api --lib -- contract::tests
echo "OK s1206"
