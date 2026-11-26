#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s1121 exit test (#822): a remote run GitHub does not pick up, or fails for the environment,
# gives back its runner units and waits again at its place in line.
#
# Not here: a real daemon against a fake GitHub over HTTP. These tests use the in-process fake
# GitHub and fake clock of crates/daemon/src/remote.rs and a real broker.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

cargo test -q -p mason-daemon --lib -- \
  remote::tests::a_run_nobody_picks_up \
  remote::tests::a_landing_floor_build_nobody_picks_up \
  remote::tests::a_retry_backing_off_holds_no_unit \
  remote::tests::two_misses \
  remote::tests::a_push_refused_every_time \
  a_run_that_already_waited_in_place \
  a_passing_check_says_how_often \
  an_outage_
echo "s1121 ok"
