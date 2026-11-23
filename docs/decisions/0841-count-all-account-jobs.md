# 0841: Count every job on the account's runners, queued ones too

Status: accepted (owner asked for it 2026-11-21)

GitHub's free plan runs 20 jobs at once per account (5 on macOS) across all repositories, so a run MASON pushes can wait behind jobs it cannot see. The pool now holds units for every other workflow's running or queued job in every repository a project names, so such a run waits in the broker, in rank order, instead.

- One poller, ETag per list, jobs only for changed lists (a 304 is free).
- macOS jobs count against `gha` and `gha-macos`; `gha_macos_ceiling` defaults to 5.
- If GitHub cannot be read the last count stays and its age is shown; unreadable repositories are named as not counted and do not hold units.
- `crates/daemon/src/sched.rs` is protected (budgets): this changes how many units the pool sees as free, not any verdict.
