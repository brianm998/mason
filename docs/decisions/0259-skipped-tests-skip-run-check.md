# 0259: skipping a task's test step also skips its agent's run_check

**Decision.** An open debt that skips a task's test step makes the develop agent's `run_check` of the tests answer "not run" without building or testing, including a check already waiting for a slot. The check reports `passed` so the agent finishes and submits, and its text says plainly that nothing was tested.

**Why.** The owner skips testing to stop spending the machine on it; a develop check runs the same tests, and on 2026-11-13 develop checks were most of the compile queue. A skip that leaves them running does not do what the owner asked.

**Trust.** `crates/daemon/src/suites.rs` (verdict logic: running suites) and `scripts/exit/s264.sh` (exit tests) are in `PROTECTED`; `jobs.rs` and `tests/scheduling.rs` are not. The owner asked for testing to be disabled for these tasks on 2026-11-13. The skip still records its debt and downgrades the task's closing verdict; no gate, limit or test is weakened. (Corrected in s271: the first version said none of these paths was protected.)
