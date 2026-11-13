# 0204: MASON's CI runs macOS exit-scripts and compose nightly

GitHub caps an account's concurrent macOS jobs. MASON's CI started 9 per push (about 222 job-minutes), and star's landing CI waited behind them (median 20 min, up to 65).

**Decision.** The push run keeps Linux jobs, `test (macos-latest)` and `monitor-swift`. The macOS legs of `exit-scripts` and `monitor-compose` run nightly (`schedule`). A `concurrency` group cancels superseded runs, and cargo is cached. A workspace test checks `cargo fmt --all --check`, because the landing never ran fmt.

**Trade-off.** A macOS-only break in an exit script is found up to a day later.

**Trust.** Edits `.github/workflows/ci.yml` (protected, "gates: CI"); owner approval needed. The tests in `ci_gates.rs` fail if a push-run exit-scripts or monitor-compose matrix lists macOS.
