<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
# 1219: net outage tests are Unix-only

Task #1240. Stage s1229.

## Decision

Daemon integration tests that depend on sh hooks or Unix file modes carry `#![cfg(unix)]` on the whole file, as the other such files do. The windows job builds all targets, so an ungated file fails the whole job. The gate is not put on one helper, since the tests would then be dead code under `clippy -D warnings`.
