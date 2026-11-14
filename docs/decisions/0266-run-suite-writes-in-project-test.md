# 0266: `run_suite` writes in the project's test command for every caller

**Decision.** `{project_test}` is written into a suite where the suite is run (`run_suite`), not only by the test step. A debt run, a ledger rerun and a landing batch run a suite exactly as the test step would.

**Why.** Three of the four callers of `run_suite` passed the suite as the definition has it, so a suite that names `{project_test}` (mason's `cargo-workspace`) could not be run by them: `mason debt run` failed with exit 127 and never paid.

**Trust.** `crates/daemon/src/suites.rs` (verdict logic: running suites) and `scripts/exit/s271.sh` (exit tests) are in `PROTECTED`, so this needs the owner's approval; `tests/testing.rs` is not. No gate, limit or test is weakened: a suite now runs where it could not run before, and a red still leaves the debt open. This stage also corrects the Trust lines of 0255-0259, which said none of their paths was protected.
