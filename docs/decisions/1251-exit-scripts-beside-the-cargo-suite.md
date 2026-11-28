# 1251: exit scripts beside the cargo suite, and slow ones are "not run" in a check

Status: approved by the owner on 2026-11-27 (pipeline-health review, #1291). Touches protected `scripts/exit/**`, `suites.rs` and the check's report.

Decision:

- When the cargo-workspace suite runs in the same check or test step, before the exit scripts, set `MASON_TESTS_ALREADY_RAN=1` for them. Their own cargo-test parts are covered by that run, and its red is reported there. The cargo run may be narrowed to what the change affects; the landing batch runs the workspace whole and the landing exit tier keeps the cargo parts.
- A run filtered to named tests, or a fast track, does not count as the cargo suite running.
- In an agent's check, a script killed at the per-script limit is `NOT RUN`, and the check is not red for it. This changes what a check reports, not what may land: the test step still fails on it. Under a fast track the check is the only run of the added scripts, so it stays a failure there.

Consequences:

- A check can be green while a script has not finished. Its answer names the script.
- Where the cargo run is narrowed, a script's cargo test outside the narrowed set runs only at landing.
- The heavy scripts' 900 s weights also shape the GitHub CI shards, where those scripts are fast, so those shards are somewhat less even.

Rejected: raising the 900 s limit (each kill would cost more), and setting the variable on every MASON run (a run without the cargo suite would prove less than it says).
