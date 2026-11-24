# 1059: runner units go landing, check, test step, debt

The owner chose on 2026-11-21: landing runs first, then agents' checks, then test steps, then debt payments; within a kind the more urgent task, then the longest wait. A person's request still goes first; aging lifts a request only among its own kind.

This is for requests that hold only runner units (`gha`, `gha-macos`, and the `gha-debt` cap). Local slots keep their own rule, where a test step goes before a check (s262, a task further along goes first). The two orders differ on purpose, by the owner's choice. Debt payments rank below all pipeline work for local slots too (the debts band).

`[remote_runs] debt_units` (default 4) caps what debt payments hold in all, through a `gha-debt` pool of that size that each debt asks for beside its `gha` units, and a debt of a sharded suite runs as at most that many shards. The cap is kept by the first project's setting seen by the broker (the pool is account-wide).

Trust: `crates/scheduler/**`, `suites.rs` are in PROTECTED; this changes the order work gets runners, not any verdict.
