<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
# 0644: the ten-compiles CI failure was a test string match, not the wait view

Status: accepted (s651).

Context: `CI/test (macos-latest)` failed on develop at 386e8e4b in `sched::tests::ten_compiles_behind_a_waiting_exclusive_lease_all_name_it`. The panic was at `crates/daemon/src/sched.rs:3774`, `assert!(!shown.contains("510"))` on the Debug text of `v.tasks`, which includes epoch-ms values; `1795105312602` contains "510".

Decision: no further change. 159f33a7 (decision 0637) already replaced the string match with a task-id check. The wait view's classification of the lease is correct and untouched.

Consequence: none for production code; `scripts/exit/s651.sh` repeats the test to confirm it no longer depends on the clock.
