# 0389: a task's test step narrows by cargo package

Status: accepted (owner, 2026-11-14 review)

## Decision

`narrow_packages = true` on a suite makes a task's test step run its `dev_command` for the changed crates and their dependents. The result is treated as s386's narrowed runs: no evidence, no green-at-base, no run time, no cached baseline, and a note in the closing note. The landing batch runs `command` whole, so the full workspace still runs before anything lands.

## Why

Whole-workspace builds in every MASON task's test step were most of the compile queue. `run_check` already narrows this way. Residual risk: a failure that only shows in an unrelated crate is caught by the batch, not the task.
