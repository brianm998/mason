#!/bin/sh
# SPDX-License-Identifier: GPL-3.0-or-later
# `RUSTDOC` for the cargo-workspace suite's run (s634): the doc-test harness gets the same
# `-- -Z unstable-options --report-time` as the test executables, and stable libtest accepts
# it only with RUSTC_BOOTSTRAP=1 in its own environment. Setting the variable here, not on
# cargo, keeps it out of every compile: anyhow, thiserror and proc-macro2 rerun their build
# scripts when it changes, which recompiles most of the workspace.
RUSTC_BOOTSTRAP=1 exec rustdoc "$@"
