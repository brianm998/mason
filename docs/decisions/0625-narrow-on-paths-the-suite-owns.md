# 0625: Narrow a suite on the paths the impact map gives it

Status: accepted (s632).

Context: a path owned by no cargo package made narrowing give up, so client-only tasks ran every cargo test each round.

Decision: before narrowing a suite, drop changed paths the impact map sends only to other suites. If none remain, run the smoke set alone (test step) or nothing (`run_check`). `.mason/smoke-tests.txt` alone is treated like docs. Unmapped paths still run whole.

Consequence: per-task steps are cheaper; the landing batch still runs the full suite, so nothing lands less tested.

Trust: `crates/daemon/src/suites.rs` is protected; owner approval needed.
