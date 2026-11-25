# 1058: runner units go back as each job finishes

A remote run gives its `gha` unit (and `gha-macos` on macOS) back when its job completes, whatever the conclusion, so the next request in line goes at once. The lease shrinks in the ledger; the hold file is rewritten so a restart counts only jobs still out. Shards run again ask afresh at the run's original rank and wait start. No verdict changes. It touches `crates/scheduler/**`, `sched.rs` and `suites.rs` (protected), at the owner's request of 2026-11-21.
