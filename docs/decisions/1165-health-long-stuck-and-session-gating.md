# 1165: stuck:long, one working health session, health proposals outside the weekly cap

Decision:
- `mason_core::health::judge` raises `stuck:long:<id>` (Warn) for a step task in one state longer than `max(typical_hi x long_factor, floor)` (factor 6; floor 6 h, landing 2 h), or with more than `checks_max` (6) full `run_check` rounds in the current run. Evidence: time, rounds, attempt, last three check results. The key is per task; a new state entry restarts the clock, and the cooldown handles repeats.
- At most one *working* health session runs; a session waiting for the owner (state `open`) keeps its banner but holds nothing back. One older than `[health] needs_owner_ttl_m` (120) is ended as `timed_out` with an alert and a `timed_out` health row; its `handled` rows are then ignored, so its keys are fresh.
- Proposals filed by the health session for MASON's own project skip the weekly self-task count and have their own limit, `[health] health_proposals_per_week` (10). The rollback cooldown, `max_open`, `max_steps` and duplicate detection still apply. This is a change to a guard (`selfguard::check_filing_health`); the owner approved it in task 1209 on 2026-11-26.
- New health action `restart_round` (`mason task retry <id> --from develop --restart --message <reason>`); the prompt tells the session what to do with `stuck:long`.
